//! The main loop: observe, react, follow the strategist's goal through GOAP one step at a time, report back.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::api::{Action, ApiError, Observation, dist};
use crate::bot::{Bot, Outcome};
use crate::config::RunArgs;
use crate::goap::state::{Extra, is_loot_item};
use crate::goap::{Goal, PlanContext, Step, WorldState, plan};
use crate::memory::Counters;
use crate::social::help::HelpOutcome;
use crate::strategist::options::JOB_MAP;
use crate::strategist::{Event, Handle, Snapshot};

/// Step errors in a row after which the goal is given up.
const MAX_ERRORS_IN_ROW: u32 = 5;
/// A failed goal is not offered again for this long.
const FAILURE_MEMORY: Duration = Duration::from_secs(300);
/// Failed steps in a row before the goal is given up.
const MAX_STEP_FAILURES: u32 = 3;
const SAVE_EVERY: Duration = Duration::from_secs(30);

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
    [
        "no monsters",
        "nothing safe",
        "no known route",
        "no Job Master",
        "no potion shop",
        "does not sell",
        "buys none",
        "cannot afford",
        // Dying on the way says the way is too hard now: the same trip would end the same
        "died on the way",
        "too many warps",
        "other warps",
    ]
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
    llm_base: Counters,
    /// The last favour that ended: a decision made before it ended may still name it.
    ended_favour: Option<Goal>,
    /// The `ask:<id>` option of the request taken up last.
    taken_ask: String,
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
        prev: None,
        last_save: Instant::now(),
        ended_favour: None,
        taken_ask: String::new(),
        llm_base: bot.memory.counters.clone(),
    };
    let started = Instant::now();
    bot.memory.visited_maps.insert(first.map.name.clone());
    bot.social.start(&first);
    bot.talk_seq.store(
        first.last_message_seq(),
        std::sync::atomic::Ordering::SeqCst,
    );

    let result = loop {
        if stop.load(Ordering::SeqCst) {
            break Ok(());
        }
        if args
            .minutes
            .is_some_and(|m| started.elapsed() >= Duration::from_secs(m * 60))
        {
            tracing::info!("time is up");
            break Ok(());
        }
        let obs = match bot.observe().await {
            Ok(obs) => obs,
            Err(error) => {
                tracing::warn!(%error, "observation failed");
                if ApiError::status_of(&error)
                    .is_some_and(|s| s.as_u16() == 409 || s.as_u16() == 404)
                {
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
                    give_up(
                        &mut bot,
                        &handle,
                        &mut r,
                        &goal,
                        &format!("keeps failing: {error}"),
                    );
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
    // A bot left in the game holds one of the server's `max_bots` places for 20 more minutes: 50 bots done with a run
    // kept the next ones out
    if let Err(error) = bot.api.disconnect(&bot.name).await {
        tracing::warn!(%error, "could not leave the game");
    }
    result
}

fn detect_events(bot: &mut Bot, handle: &Handle, r: &mut Run, obs: &Observation) {
    let Some(prev) = &r.prev else { return };
    if obs.me.base_level > prev.me.base_level {
        bot.memory.note(format!(
            "reached base level {} on {}",
            obs.me.base_level, obs.map.name
        ));
        handle.events.send(Event::LevelUp).ok();
    }
    if obs.me.job_level > prev.me.job_level {
        bot.memory
            .note(format!("reached job level {}", obs.me.job_level));
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
    if bot.gear_reflexes(&obs).await? {
        return Ok(());
    }
    let doing = r
        .goal
        .as_ref()
        .map(|g| g.label())
        .unwrap_or_else(|| "looking around".into());
    if bot.converse(&obs, &doing).await {
        // The strategist decides with the request in its snapshot
        let ws = world_state(bot, &obs);
        publish(bot, handle, r, &obs, &ws);
        handle.events.send(Event::Asked).ok();
    }
    bot.decline_stale(&obs, r.goal.as_ref()).await;
    if bot.party_reflexes(&obs, r.goal.as_ref()).await? {
        return Ok(());
    }

    bot.keep = crate::quests::kept_items(&bot.memory.quests);
    // Selling is offered once a potion shop is known, and the shop was looked up only for a trip to it, which buying
    // potions starts with zeny a new character has none of: 45 of 50 new bots ended an hour with their loot unsold
    if bot.memory.potion_shop.is_none()
        && !obs.map.name.starts_with("new_")
        && bot.shop_checked_on.as_deref() != Some(obs.map.name.as_str())
    {
        bot.potion_shop(&obs.map.name).await.ok();
    }
    let mut ws = world_state(bot, &obs);
    // A Novice without a job holds its points: the strategist picks the job first, and its build decides where they go
    let awaits_job = obs.me.job.eq_ignore_ascii_case("novice")
        && bot.memory.strategy.job_target.is_none()
        && bot.memory.strategy.pinned_jobs.is_empty();
    if ws.status_points && !awaits_job {
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
        bot.answer_requests(&obs, &d.asked, &d.option, Some(&d.goal), &d.refusals)
            .await;
        // The same favour asked again is a new one; a later directive for the same request is not
        if d.option.starts_with("ask:") && d.option != r.taken_ask {
            r.ended_favour = None;
            r.taken_ask = d.option.clone();
        }
        // A quest is taken up with its first goal, even one that only buys or hunts what it asks for: the book holds
        // its stage, and without an entry the goal of a later stage reads as done before it starts
        if let Some(id) = d.option.strip_prefix("quest:")
            && !bot.memory.quests.contains_key(id)
        {
            bot.memory.quests.insert(id.to_owned(), Default::default());
            bot.keep = crate::quests::kept_items(&bot.memory.quests);
        }
        if r.ended_favour.as_ref() == Some(&d.goal) {
            tracing::debug!(goal = %d.goal.label(), "a favour that is over, not taken up again");
        } else if r
            .failures
            .iter()
            .any(|(g, at)| *g == d.goal && at.elapsed() < FAILURE_MEMORY)
        {
            // Decided before the goal failed
            tracing::debug!(goal = %d.goal.label(), "a goal given up lately, not taken up again");
        } else if r.goal.as_ref() != Some(&d.goal) {
            tracing::info!(goal = %d.goal.label(), reason = %d.reason, by_llm = d.by_llm, "new goal");
            r.goal = Some(d.goal);
            r.goal_since = Instant::now();
            r.step_failures = 0;
        }
    }
    publish(bot, handle, r, &obs, &ws);
    // A favour asked in chat runs by itself, not planned.
    if let Some(ref goal @ Goal::Help { ref who, task }) = r.goal.clone() {
        let event = match bot.help(&obs, who, task, r.goal_since).await? {
            HelpOutcome::Going => return Ok(()),
            HelpOutcome::Finished => {
                tracing::info!(goal = %goal.label(), "favour done");
                r.last_outcome = format!("{} done", goal.label());
                Event::GoalDone(goal.label())
            }
            HelpOutcome::Failed(reason) => {
                tracing::info!(goal = %goal.label(), %reason, "favour given up");
                bot.memory
                    .note(format!("could not {}: {reason}", goal.label()));
                r.last_outcome = format!("{} failed: {reason}", goal.label());
                Event::GoalFailed(format!("{}: {reason}", goal.label()))
            }
        };
        // The strategist must not see the favour as current any more, or it keeps it
        r.goal = None;
        r.ended_favour = Some(goal.clone());
        let obs = bot.observe().await?;
        let ws = world_state(bot, &obs);
        publish(bot, handle, r, &obs, &ws);
        handle.events.send(event).ok();
        return Ok(());
    }
    // In a party, following the leader replaces the hunt; errands still run as planned.
    if let Some(outcome) = bot.party_step(&obs, r.goal.as_ref()).await? {
        r.last_outcome = match outcome {
            Outcome::Failed(reason) => format!("with the party: {reason}"),
            _ => "with the party".into(),
        };
        return Ok(());
    }
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
    let ctx = PlanContext {
        shop_map,
        job_map: Some(JOB_MAP.to_owned()),
        hops: Vec::new(),
    };
    bot.travel_spot = goal.spot();
    bot.exploring = match &goal {
        Goal::Explore { map } => Some(map.clone()),
        _ => None,
    };
    ws.hunted = false;
    let Some(steps) = plan(&ws, &goal, &ctx) else {
        give_up(bot, handle, r, &goal, "no plan reaches it");
        return Ok(());
    };
    let Some(step) = steps.first().cloned() else {
        return Ok(());
    };
    // A leader does not pull more while a member in sight is low on HP and nothing attacks it: it gets its HP back.
    if matches!(step, Step::Hunt { .. })
        && bot.member_in_trouble(&obs)
        && crate::party::attacking_a_member(
            &obs,
            &crate::party::member_ids(&obs),
            crate::party::SIGHT,
        )
        .is_none()
    {
        Bot::pause(1500).await;
        return Ok(());
    }
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
        Outcome::Failed(reason)
            if matches!(step, Step::Hunt { .. })
                && !hopeless(&reason)
                && !reason.contains("died") =>
        {
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
    bot.memory
        .note(format!("gave up '{}': {reason}", goal.label()));
    r.failures.push((goal.clone(), Instant::now()));
    r.goal = None;
    r.step_failures = 0;
    // The strategist answers the event at once, maybe before the next snapshot: it must not choose the goal again.
    handle.snapshots.send_modify(|snap| {
        if let Some(snap) = snap {
            snap.recent_failures.push(goal.clone());
            snap.goal = None;
        }
    });
    handle
        .events
        .send(Event::GoalFailed(format!("{}: {reason}", goal.label())))
        .ok();
}

/// How far from its shop spot the bot still stands at the shop.
const AT_SHOP: u32 = 6;

fn world_state(bot: &Bot, obs: &Observation) -> WorldState {
    // Only where the shop is: following the leader takes the bot away without a step that clears the flag, and every
    // sale after failed at once without a walk back (88 times for 4 bots in an hour)
    let at_shop = bot
        .memory
        .potion_shop
        .as_ref()
        .is_some_and(|s| s.map == obs.map.name && dist(obs.pos(), (s.x, s.y)) <= AT_SHOP);
    let extra = Extra {
        near_shop: bot.near_shop && at_shop,
        potion_price: bot
            .memory
            .potion_shop
            .as_ref()
            .map(|s| s.price)
            .unwrap_or(50),
        heal_hp: bot.knowledge.heal_stock(obs),
        keep: bot.keep.clone(),
        quest_stages: crate::quests::stages(&bot.memory.quests, obs),
    };
    let mut ws = WorldState::from_observation(obs, &extra);
    if bot.stats_stuck_at == Some(obs.me.status_points) {
        ws.status_points = false;
    }
    if bot
        .skills_stuck_at
        .as_ref()
        .is_some_and(|(job, lvl)| *job == obs.me.job && *lvl == obs.me.job_level)
    {
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
        loot_items: obs
            .inventory
            .iter()
            .filter(|i| is_loot_item(i, &bot.keep))
            .map(|i| i.amount)
            .sum(),
        potion_price: bot
            .memory
            .potion_shop
            .as_ref()
            .map(|s| s.price)
            .unwrap_or(0),
        kills: bot.memory.counters.kills,
        deaths: bot.memory.counters.deaths,
        kills_by_mob: sorted(&bot.memory.kills_by_mob),
        deaths_by_cause: sorted(&bot.memory.deaths_by_cause),
        journal: bot.memory.journal.clone(),
        visited: bot.memory.visited_maps.iter().cloned().collect(),
        recent_failures: r.failures.iter().map(|(g, _)| g.clone()).collect(),
        bad_shops: bot.memory.bad_shops.clone(),
        losing_to: bot.memory.losing_to(obs.me.base_level),
        quests: bot.memory.quests.clone(),
        requests: bot.social.pending.clone(),
        party_play: bot.party.is_some(),
        friends: bot
            .party
            .as_ref()
            .map(|p| p.config.friends.clone())
            .unwrap_or_default(),
        notes: bot.memory.strategy.notes.clone(),
        job_target: bot.memory.strategy.job_target.clone(),
        can_heal: bot.can_heal(obs),
        deadly_ways: bot.recent_deadly_ways(),
    };
    handle.snapshots.send_replace(Some(snap));
}

async fn print_report(bot: &Bot, r: &Run, started: Instant) {
    let obs = bot.observe().await.ok();
    let c = &bot.memory.counters;
    println!(
        "\n== run report ({} min) ==",
        started.elapsed().as_secs() / 60
    );
    if let Some(obs) = &obs {
        println!(
            "{} the {}: base level {} -> {}, job level {} -> {}, zeny {}, on {}",
            obs.me.name,
            obs.me.job,
            r.start_level,
            obs.me.base_level,
            r.start_job_level,
            obs.me.job_level,
            obs.me.zeny,
            obs.map.name
        );
    }
    println!(
        "kills {} | deaths {} | loot picked {} | zeny from sales {}",
        c.kills, c.deaths, c.loot_picked, c.zeny_earned
    );
    let avg = if c.llm_calls > 0 {
        c.llm_prompt_chars / c.llm_calls
    } else {
        0
    };
    println!(
        "LLM steps {} | failed or partly invalid {} | avg prompt {} chars",
        c.llm_calls, c.llm_invalid, avg
    );
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
        assert!(hopeless(
            "nothing safe to fight on prt_fild02 (40 monsters)"
        ));
        assert!(!hopeless("fight with Poring took too long"));
    }
}
