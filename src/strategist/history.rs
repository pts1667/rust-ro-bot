//! The other way to keep context: one ordinary agent conversation for the whole run, every strategist step a new
//! user turn in it, and smolgent's compaction when it grows long. Compared with SKILL.state, the model sees its
//! earlier choices and what came of them, at the cost of a long and repetitive context. The validated Σ, the option
//! menu and the fallback stay the same.
//!
//! A turn ends with the first valid `commit`: asked for a closing line instead, the model kept committing until the
//! round limit (11 commits a turn, ~2.4k tokens). Compaction drops the oldest turns by code: the model never made a
//! valid compaction call (`"turn_ids": 0`, ids it could not see, the protected system prompt), and each failed call
//! made the context bigger.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Result, anyhow};
use serde_json::Value;
use smolgent::{
    AgentEvent, AgentEventReceiver, ChatSession, CompactionConfig, NotificationConfig,
    SessionConfig, TelemetryConfig, TelemetryEvent, TelemetryEventReceiver, Tool, ToolRegistry,
};

use super::llm::{LLM_TIMEOUT, Llm};
use super::options::Opt;
use super::skill_state::StrategicState;
use super::{LlmDecision, Snapshot, commit_tool, examples, finish, lookup_tools, prompt, shuffled};
use crate::knowledge::Knowledge;

/// Estimated tokens at which the oldest turns are dropped, and the size that leaves. Each llama.cpp slot holds 100k.
pub const COMPACT_AT: usize = 70_000;
pub const COMPACT_TO: usize = 35_000;
/// Room for the events of one step; the channels are bounded and drained after each step.
const CHANNEL: usize = 4096;
/// Tool rounds of one step (lookups, then the commit).
const MAX_ROUNDS: usize = 10;

/// The tool whose successful call ends a turn.
const COMMIT: &str = "commit";

const HISTORY_NOTE: &str = "\n\nThis is one long game: the earlier turns above are your own past choices and what \
followed. The oldest turns may have been removed. Learn from them, but answer each new turn from its own OPTIONS.";

pub struct History {
    session: ChatSession,
    turn: u32,
    events: Option<AgentEventReceiver>,
    telemetry: Option<TelemetryEventReceiver>,
}

impl History {
    pub fn new(llm: &Llm) -> Self {
        Self::with(llm, COMPACT_AT, COMPACT_TO, false, false)
    }

    /// `model_compaction` first asks the model to compact (the trim still follows if it falls short); `trace` also
    /// records every request and response. Both for `history-check`.
    pub fn with(
        llm: &Llm,
        compact_at: usize,
        compact_to: usize,
        model_compaction: bool,
        trace: bool,
    ) -> Self {
        let mut system = format!("{}{HISTORY_NOTE}", prompt::SYSTEM);
        if llm.examples {
            system.push_str(&examples::text());
        }
        let config = SessionConfig {
            max_tool_rounds: MAX_ROUNDS,
            stop_after_tools: vec![COMMIT.into()],
            compaction: CompactionConfig {
                protect_recent_turns: 6,
                model_compaction,
                ..CompactionConfig::trim_only(compact_at, compact_to)
            },
            event_channel_capacity: Some(CHANNEL),
            notifications: NotificationConfig {
                compaction: true,
                tool_payloads: trace,
                ..NotificationConfig::all()
            },
            telemetry: if trace {
                TelemetryConfig {
                    model_metadata: true,
                    model_payloads: true,
                    tool_calls: true,
                    tool_payloads: true,
                    messages: false,
                }
            } else {
                TelemetryConfig::none()
            },
            telemetry_channel_capacity: Some(CHANNEL),
            ..Default::default()
        };
        let (session, events, telemetry) =
            ChatSession::with_system_prompt_config_and_telemetry(system, config);
        Self {
            session,
            turn: 0,
            events,
            telemetry,
        }
    }

    /// What the session reported since the last call: compaction progress always, requests and responses with
    /// `trace`.
    pub fn drain(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(events) = &self.events {
            while let Ok(event) = events.try_recv() {
                match event {
                    AgentEvent::CompactionBreakdown {
                        total_estimated_tokens,
                        largest_turns,
                        ..
                    } => {
                        out.push(format!(
                            "compaction breakdown: {total_estimated_tokens} tokens"
                        ));
                        for t in largest_turns.iter().take(12) {
                            let preview: String = t.summary.chars().take(70).collect();
                            out.push(format!(
                                "    turn_id={} {:?} {} tokens: {preview}",
                                t.turn_id, t.role, t.estimated_tokens
                            ));
                        }
                    }
                    AgentEvent::CompactionStarted { .. }
                    | AgentEvent::CompactionTrimmed { .. }
                    | AgentEvent::CompactionToolCallStarted { .. }
                    | AgentEvent::CompactionFinished { .. }
                    | AgentEvent::CompactionSkipped { .. }
                    | AgentEvent::MaxToolRoundsReached { .. }
                    | AgentEvent::ToolCallFailed { .. } => out.push(format!("{event:?}")),
                    _ => {}
                }
            }
        }
        if let Some(telemetry) = &self.telemetry {
            while let Ok(event) = telemetry.try_recv() {
                match event {
                    TelemetryEvent::ModelRequest {
                        message_count,
                        request: Some(request),
                        ..
                    } => {
                        let tools: Vec<&str> = request.tools.iter().map(|t| t.name()).collect();
                        out.push(format!(
                            "request: {message_count} messages, tools {tools:?}"
                        ));
                        if tools.iter().all(|t| t.starts_with("compact_"))
                            && let Some(last) = request.messages.last()
                        {
                            out.push(format!(
                                "    last message ({:?}):
{}",
                                last.role,
                                last.content.text()
                            ));
                        }
                    }
                    TelemetryEvent::ModelResponse {
                        response: Some(response),
                        ..
                    } => {
                        let m = &response.message;
                        out.push(format!("response: content {:?}", m.content.text()));
                        for call in &m.tool_calls {
                            out.push(format!(
                                "    call {}({})",
                                call.function.name, call.function.arguments
                            ));
                        }
                    }
                    TelemetryEvent::ToolCallFinished { .. }
                    | TelemetryEvent::ToolCallFailed { .. } => out.push(format!("    {event:?}")),
                    _ => {}
                }
            }
        }
        out
    }

    pub fn estimated_tokens(&self) -> usize {
        self.session.estimated_context_tokens()
    }

    /// One strategist step as a new turn of the conversation.
    pub async fn step(
        &mut self,
        llm: &Llm,
        knowledge: &Knowledge,
        state: &StrategicState,
        snap: &Snapshot,
        options: &[Opt],
    ) -> Result<LlmDecision> {
        let start = Instant::now();
        self.turn += 1;
        let options = shuffled(llm, options);
        let user = format!(
            "TURN {}\n{}",
            self.turn,
            prompt::user_message(llm.order, state, snap, &options)
        );
        let prompt_chars = user.len();
        self.session.push_user(user);

        // The commit tool records its arguments and ends the turn; a wrong option id fails the call, so the model
        // retries.
        let committed: Arc<Mutex<Option<Value>>> = Default::default();
        let mut registry: ToolRegistry = lookup_tools(knowledge, snap);
        let (record, ids) = (
            committed.clone(),
            options.iter().map(|o| o.id.clone()).collect::<Vec<_>>(),
        );
        registry.insert(Tool::new(commit_tool(None), move |args: Value| {
            let record = record.clone();
            let ids = ids.clone();
            Box::pin(async move {
                let id = args["option"].as_str().unwrap_or_default().to_owned();
                if !ids.contains(&id) {
                    return Err(smolgent::Error::Tool(format!(
                        "{id:?} is not an option; choose one of {}",
                        ids.join(", ")
                    )));
                }
                *record.lock().unwrap() = Some(args);
                Ok(Value::String("committed".into()))
            })
        }));

        let result = tokio::time::timeout(
            LLM_TIMEOUT,
            self.session.run_with_tools(&llm.free, &registry),
        )
        .await;
        let args = committed.lock().unwrap().take();
        match (args, result) {
            (Some(args), _) => finish(args, &options, Vec::new(), prompt_chars, start),
            (None, Err(_)) => Err(anyhow!("LLM timed out")),
            (None, Ok(Err(e))) => Err(anyhow!("LLM request failed: {e}")),
            (None, Ok(Ok(_))) => Err(anyhow!("the model answered without committing")),
        }
    }
}
