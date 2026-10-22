//! The strategist: picks the goal GOAP works on. It runs beside the agent and never blocks it.
//!
//! With the LLM, each step follows SKILL.state: a fresh smolgent session gets the specification P, the
//! strategic state Σ and an observation digest O, may look facts up with two tools, and must `commit` an option
//! and a patch ΔΣ. The session (lookups and any reasoning) is then dropped; only the validated Σ survives.
//! Without the LLM, or when it fails, a deterministic fallback picks from the same options.

pub mod decided;
pub mod eval;
mod examples;
pub mod helper;
pub mod history;
pub mod llm;
pub mod options;
pub mod prompt;
pub mod skill_state;

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use smolgent::{ChatMessage, ChatSession, Tool, ToolDefinition, ToolRegistry};
use tokio::sync::{mpsc, watch};
use tracing::Instrument;

use crate::api::Observation;
use crate::goap::{Goal, WorldState};
use crate::knowledge::Knowledge;
use crate::memory::Counters;
use llm::{LLM_TIMEOUT, Llm};
use options::{Opt, build_options, fallback};
use skill_state::StrategicState;

/// Lookup rounds before the model must commit.
const LOOKUP_ROUNDS: usize = 8;
/// Lookups run per round; extra calls are answered with a refusal.
const LOOKUPS_PER_ROUND: usize = 8;
/// The user turn after a plain-text answer: llama.cpp reads one trailing assistant message as a prefill to continue
/// and refuses two.
const COMMIT_NUDGE: &str = "Call commit now with your choice.";
/// Quests returned per `quest_info` search: a job change has one quest per guild NPC.
const QUEST_HITS: u32 = 4;

/// What the agent tells the strategist about the game, refreshed after every step.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub obs: Observation,
    pub ws: WorldState,
    pub goal: Option<Goal>,
    pub goal_secs: u64,
    pub last_outcome: String,
    pub weight_frac: f32,
    /// HP the healing items in the inventory would give back.
    pub heal_hp: u32,
    /// Number of healing items in the inventory.
    pub heal_items: u32,
    pub loot_items: u32,
    pub potion_price: u64,
    pub kills: u64,
    pub deaths: u64,
    pub kills_by_mob: Vec<(String, u64)>,
    pub deaths_by_cause: Vec<(String, u64)>,
    pub journal: Vec<String>,
    pub visited: HashSet<String>,
    /// Goals that failed recently, not to be offered again for a while.
    pub recent_failures: Vec<Goal>,
    /// Shops the bot cannot buy from, as `map:x,y`.
    pub bad_shops: Vec<String>,
    /// Monsters that keep killing the bot at its level: left alone like `avoid_mobs`.
    pub losing_to: Vec<String>,
    /// Where the bot stands in the quests it took up.
    pub quests: crate::quests::QuestBook,
    /// What people asked the bot in chat, not decided yet.
    pub requests: Vec<crate::social::Request>,
    /// The bot may play in a party (not `--solo`).
    pub party_play: bool,
    /// The characters the bot was told it plays with.
    pub friends: Vec<String>,
    /// The strategist's notes (Σ), as the agent last got them.
    pub notes: Vec<String>,
    /// The job the strategist aims for (Σ).
    pub job_target: Option<String>,
    /// The bot heals its party members (the healer's role, as an Acolyte or Priest).
    pub can_heal: bool,
    /// Maps the bot died crossing lately: no hunting ground on a way through them.
    pub deadly_ways: Vec<String>,
    /// The towns of the `alive_towns` plugin and the town quests taken, with their rewards.
    pub town_quests: crate::behaviors::town_quests::TownQuestBook,
}

/// Why the strategist should think now. The texts are only read through `Debug`, in the trigger log.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum Event {
    LevelUp,
    JobLevelUp,
    Died,
    MapChanged,
    GoalDone(String),
    GoalFailed(String),
    /// Someone asked the bot for something in chat.
    Asked,
    /// Notes the bot took on the players' forum, for Σ.
    ForumNotes(Vec<String>),
    /// Lessons the decision model kept from the forum: a note each, and what it changes in Σ.
    ForumLessons(Vec<crate::voice::known::Lesson>),
}

#[derive(Debug, Clone)]
pub struct Directive {
    pub seq: u64,
    pub goal: Goal,
    /// The id of the option chosen: `ask:<n>` takes the request `n`.
    pub option: String,
    /// The requests the decision saw: those not taken are declined.
    pub asked: Vec<u64>,
    /// Why the bot cannot do some of them, by request id.
    pub refusals: Vec<(u64, String)>,
    pub reason: String,
    pub state: StrategicState,
    pub by_llm: bool,
    pub counters: Counters,
}

pub struct Handle {
    pub directives: watch::Receiver<Option<Directive>>,
    pub events: mpsc::UnboundedSender<Event>,
    pub snapshots: watch::Sender<Option<Snapshot>>,
}

/// The settings of the strategist task, see `spawn`.
pub struct Settings {
    pub knowledge: Knowledge,
    pub llm: Option<Llm>,
    /// The decision model, which takes its share of the steps when there is no LLM.
    pub decider: Option<decided::Decider>,
    /// The strategist thinks at least this often, besides on every event.
    pub period: Duration,
}

/// The channels the strategist task talks through: directives out, events and snapshots in, and the goal changes.
struct Channels {
    directives: watch::Sender<Option<Directive>>,
    events: mpsc::UnboundedReceiver<Event>,
    snapshots: watch::Receiver<Option<Snapshot>>,
    goal_changes: Arc<AtomicU64>,
}

/// The outcome of one LLM step.
#[derive(Debug)]
pub struct LlmDecision {
    pub option: Opt,
    pub reason: String,
    pub patch: Value,
    pub lookups: Vec<String>,
    pub prompt_chars: usize,
    pub millis: u128,
}

/// What one LLM step reads: the model, the game knowledge for the lookups, the snapshot, and the options to pick from.
#[derive(Clone, Copy)]
pub struct Step<'a> {
    pub llm: &'a Llm,
    pub knowledge: &'a Knowledge,
    pub snap: &'a Snapshot,
    pub options: &'a [Opt],
}

/// When an LLM step started and how long its first prompt was: what the decision reports about its cost.
#[derive(Clone, Copy)]
struct Cost {
    start: Instant,
    prompt_chars: usize,
}

/// `goal_changes` gets the sequence number of every directive whose goal differs from the previous one, so
/// long behaviours can stop early.
pub fn spawn(
    settings: Settings,
    mut state: StrategicState,
    goal_changes: Arc<AtomicU64>,
) -> Handle {
    examples::scrub_state(&mut state);
    let (dir_tx, dir_rx) = watch::channel(None);
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let (snap_tx, snap_rx) = watch::channel(None);
    let channels = Channels {
        directives: dir_tx,
        events: ev_rx,
        snapshots: snap_rx,
        goal_changes,
    };
    tokio::spawn(run(settings, state, channels).in_current_span());
    Handle {
        directives: dir_rx,
        events: ev_tx,
        snapshots: snap_tx,
    }
}

async fn run(settings: Settings, mut state: StrategicState, channels: Channels) {
    let Channels {
        directives,
        mut events,
        mut snapshots,
        goal_changes,
    } = channels;
    let mut seq = 0;
    let mut last_goal: Option<Goal> = None;
    let mut counters = Counters::default();
    // The conversation of the history mode, kept for the whole run.
    let mut history: Option<history::History> = None;
    // Wait for the first snapshot.
    while snapshots.borrow().is_none() {
        if snapshots.changed().await.is_err() {
            return;
        }
    }
    let mut why = "start".to_string();
    loop {
        let snap = snapshots.borrow().clone();
        let Some(snap) = snap else { return };
        match decide(&settings, &mut state, &snap, &mut counters, &mut history).await {
            Ok((chosen, reason, by_llm)) => {
                let goal = chosen.goal;
                seq += 1;
                tracing::info!(target: "strategist", seq, trigger = %why, goal = %goal.label(), by_llm, %reason, "directive");
                let asked: Vec<u64> = snap.requests.iter().map(|r| r.id).collect();
                // Someone waits for an answer: the step under way stops so the agent says it now
                if last_goal.as_ref() != Some(&goal) || !asked.is_empty() {
                    goal_changes.store(seq, Ordering::SeqCst);
                    last_goal = Some(goal.clone());
                }
                let refusals = options::request_refusals(&snap);
                let d = Directive {
                    seq,
                    goal,
                    option: chosen.id,
                    asked,
                    refusals,
                    reason,
                    state: state.clone(),
                    by_llm,
                    counters: counters.clone(),
                };
                if directives.send(Some(d)).is_err() {
                    return;
                }
            }
            Err(error) => tracing::warn!(target: "strategist", %error, "no decision"),
        }
        // Next trigger: an event, or the period.
        tokio::select! {
            ev = events.recv() => match ev {
                Some(ev) => {
                    why = format!("{ev:?}");
                    take_notes(&mut state, &ev);
                }
                None => return,
            },
            _ = tokio::time::sleep(settings.period) => why = "periodic".into(),
        }
        // Let the agent publish the snapshot that goes with the event, and fold bursts of events into one step.
        tokio::time::sleep(Duration::from_millis(400)).await;
        while let Ok(ev) = events.try_recv() {
            why = format!("{why}+{ev:?}");
            take_notes(&mut state, &ev);
        }
    }
}

/// Adds the notes of a `ForumNotes` or `ForumLessons` event to Σ, through the same checks as the model's own patches.
/// A lesson may also leave a monster alone for a few levels, or set the job to head for when none is set.
fn take_notes(state: &mut StrategicState, ev: &Event) {
    match ev {
        Event::ForumNotes(notes) => {
            let report = state.apply_patch(&json!({ "notes": notes }));
            tracing::info!(target: "strategist", ?notes, applied = !report.applied.is_empty(), "notes from the forum");
        }
        Event::ForumLessons(lessons) => apply_lessons(state, lessons, "forum: "),
        _ => {}
    }
}

/// Adds lessons to Σ: a note each (`prefix` says where it comes from), and what the lesson changes: a monster left
/// alone for a few levels, or the job to head for when none is set.
pub fn apply_lessons(
    state: &mut StrategicState,
    lessons: &[crate::voice::known::Lesson],
    prefix: &str,
) {
    use crate::voice::known::Effect;
    for lesson in lessons {
        let note = format!("{prefix}{}", lesson.note.render());
        let report = state.apply_patch(&json!({ "notes": [note] }));
        match &lesson.effect {
            Effect::Avoid { monster, until } => state.caution(monster, *until),
            Effect::Job(job) if state.job_target.is_none() => {
                state.apply_patch(&json!({ "job_target": job }));
            }
            _ => {}
        }
        tracing::info!(target: "strategist", %note, effect = ?lesson.effect, applied = !report.applied.is_empty(), "lesson");
    }
}

/// Sets the first job a Novice heads for, which it did not have: one at random among the jobs its stats still suit.
/// Without it, every bot kept the default build and became a Swordman, and the decision model, asked, picked
/// Swordman 7 times in 10.
fn pick_first_job(state: &mut StrategicState, snap: &Snapshot) {
    let s = &snap.obs.me.stats;
    pick_first_job_from(state, [s.str, s.agi, s.vit, s.int, s.dex, s.luk]);
}

/// `pick_first_job` on the stats as they are.
fn pick_first_job_from(state: &mut StrategicState, stats: [u32; 6]) {
    let fresh = stats.iter().all(|v| *v == stats[0]);
    let jobs = skill_state::open_first_jobs(stats, &state.stat_build);
    let job = jobs[rand::random_range(0..jobs.len())];
    state.head_for(job, fresh);
    tracing::info!(target: "strategist", job, ?jobs, fresh, build = %state.stat_build, "first job chosen");
}

/// The job level from which a first job's character heads for a second job.
const SECOND_JOB_LEVEL: u32 = 40;

/// Sets the second job a character past job level 40 heads for, when it has none: one at random among the second
/// jobs of its first job (Swordman: Knight or Crusader). The quests of the other second jobs are left alone then.
fn pick_second_job(state: &mut StrategicState, snap: &Snapshot) {
    let jobs = skill_state::second_jobs_of(&snap.obs.me.job);
    if jobs.is_empty() {
        return;
    }
    let job = jobs[rand::random_range(0..jobs.len())];
    state.head_for(job, false);
    tracing::info!(target: "strategist", job, ?jobs, "second job chosen");
}

/// One decision: the LLM or the decision model when there is one (for its share of the steps), the fallback
/// otherwise or when the model fails.
async fn decide(
    settings: &Settings,
    state: &mut StrategicState,
    snap: &Snapshot,
    counters: &mut Counters,
    history: &mut Option<history::History>,
) -> Result<(Opt, String, bool)> {
    let knowledge = &settings.knowledge;
    let llm = settings.llm.as_ref();
    let decider = settings.decider.as_ref();
    state.on_job(&snap.obs.me.job);
    // Reborn, a character follows the line of the second job it was: its high first job, then its transcendent job
    let me = &snap.obs.me;
    if me.reborn
        && state.pinned_jobs.is_empty()
        && let Some(past) = me.past_job.as_deref()
        && !crate::party::same_job(&me.job, past)
        && state
            .job_target
            .as_deref()
            .is_none_or(|t| !crate::party::same_job(t, past))
    {
        state.head_for_reborn(past);
        tracing::info!(target: "strategist", past, "reborn: heading for the transcendent job");
    }
    if state.job_target.is_none() && state.pinned_jobs.is_empty() && !me.reborn {
        if snap.obs.me.job.eq_ignore_ascii_case("novice") {
            pick_first_job(state, snap);
        } else if skill_state::is_first_job(&snap.obs.me.job)
            && snap.obs.me.job_level >= SECOND_JOB_LEVEL
        {
            pick_second_job(state, snap);
        }
    }
    // The decision model looks back at what happened lately, for notes, before it decides
    if let Some(d) = decider {
        match d.learn(state, snap).await {
            Ok(lessons) => apply_lessons(state, &lessons, ""),
            Err(error) => {
                tracing::warn!(target: "strategist", error = %format!("{error:#}"), "no lessons")
            }
        }
    }
    let options = build_options(snap, knowledge, state).await?;
    tracing::debug!(target: "strategist", options = ?options.iter().map(|o| (&o.id, o.score)).collect::<Vec<_>>(), "options");
    if options.len() == 1 {
        return Ok((options[0].clone(), "only option".into(), false));
    }
    // A share of the steps goes to the fallback, to compare the two and to save LLM time. A step never waits in the
    // LLM server's queue: with every slot busy (other bots, the forum) the fallback decides.
    let llm = llm.filter(|l| rand::random::<f32>() < l.share);
    let llm = match llm {
        Some(l) if !l.slot_free().await => {
            tracing::info!(target: "strategist", "every LLM slot is busy, the fallback decides");
            None
        }
        other => other,
    };
    if let Some(llm) = llm {
        counters.llm_calls += 1;
        let input = Step {
            llm,
            knowledge,
            snap,
            options: &options,
        };
        let step = match llm.context {
            llm::ContextMode::SkillState => llm_step(input, state).await,
            llm::ContextMode::History => {
                let h = history.get_or_insert_with(|| history::History::new(llm));
                let step = h.step(input, state).await;
                for line in h.drain() {
                    tracing::info!(target: "strategist", "history: {line}");
                }
                tracing::info!(target: "strategist", context_tokens = h.estimated_tokens(), "history");
                step
            }
        };
        match step {
            Ok(d) => {
                counters.llm_prompt_chars += d.prompt_chars as u64;
                let report = state.apply_patch(&examples::scrub_patch(d.patch.clone()));
                if !report.rejected.is_empty() {
                    counters.llm_invalid += 1;
                }
                tracing::info!(
                    target: "strategist",
                    option = %d.option.id,
                    ms = d.millis,
                    prompt_chars = d.prompt_chars,
                    lookups = ?d.lookups,
                    patch = %d.patch,
                    applied = ?report.applied,
                    rejected = ?report.rejected,
                    "LLM step"
                );
                return Ok((d.option, d.reason, true));
            }
            Err(error) => {
                counters.llm_invalid += 1;
                tracing::warn!(target: "strategist", %error, "LLM step failed, using the fallback");
            }
        }
    }
    if let Some(d) = decider.filter(|d| rand::random::<f32>() < d.share) {
        counters.llm_calls += 1;
        let start = Instant::now();
        match decided::decision_step(&d.s1, state, snap, &options).await {
            Ok((option, reason)) => {
                tracing::info!(target: "strategist", option = %option.id, ms = start.elapsed().as_millis() as u64, "decision step");
                return Ok((option, reason, true));
            }
            Err(error) => {
                counters.llm_invalid += 1;
                tracing::warn!(target: "strategist", error = %format!("{error:#}"), "decision step failed, using the fallback");
            }
        }
    }
    let o = fallback(snap, &options, state).ok_or_else(|| anyhow!("no option"))?;
    let reason = format!("fallback: {}", o.line);
    Ok((o, reason, false))
}

fn commit_definition(options: &[Opt]) -> ToolDefinition {
    commit_tool(Some(options))
}

/// The commit tool; with `None`, the option is any string (checked by the tool), so the definition is the same every
/// turn. History mode needs that: the tools are part of the prompt, and a definition that changes every turn makes
/// llama.cpp read the whole conversation again (2 s a turn at 3k tokens, 12 s at 57k).
fn commit_tool(options: Option<&[Opt]>) -> ToolDefinition {
    let option = match options {
        Some(options) => {
            json!({"type": "string", "enum": options.iter().map(|o| o.id.as_str()).collect::<Vec<_>>()})
        }
        None => json!({"type": "string", "description": "One option id from OPTIONS"}),
    };
    ToolDefinition::new(
        "commit",
        "Commit the next goal: first your analysis of the options, then one option id, then the changes to your STATE.",
        // Keys are generated in alphabetical order, so `analysis` comes before `option`: the model compares the
        // options before it names one, instead of justifying the first option it named.
        json!({
            "type": "object",
            "properties": {
                "analysis": {"type": "string", "maxLength": 300, "description": "Compare the options against the guidelines in one or two sentences"},
                "option": option,
                "state_patch": StrategicState::patch_schema()
            },
            "required": ["analysis", "option", "state_patch"]
        }),
    )
}

/// `monster_info` and `map_info`, answering in a few short lines.
fn lookup_tools(knowledge: &Knowledge, snap: &Snapshot) -> ToolRegistry {
    let k = knowledge.clone();
    let obs = snap.obs.clone();
    let monster = Tool::new(
        ToolDefinition::new(
            "monster_info",
            "Facts about a monster: level, HP, experience, aggressiveness, and how a fight would go for you.",
            json!({"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}),
        ),
        move |args: Value| {
            let k = k.clone();
            let obs = obs.clone();
            Box::pin(async move {
                let name = args["name"].as_str().unwrap_or_default().to_owned();
                let text = monster_info(&k, &obs, &name)
                    .await
                    .unwrap_or_else(|e| format!("no data: {e}"));
                Ok(Value::String(text))
            })
        },
    );
    let k = knowledge.clone();
    let map = Tool::new(
        ToolDefinition::new(
            "map_info",
            "Facts about a map: whether it is a town, its monsters, and the maps its warps lead to.",
            json!({"type": "object", "properties": {"map": {"type": "string"}}, "required": ["map"]}),
        ),
        move |args: Value| {
            let k = k.clone();
            Box::pin(async move {
                let name = args["map"].as_str().unwrap_or_default().to_owned();
                let text = map_info(&k, &name)
                    .await
                    .unwrap_or_else(|e| format!("no data: {e}"));
                Ok(Value::String(text))
            })
        },
    );
    let k = knowledge.clone();
    let wiki = Tool::new(
        ToolDefinition::new(
            "wiki_page",
            "An iRO Wiki Classic page, e.g. \"Swordman\", \"Novice\", \"Stats\", \"Prontera\".",
            json!({"type": "object", "properties": {"title": {"type": "string"}}, "required": ["title"]}),
        ),
        move |args: Value| {
            let k = k.clone();
            Box::pin(async move {
                let title = args["title"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .replace(' ', "_");
                let text = match k.db.wiki(&format!("{title}.md")).await {
                    Ok(page) => wiki_digest(&page),
                    Err(e) => format!("no page: {e}"),
                };
                Ok(Value::String(text))
            })
        },
    );
    let k = knowledge.clone();
    let quest = Tool::new(
        ToolDefinition::new(
            "quest_info",
            "Quests matching a name, with a step-by-step guide, e.g. \"Job Change to Swordman\".",
            json!({"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}),
        ),
        move |args: Value| {
            let k = k.clone();
            Box::pin(async move {
                let name = args["name"].as_str().unwrap_or_default().to_owned();
                let text = quest_info(&k, &name)
                    .await
                    .unwrap_or_else(|e| format!("no data: {e}"));
                Ok(Value::String(text))
            })
        },
    );
    ToolRegistry::new()
        .with_tool(monster)
        .with_tool(map)
        .with_tool(wiki)
        .with_tool(quest)
}

/// The prose of a wiki page without links, images and tables, for a small context.
fn wiki_digest(page: &str) -> String {
    let mut out = String::new();
    for line in page.lines().map(str::trim) {
        if line.is_empty()
            || line.starts_with('|')
            || line.starts_with("![")
            || line.starts_with("---")
        {
            continue;
        }
        let mut text = String::new();
        let mut chars = line.chars().peekable();
        // Keep the text of [text](link), drop the link.
        while let Some(c) = chars.next() {
            match c {
                '[' => {}
                ']' if chars.peek() == Some(&'(') => {
                    for c in chars.by_ref() {
                        if c == ')' {
                            break;
                        }
                    }
                }
                _ => text.push(c),
            }
        }
        out.push_str(text.trim_start_matches('#').trim());
        out.push(' ');
    }
    out
}

/// The guide part of the quests matching a name: who to talk to, where, and what they ask for.
async fn quest_info(k: &Knowledge, name: &str) -> Result<String> {
    let hits = k.db.search("quests", name, QUEST_HITS).await?;
    if hits.is_empty() {
        bail!("no quest matches {name:?}");
    }
    let mut out = Vec::new();
    for hit in hits {
        let Some(id) = hit.id_u32() else { continue };
        let q = k.db.get(&format!("/db/quests/{id}")).await?;
        // `notes` is the same explanation of the data on every quest.
        let keep = [
            "title",
            "description",
            "guide",
            "prerequisites",
            "recommended_level",
            "sources",
            "objectives",
        ];
        let fields: serde_json::Map<String, Value> = keep
            .iter()
            .filter_map(|key| {
                q.get(*key)
                    .filter(|v| !v.is_null())
                    .map(|v| ((*key).to_owned(), v.clone()))
            })
            .collect();
        out.push(Value::Object(fields));
    }
    Ok(serde_json::to_string(&out)?)
}

async fn monster_info(k: &Knowledge, obs: &Observation, name: &str) -> Result<String> {
    let hits = k.db.search("mobs", name, 1).await?;
    let id = hits
        .first()
        .and_then(|l| l.id_u32())
        .ok_or_else(|| anyhow!("unknown monster {name}"))?;
    let m = k.db.mob(id).await?;
    let a = k.fighter(obs).await?.assess(&m);
    let drops: Vec<String> = m
        .drops
        .iter()
        .filter(|d| !d.item.name.is_empty())
        .take(4)
        .map(|d| d.item.name.clone())
        .collect();
    Ok(format!(
        "{}: level {}, HP {}, base exp {}, job exp {}, {}{}. A fight would take about {:.0} s and cost about {:.0}% of your HP. Drops: {}.",
        m.name_english,
        m.level,
        m.hp,
        m.exp,
        m.job_exp,
        if m.aggressive() {
            "attacks on sight"
        } else {
            "passive"
        },
        if m.boss() { ", BOSS" } else { "" },
        a.kill_secs,
        a.danger * 100.0,
        drops.join(", ")
    ))
}

async fn map_info(k: &Knowledge, name: &str) -> Result<String> {
    let m = k.db.map(name).await?;
    let mut mobs = Vec::new();
    for s in m.mob_spawns.iter().take(8) {
        mobs.push(format!("{} x{}", s.mob.name, s.amount));
    }
    let links: Vec<String> = m
        .connected_maps
        .iter()
        .take(8)
        .map(|l| l.id_str())
        .collect();
    Ok(format!(
        "{}: {}. Monsters: {}. Warps to: {}.",
        m.name,
        if m.is_town() { "town" } else { "not a town" },
        if mobs.is_empty() {
            "none".into()
        } else {
            mobs.join(", ")
        },
        links.join(", ")
    ))
}

/// One SKILL.state step with the LLM.
pub async fn llm_step(step: Step<'_>, state: &StrategicState) -> Result<LlmDecision> {
    let options = shuffled(step.llm, step.options);
    let user = prompt::user_message(step.llm.order, state, step.snap, &options);
    llm_turn(
        Step {
            options: &options,
            ..step
        },
        user,
    )
    .await
}

/// The options in random order when the LLM shuffles them: the model favours the first one it reads.
pub fn shuffled(llm: &Llm, options: &[Opt]) -> Vec<Opt> {
    use rand::seq::SliceRandom;
    let mut options = options.to_vec();
    if llm.shuffle {
        options.shuffle(&mut rand::rng());
    }
    options
}

/// One step on a ready user message. `snap` only serves the lookups (the fight estimates of `monster_info`).
pub async fn llm_turn(step: Step<'_>, user: String) -> Result<LlmDecision> {
    let Step {
        llm,
        knowledge,
        snap,
        options,
    } = step;
    let start = Instant::now();
    let system = if llm.examples {
        format!("{}{}", prompt::SYSTEM, examples::text())
    } else {
        prompt::SYSTEM.to_owned()
    };
    let cost = Cost {
        start,
        prompt_chars: system.len() + user.len(),
    };
    let lookups = lookup_tools(knowledge, snap);
    let commit_only = ToolRegistry::new().with_tool(Tool::new(commit_definition(options), |_| {
        Box::pin(async { Ok(Value::Null) })
    }));
    let mut all = lookup_tools(knowledge, snap);
    all.insert(Tool::new(commit_definition(options), |_| {
        Box::pin(async { Ok(Value::Null) })
    }));

    let mut session = ChatSession::with_system_prompt(system);
    session.push_user(user);
    let mut done_lookups = Vec::new();
    let mut force = false;
    for round in 0..=LOOKUP_ROUNDS {
        let last = round == LOOKUP_ROUNDS || std::mem::take(&mut force);
        let (provider, registry) = if last {
            (&llm.forced, &commit_only)
        } else {
            (&llm.free, &all)
        };
        let response =
            tokio::time::timeout(LLM_TIMEOUT, session.complete_with_tools(provider, registry))
                .await
                .map_err(|_| anyhow!("LLM timed out"))?
                .map_err(|e| anyhow!("LLM request failed: {e}"))?;
        let calls = &response.message.tool_calls;
        if let Some(commit) = calls.iter().find(|c| c.function.name == "commit") {
            let args: Value = llm::parse_json_loose(&commit.function.arguments)
                .ok_or_else(|| anyhow!("commit arguments are not JSON"))?;
            let id = args["option"].as_str().unwrap_or_default();
            // Without grammar the model sometimes names an option that is not offered: say so and let it retry.
            if !last && !options.iter().any(|o| o.id == id) {
                let ids: Vec<&str> = options.iter().map(|o| o.id.as_str()).collect();
                let error = format!(
                    "error: {id:?} is not an option; choose one of {}",
                    ids.join(", ")
                );
                session.push(ChatMessage::tool_result(&commit.id, "commit", error));
                continue;
            }
            return finish(args, options, done_lookups, cost);
        }
        if calls.is_empty() {
            // Plain text: accept a JSON answer with the same fields, else ask again with the commit tool forced.
            if let Some(args) = llm::parse_json_loose(&response.message.content.text())
                && args.get("option").is_some()
            {
                return finish(args, options, done_lookups, cost);
            }
            if last {
                break;
            }
            session.push_user(COMMIT_NUDGE);
            force = true;
            continue;
        }
        // The model tends to fire many lookups at once; run the first few, answer the rest with a refusal
        // (every call needs a result message).
        for (i, c) in calls.iter().enumerate() {
            if i < LOOKUPS_PER_ROUND {
                done_lookups.push(format!("{}({})", c.function.name, c.function.arguments));
                let result = lookups.execute_call_reporting_errors(c).await;
                session.push(result.into_message());
            } else {
                session.push(ChatMessage::tool_result(
                    &c.id,
                    &c.function.name,
                    &format!("skipped: at most {LOOKUPS_PER_ROUND} lookups per turn"),
                ));
            }
        }
    }
    bail!("the model did not commit")
}

fn finish(args: Value, options: &[Opt], lookups: Vec<String>, cost: Cost) -> Result<LlmDecision> {
    let id = args["option"].as_str().unwrap_or_default();
    let option = options
        .iter()
        .find(|o| o.id == id)
        .cloned()
        .ok_or_else(|| anyhow!("unknown option {id:?}"))?;
    Ok(LlmDecision {
        option,
        reason: args["analysis"]
            .as_str()
            .or(args["reason"].as_str())
            .unwrap_or_default()
            .chars()
            .take(300)
            .collect(),
        patch: args.get("state_patch").cloned().unwrap_or(Value::Null),
        lookups,
        prompt_chars: cost.prompt_chars,
        millis: cost.start.elapsed().as_millis(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wiki_digest_keeps_prose_only() {
        let page = "# Swordman\n\n![img](a.png)\n| a | b |\nThe [Swordman](Swordman.md) is a melee class.\n";
        assert_eq!(
            wiki_digest(page).trim(),
            "Swordman The Swordman is a melee class."
        );
    }

    #[test]
    fn commit_schema_enumerates_options() {
        let options = vec![options::Opt {
            id: "rest".into(),
            goal: Goal::Rest,
            line: String::new(),
            score: 0.0,
        }];
        let def = commit_definition(&options);
        assert_eq!(
            def.function.parameters["properties"]["option"]["enum"],
            json!(["rest"])
        );
    }
}
