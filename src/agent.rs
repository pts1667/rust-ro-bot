//! The main loop: observe, react, follow the strategist's goal through GOAP one step at a time, report back.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::api::{Action, ApiError, Observation};
use crate::bot::{Bot, Outcome};
use crate::config::RunArgs;
use crate::goap::state::{Extra, is_loot};
use crate::goap::{Goal, PlanContext, Step, WorldState, plan};
use crate::memory::Counters;
use crate::strategist::options::JOB_MAP;
use crate::strategist::{Event, Handle, Snapshot};

/// Step errors in a row after which the goal is given up.
const MAX_ERRORS_IN_ROW: u32 = 5;
/// A failed goal is not offered again for this long.
const FAILURE_MEMORY: Duration = Duration::from_secs(300);
/// Failed steps in a row before the goal is given up.
const MAX_STEP_FAILURES: u32 = 3;
const SAVE_EVERY: Duration = Duration::from_secs(30);
const CHAT_COOLDOWN: Duration = Duration::from_secs(20);

/// Carrying capacity in the observation's weight unit (tenths), pre-renewal: job base + 300 per STR.
pub fn max_weight(job: &str, str: u32) -> u32 {
    let base = match job.to_lowercase().as_str() {
        "swordman" | "swordsman" | "merchant" => 28000,
        "archer" => 26000,
        "acolyte" | "thief" => 24000,
        "mage" | "magician" => 22000,
        _ => 20000,
    };
    base + 300 * str
}

/// Step failures after which retrying the goal is pointless.
fn hopeless(reason: &str) -> bool {
    ["no monsters", "nothing safe", "no known route", "no Job Master", "no potion shop", "does not sell", "buys none", "cannot afford"]
        .iter()
        .any(|k| reason.contains(k))
}

struct Run {
    goal: Option<Goal>,
    goal_since: Instant,
    applied_seq: u64,
    last_outcome: String,
    last_step: Option<Step>,
    failures: Vec<(Goal, Instant)>,
    step_failures: u32,
    /// Step errors in a row.
    errors_in_row: u32,
    start_level: u32,
    start_job_level: u32,
    prev: Option<Observation>,
    last_save: Instant,
    last_chat_seq: u64,
    last_reply: Option<Instant>,
    llm_base: Counters,
}

pub async fn run(mut bot: Bot, mut handle: Handle, args: RunArgs) -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                tracing::info!("Ctrl-C: stopping after the current step");
                stop.store(true, Ordering::SeqCst);
            }
        });
    }
    let first = bot.observe().await?;
    let mut r = Run {
        goal: None,
        goal_since: Instant::now(),
        applied_seq: 0,
        last_outcome: "just started".into(),
        last_step: None,
        failures: Vec::new(),
        step_failures: 0,
        errors_in_row: 0,
        start_level: first.me.base_level,
        start_job_level: first.me.job_level,
        last_chat_seq: first.last_message_seq(),
        prev: None,
        last_save: Instant::now(),
        last_reply: None,
        llm_base: bot.memory.counters.clone(),
    };
    let started = Instant::now();
    bot.memory.visited_maps.insert(first.map.name.clone());

    let result = loop {
        if stop.load(Ordering::SeqCst) {
            break Ok(());
        }
        if args.minutes.is_some_and(|m| started.elapsed() >= Duration::from_secs(m * 60)) {
            tracing::info!("time is up");
            break Ok(());
        }
        let obs = match bot.observe().await {
            Ok(obs) => obs,
            Err(error) => {
                tracing::warn!(%error, "observation failed");
                if ApiError::status_of(&error).is_some_and(|s| s.as_u16() == 409 || s.as_u16() == 404) {
                    crate::ensure_connected(&bot.api, &bot.name).await.ok();
                }
                Bot::pause(2000).await;
                continue;
            }
        };
        if !obs.ready {
            Bot::pause(500).await;
            continue;
        }
        if args.until_level.is_some_and(|l| obs.me.base_level >= l) {
            tracing::info!(level = obs.me.base_level, "target level reached");
            break Ok(());
        }
        detect_events(&mut bot, &handle, &mut r, &obs);
        r.prev = Some(obs.clone());

        match tick(&mut bot, &mut handle, &mut r, obs).await {
            Ok(()) => r.errors_in_row = 0,
            Err(error) => {
                tracing::warn!(error = format!("{error:#}"), "step error");
                r.last_outcome = format!("error: {error}");
                // An error that repeats (a refused action the plan keeps choosing) ends the goal, or it loops forever.
                r.errors_in_row += 1;
                if r.errors_in_row >= MAX_ERRORS_IN_ROW
                    && let Some(goal) = r.goal.clone()
                {
                    r.errors_in_row = 0;
                    give_up(&mut bot, &handle, &mut r, &goal, &format!("keeps failing: {error}"));
                }
                Bot::pause(1500).await;
            }
        }
        if r.last_save.elapsed() >= SAVE_EVERY {
            r.last_save = Instant::now();
            if let Err(error) = bot.memory.save() {
                tracing::warn!(%error, "saving memory failed");
            }
        }
    };

    bot.act(Action::Stop).await.ok();
    bot.memory.save()?;
    print_report(&bot, &r, started).await;
    result
}

fn detect_events(bot: &mut Bot, handle: &Handle, r: &mut Run, obs: &Observation) {
    let Some(prev) = &r.prev else { return };
    if obs.me.base_level > prev.me.base_level {
        bot.memory.note(format!("reached base level {} on {}", obs.me.base_level, obs.map.name));
        handle.events.send(Event::LevelUp).ok();
    }
    if obs.me.job_level > prev.me.job_level {
        bot.memory.note(format!("reached job level {}", obs.me.job_level));
        handle.events.send(Event::JobLevelUp).ok();
    }
    if obs.map.name != prev.map.name {
        handle.events.send(Event::MapChanged).ok();
    }
    if obs.me.dead && !prev.me.dead {
        // Counted here so deaths on the way somewhere count too, not only those in a fight.
        bot.memory.counters.deaths += 1;
        handle.events.send(Event::Died).ok();
    }
}

async fn tick(bot: &mut Bot, handle: &mut Handle, r: &mut Run, obs: Observation) -> Result<()> {
    bot.knowledge.learn_heals(&obs).await;
    // Reflexes come before any goal.
    if obs.me.dead {
        tracing::info!("dead: respawning");
        bot.act(Action::Respawn).await?;
        r.last_outcome = "died and respawned at the save point".into();
        Bot::pause(1500).await;
        return Ok(());
    }
    if obs.dialog.as_ref().is_some_and(|d| d.prompt.is_some()) {
        bot.act(Action::DialogClose).await.ok();
    }
    if obs.me.hp_frac() < 0.25
        && obs.me.action == "attacking"
        && let Some(heal) = bot.knowledge.heal_action(&obs)
    {
        bot.act(heal.action).await.ok();
    }
    reply_to_chat(bot, r, &obs).await;

    let mut ws = world_state(bot, &obs);
    if ws.status_points {
        let outcome = bot.raise_stats().await?;
        tracing::info!(?outcome, "spent status points");
        return Ok(());
    }
    if ws.skill_points {
        let outcome = bot.learn_skills().await?;
        tracing::info!(?outcome, "spent skill points");
        return Ok(());
    }

    // The strategist's latest goal.
    let directive = handle.directives.borrow().clone();
    if let Some(d) = directive
        && d.seq > r.applied_seq
    {
        r.applied_seq = d.seq;
        bot.current_directive = d.seq;
        bot.memory.strategy = d.state.clone();
        let llm = &d.counters;
        bot.memory.counters.llm_calls = r.llm_base.llm_calls + llm.llm_calls;
        bot.memory.counters.llm_invalid = r.llm_base.llm_invalid + llm.llm_invalid;
        bot.memory.counters.llm_prompt_chars = r.llm_base.llm_prompt_chars + llm.llm_prompt_chars;
        if r.goal.as_ref() != Some(&d.goal) {
            tracing::info!(goal = %d.goal.label(), reason = %d.reason, by_llm = d.by_llm, "new goal");
            r.goal = Some(d.goal);
            r.goal_since = Instant::now();
            r.step_failures = 0;
        }
    }
    publish(bot, handle, r, &obs, &ws);
    let Some(goal) = r.goal.clone() else {
        Bot::pause(500).await;
        return Ok(());
    };

    if goal.satisfied(&ws) {
        tracing::info!(goal = %goal.label(), "goal reached");
        r.last_outcome = format!("{} done", goal.label());
        r.goal = None;
        handle.events.send(Event::GoalDone(goal.label())).ok();
        return Ok(());
    }
    let shop_map = if matches!(goal, Goal::SellLoot | Goal::BuyPotions) && !ws.in_training {
        bot.potion_shop(&obs.map.name).await?.map(|s| s.map)
    } else {
        bot.memory.potion_shop.as_ref().map(|s| s.map.clone())
    };
    let ctx = PlanContext { shop_map, job_map: Some(JOB_MAP.to_owned()), hops: Vec::new() };
    ws.hunted = false;
    let Some(steps) = plan(&ws, &goal, &ctx) else {
        give_up(bot, handle, r, &goal, "no plan reaches it");
        return Ok(());
    };
    let Some(step) = steps.first().cloned() else { return Ok(()) };
    if r.last_step.as_ref() != Some(&step) {
        tracing::info!(goal = %goal.label(), plan = ?steps, "plan");
    }
    r.last_step = Some(step.clone());

    match bot.execute(&step).await? {
        Outcome::Done => {
            r.step_failures = 0;
            r.last_outcome = format!("{step:?} done");
        }
        Outcome::Preempted => {
            tracing::info!(?step, "step preempted by a new goal");
            r.last_outcome = format!("{step:?} interrupted for a new goal");
        }
        Outcome::Failed(reason) if matches!(step, Step::Hunt { .. }) && !hopeless(&reason) && !reason.contains("died") => {
            // One target that runs off or sits out of reach says nothing about the map: it is blacklisted, go on.
            tracing::info!(%reason, "target skipped");
            r.last_outcome = format!("target skipped: {reason}");
        }
        Outcome::Failed(reason) => {
            r.step_failures += 1;
            tracing::info!(?step, %reason, "step failed");
            r.last_outcome = format!("{step:?} failed: {reason}");
            if hopeless(&reason) || r.step_failures >= MAX_STEP_FAILURES {
                give_up(bot, handle, r, &goal, &reason);
            }
        }
    }
    let obs = bot.observe().await?;
    let ws = world_state(bot, &obs);
    publish(bot, handle, r, &obs, &ws);
    Ok(())
}

fn give_up(bot: &mut Bot, handle: &Handle, r: &mut Run, goal: &Goal, reason: &str) {
    tracing::info!(goal = %goal.label(), %reason, "goal given up");
    bot.memory.note(format!("gave up '{}': {reason}", goal.label()));
    r.failures.push((goal.clone(), Instant::now()));
    r.goal = None;
    r.step_failures = 0;
    handle.events.send(Event::GoalFailed(format!("{}: {reason}", goal.label()))).ok();
}

fn world_state(bot: &Bot, obs: &Observation) -> WorldState {
    let extra = Extra {
        near_shop: bot.near_shop,
        potion_price: bot.memory.potion_shop.as_ref().map(|s| s.price).unwrap_or(50),
        heal_hp: bot.knowledge.heal_stock(obs),
    };
    let mut ws = WorldState::from_observation(obs, &extra);
    if bot.stats_stuck_at == Some(obs.me.status_points) {
        ws.status_points = false;
    }
    if bot.skills_stuck_at.as_ref().is_some_and(|(job, lvl)| *job == obs.me.job && *lvl == obs.me.job_level) {
        ws.skill_points = false;
    }
    ws
}

fn publish(bot: &Bot, handle: &Handle, r: &mut Run, obs: &Observation, ws: &WorldState) {
    r.failures.retain(|(_, at)| at.elapsed() < FAILURE_MEMORY);
    let sorted = |m: &std::collections::BTreeMap<String, u64>| {
        let mut v: Vec<(String, u64)> = m.iter().map(|(k, v)| (k.clone(), *v)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v
    };
    let snap = Snapshot {
        obs: obs.clone(),
        ws: ws.clone(),
        goal: r.goal.clone(),
        goal_secs: r.goal_since.elapsed().as_secs(),
        last_outcome: r.last_outcome.clone(),
        weight_frac: obs.me.weight as f32 / max_weight(&obs.me.job, obs.me.stats.str) as f32,
        heal_hp: bot.knowledge.heal_stock(obs),
        heal_items: bot.knowledge.heal_items(obs),
        loot_items: obs.inventory.iter().filter(|i| is_loot(&i.kind, i.equipped)).map(|i| i.amount).sum(),
        potion_price: bot.memory.potion_shop.as_ref().map(|s| s.price).unwrap_or(0),
        kills: bot.memory.counters.kills,
        deaths: bot.memory.counters.deaths,
        kills_by_mob: sorted(&bot.memory.kills_by_mob),
        deaths_by_cause: sorted(&bot.memory.deaths_by_cause),
        journal: bot.memory.journal.clone(),
        visited: bot.memory.visited_maps.iter().cloned().collect(),
        recent_failures: r.failures.iter().map(|(g, _)| g.clone()).collect(),
    };
    handle.snapshots.send_replace(Some(snap));
}

/// Answers what other players say around the bot, through the LLM helper, now and then.
async fn reply_to_chat(bot: &mut Bot, r: &mut Run, obs: &Observation) {
    let new: Vec<_> = obs.messages.iter().filter(|m| m.seq > r.last_chat_seq).cloned().collect();
    r.last_chat_seq = r.last_chat_seq.max(obs.last_message_seq());
    let Some(helper) = bot.helper.clone() else { return };
    if r.last_reply.is_some_and(|t| t.elapsed() < CHAT_COOLDOWN) {
        return;
    }
    let me = obs.me.name.clone();
    let Some((from, text)) = new.iter().filter(|m| m.channel == "chat" || m.channel == "whisper").find_map(|m| {
        let (from, text) = m.text.split_once(" : ")?;
        (from.trim() != me).then(|| (from.trim().to_owned(), text.trim().to_owned()))
    }) else {
        return;
    };
    let doing = r.goal.as_ref().map(|g| g.label()).unwrap_or_else(|| "thinking".into());
    match helper.chat_reply(&me, &from, &text, &doing).await {
        Ok(Some(reply)) => {
            r.last_reply = Some(Instant::now());
            tracing::info!(%from, %text, %reply, "chat");
            bot.act(Action::Say { text: reply }).await.ok();
        }
        Ok(None) => {}
        Err(error) => tracing::debug!(%error, "no chat reply"),
    }
}

async fn print_report(bot: &Bot, r: &Run, started: Instant) {
    let obs = bot.observe().await.ok();
    let c = &bot.memory.counters;
    println!("\n== run report ({} min) ==", started.elapsed().as_secs() / 60);
    if let Some(obs) = &obs {
        println!(
            "{} the {}: base level {} -> {}, job level {} -> {}, zeny {}, on {}",
            obs.me.name, obs.me.job, r.start_level, obs.me.base_level, r.start_job_level, obs.me.job_level, obs.me.zeny, obs.map.name
        );
    }
    println!("kills {} | deaths {} | loot picked {} | zeny from sales {}", c.kills, c.deaths, c.loot_picked, c.zeny_earned);
    let avg = if c.llm_calls > 0 { c.llm_prompt_chars / c.llm_calls } else { 0 };
    println!("LLM steps {} | failed or partly invalid {} | avg prompt {} chars", c.llm_calls, c.llm_invalid, avg);
    println!("objective: {}", bot.memory.strategy.objective);
    println!("notes: {:?}", bot.memory.strategy.notes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weight_grows_with_str() {
        assert_eq!(max_weight("Novice", 1), 20300);
        assert!(max_weight("Swordman", 10) > max_weight("Novice", 10));
    }

    #[test]
    fn hopeless_reasons_are_recognised() {
        assert!(hopeless("nothing safe to fight on prt_fild02 (40 monsters)"));
        assert!(!hopeless("fight with Poring took too long"));
    }
}
