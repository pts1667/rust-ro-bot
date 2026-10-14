//! The local llama.cpp model through smolgent.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use smolgent::{ChatMessage, ChatProvider, ChatResponse, ProviderConfig};

/// A strategist step or a helper question that takes longer than this is dropped.
pub const LLM_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone)]
pub struct Llm {
    /// Tools offered, the model may answer or call them.
    pub free: ChatProvider,
    /// Tools offered, the model must call one (`tool_choice: required`).
    pub forced: ChatProvider,
    base_url: String,
    /// Chance that a strategist step goes to the LLM rather than the deterministic fallback.
    pub share: f32,
    /// Layout of the strategist turns.
    pub order: super::prompt::Order,
    /// Send the few-shot example turns.
    pub examples: bool,
    /// Present the options in random order.
    pub shuffle: bool,
    /// How context is kept between strategist steps.
    pub context: ContextMode,
}

/// How the strategist keeps context between steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum ContextMode {
    /// SKILL.state: a fresh prompt each step, only the validated state Σ carried over.
    #[default]
    SkillState,
    /// One agent conversation for the whole run, compacted by the model when it grows long.
    History,
}

impl Llm {
    /// `think`: tokens the model may reason before a strategist answer; the reasoning is never sent back.
    pub fn new(
        base_url: &str,
        model: &str,
        think: u32,
        sampling: &crate::config::Sampling,
    ) -> Result<Self> {
        let config = || -> Result<ProviderConfig> {
            let mut c = ProviderConfig::llama_cpp(base_url, model).map_err(|e| anyhow!("{e}"))?;
            // The model thinks at length by default; llama.cpp ignores `reasoning.enabled = false` and only honours this.
            c.extra_body
                .insert("thinking_budget_tokens".into(), json!(think));
            for (key, value) in sampling.fields() {
                c.extra_body.insert(key.into(), value);
            }
            Ok(c)
        };
        let free = ChatProvider::new(config()?);
        let mut forced_config = config()?;
        forced_config
            .extra_body
            .insert("tool_choice".into(), json!("required"));
        Ok(Self {
            free,
            forced: ChatProvider::new(forced_config),
            base_url: base_url.trim_end_matches('/').to_owned(),
            share: 1.0,
            order: Default::default(),
            examples: true,
            shuffle: true,
            context: Default::default(),
        })
    }

    /// Whether the server answers `/health`.
    pub async fn healthy(&self) -> bool {
        let url = format!("{}/health", self.base_url);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build();
        match client {
            Ok(c) => c
                .get(url)
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false),
            Err(_) => false,
        }
    }

    /// Whether llama.cpp has an idle slot now (`/slots`). A server that does not say is taken to have one.
    pub async fn slot_free(&self) -> bool {
        let url = format!("{}/slots", self.base_url);
        let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
        else {
            return true;
        };
        let Ok(response) = client.get(url).send().await else {
            return true;
        };
        let Ok(slots) = response.json::<Vec<Value>>().await else {
            return true;
        };
        slots.is_empty()
            || slots
                .iter()
                .any(|s| s.get("is_processing").and_then(Value::as_bool) != Some(true))
    }

    /// One request whose answer must match `schema` (grammar-constrained by llama.cpp), parsed as JSON.
    pub async fn ask_json(
        &self,
        system: &str,
        user: &str,
        schema: Value,
        max_tokens: u32,
    ) -> Result<Value> {
        let messages = [ChatMessage::system(system), ChatMessage::user(user)];
        let mut request = self.free.build_request(&messages, Vec::new());
        request.response_format =
            Some(json!({"type": "json_schema", "json_schema": {"schema": schema}}));
        // Dialog and chat answers are simple, and thinking does not mix well with grammar sampling.
        request
            .extra
            .insert("thinking_budget_tokens".into(), json!(0));
        request.max_tokens = Some(max_tokens);
        let response: ChatResponse =
            tokio::time::timeout(LLM_TIMEOUT, self.free.send_request(request))
                .await
                .context("LLM timed out")?
                .map_err(|e| anyhow!("LLM request failed: {e}"))?;
        let text = response.message.content.text();
        parse_json_loose(&text).with_context(|| format!("LLM answer is not JSON: {text}"))
    }
}

/// Parses JSON that may be wrapped in a code fence or surrounded by text.
pub fn parse_json_loose(text: &str) -> Option<Value> {
    let t = text.trim();
    if let Ok(v) = serde_json::from_str(t) {
        return Some(v);
    }
    let start = t.find('{')?;
    let end = t.rfind('}')?;
    serde_json::from_str(&t[start..=end]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loose_json_handles_fences() {
        assert_eq!(
            parse_json_loose("```json\n{\"a\": 1}\n```"),
            Some(json!({"a": 1}))
        );
        assert_eq!(parse_json_loose("{\"a\": 1}"), Some(json!({"a": 1})));
        assert_eq!(parse_json_loose("nope"), None);
    }
}
