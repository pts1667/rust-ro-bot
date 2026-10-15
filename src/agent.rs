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
use crate::social::Decision;
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

/// The bot, the strategist's handle and the state of the run: what the loop works on.
struct Agent {
    bot: Bot,
    handle: Handle,
    run: Run,
}

/// Plays until `stop` is set (Ctrl-C), the time is up or the level is reached.
pub async fn run(bot: Bot, handle: Handle, args: RunArgs, stop: Arc<AtomicBool>) -> Result<()> {
    let first = bot.observe().await?;
    let run = Run {
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
    let mut agent = Agent { bot, handle, run };
    agent.bot.memory.visited_maps.insert(first.map.name.clone());
    agent.bot.social.start(&first);
    agent.bot.talk_seq.store(
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
        let obs = match agent.bot.observe().await {
            Ok(obs) => obs,
            Err(error) => {
                tracing::warn!(%error, "observation failed");
                if ApiError::status_of(&error)
                    .is_some_and(|s| s.as_u16() == 409 || s.as_u16() == 404)
                {
                    crate::ensure_connected(&agent.bot.api, &agent.bot.name)
                        .await
                        .ok();
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
        agent.detect_events(&obs);
        agent.run.prev = Some(obs.clone());

        match agent.tick(obs).await {
            Ok(()) => agent.run.errors_in_row = 0,
            Err(error) => {
                tracing::warn!(error = format!("{error:#}"), "step error");
                agent.run.last_outcome = format!("error: {error}");
                // An error that repeats (a refused action the plan keeps choosing) ends the goal, or it loops forever.
                agent.run.errors_in_row += 1;
                if agent.run.errors_in_row >= MAX_ERRORS_IN_ROW
                    && let Some(goal) = agent.run.goal.clone()
                {
                    agent.run.errors_in_row = 0;
                    agent.give_up(&goal, &format!("keeps failing: {error}"));
                }
                Bot::pause(1500).await;
            }
        }
        if agent.run.last_save.elapsed() >= SAVE_EVERY {
            agent.run.last_save = Instant::now();
            if let Err(error) = agent.bot.memory.save() {
                tracing::warn!(%error, "saving memory failed");
            }
        }
    };

    agent.bot.act(Action::Stop).await.ok();
    agent.bot.memory.save()?;
    print_report(&agent.bot, &agent.run, started).await;
    // A bot left in the game holds one of the server's `max_bots` places for 20 more minutes: 50 bots done with a run
    // kept the next ones out
    if let Err(error) = agent.bot.api.disconnect(&agent.bot.name).await {
        tracing::warn!(%error, "could not leave the game");
    }
    result
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

impl Agent {
    fn detect_events(&mut self, obs: &Observation) {
        let Some(prev) = &self.run.prev else { return };
        if obs.me.base_level > prev.me.base_level {
            self.bot.memory.note(format!(
                "reached base level {} on {}",
                obs.me.base_level, obs.map.name
            ));
            self.handle.events.send(Event::LevelUp).ok();
        }
        if obs.me.job_level > prev.me.job_level {
            self.bot
                .memory
                .note(format!("reached job level {}", obs.me.job_level));
            self.handle.events.send(Event::JobLevelUp).ok();
        }
        if obs.map.name != prev.map.name {
            self.handle.events.send(Event::MapChanged).ok();
        }
        if obs.me.dead && !prev.me.dead {
            // Counted here so deaths on the way somewhere count too, not only those in a fight.
            self.bot.memory.counters.deaths += 1;
            self.handle.events.send(Event::Died).ok();
        }
    }

    async fn tick(&mut self, obs: Observation) -> Result<()> {
        self.bot.knowledge.learn_heals(&obs).await;
        // Reflexes come before any goal.
        if obs.me.dead {
            tracing::info!("dead: respawning");
            self.bot.act(Action::Respawn).await?;
            self.run.last_outcome = "died and respawned at the save point".into();
            Bot::pause(1500).await;
            return Ok(());
        }
        if obs.dialog.as_ref().is_some_and(|d| d.prompt.is_some()) {
            self.bot.act(Action::DialogClose).await.ok();
        }
        if obs.me.hp_frac() < 0.25
            && obs.me.action == "attacking"
            && let Some(heal) = self.bot.knowledge.heal_action(&obs)
        {
            self.bot.act(heal.action).await.ok();
        }
        if self.bot.gear_reflexes(&obs).await? {
            return Ok(());
        }
        let doing = self
            .run
            .goal
            .as_ref()
            .map(|g| g.label())
            .unwrap_or_else(|| "looking around".into());
        if self.bot.converse(&obs, &doing).await {
            // The strategist decides with the request in its snapshot
            let ws = world_state(&self.bot, &obs);
            self.publish(&obs, &ws);
            self.handle.events.send(Event::Asked).ok();
        }
        self.bot.decline_stale(&obs, self.run.goal.as_ref()).await;
        if self
            .bot
            .party_reflexes(&obs, self.run.goal.as_ref())
            .await?
        {
            return Ok(());
        }

        self.bot.keep = crate::quests::kept_items(&self.bot.memory.quests);
        // Selling is offered once a potion shop is known, and the shop was looked up only for a trip to it, which buying
        // potions starts with zeny a new character has none of: 45 of 50 new bots ended an hour with their loot unsold
        if self.bot.memory.potion_shop.is_none()
            && !obs.map.name.starts_with("new_")
            && self.bot.shop_checked_on.as_deref() != Some(obs.map.name.as_str())
        {
            self.bot.potion_shop(&obs.map.name).await.ok();
        }
        let mut ws = world_state(&self.bot, &obs);
        // A Novice without a job holds its points: the strategist picks the job first, and its build decides where they go
        let awaits_job = obs.me.job.eq_ignore_ascii_case("novice")
            && self.bot.memory.strategy.job_target.is_none()
            && self.bot.memory.strategy.pinned_jobs.is_empty();
        if ws.status_points && !awaits_job {
            let outcome = self.bot.raise_stats().await?;
            tracing::info!(?outcome, "spent status points");
            return Ok(());
        }
        if ws.skill_points {
            let outcome = self.bot.learn_skills().await?;
            tracing::info!(?outcome, "spent skill points");
            return Ok(());
        }

        // The strategist's latest goal.
        let directive = self.handle.directives.borrow().clone();
        if let Some(d) = directive
            && d.seq > self.run.applied_seq
        {
            self.run.applied_seq = d.seq;
            self.bot.current_directive = d.seq;
            self.bot.memory.strategy = d.state.clone();
            let llm = &d.counters;
            self.bot.memory.counters.llm_calls = self.run.llm_base.llm_calls + llm.llm_calls;
            self.bot.memory.counters.llm_invalid = self.run.llm_base.llm_invalid + llm.llm_invalid;
            self.bot.memory.counters.llm_prompt_chars =
                self.run.llm_base.llm_prompt_chars + llm.llm_prompt_chars;
            let decision = Decision {
                asked: &d.asked,
                chosen: &d.option,
                goal: Some(&d.goal),
                refusals: &d.refusals,
            };
            self.bot.answer_requests(&obs, decision).await;
            // The same favour asked again is a new one; a later directive for the same request is not
            if d.option.starts_with("ask:") && d.option != self.run.taken_ask {
                self.run.ended_favour = None;
                self.run.taken_ask = d.option.clone();
            }
            // A quest is taken up with its first goal, even one that only buys or hunts what it asks for: the book holds
            // its stage, and without an entry the goal of a later stage reads as done before it starts
            if let Some(id) = d.option.strip_prefix("quest:")
                && !self.bot.memory.quests.contains_key(id)
            {
                self.bot
                    .memory
                    .quests
                    .insert(id.to_owned(), Default::default());
                self.bot.keep = crate::quests::kept_items(&self.bot.memory.quests);
            }
            if self.run.ended_favour.as_ref() == Some(&d.goal) {
                tracing::debug!(goal = %d.goal.label(), "a favour that is over, not taken up again");
            } else if self
                .run
                .failures
                .iter()
                .any(|(g, at)| *g == d.goal && at.elapsed() < FAILURE_MEMORY)
            {
                // Decided before the goal failed
                tracing::debug!(goal = %d.goal.label(), "a goal given up lately, not taken up again");
            } else if self.run.goal.as_ref() != Some(&d.goal) {
                tracing::info!(goal = %d.goal.label(), reason = %d.reason, by_llm = d.by_llm, "new goal");
                self.run.goal = Some(d.goal);
                self.run.goal_since = Instant::now();
                self.run.step_failures = 0;
            }
        }
        self.publish(&obs, &ws);
        // A favour asked in chat runs by itself, not planned.
        if let Some(ref goal @ Goal::Help { ref who, task }) = self.run.goal.clone() {
            let event = match self.bot.help(&obs, who, task, self.run.goal_since).await? {
                HelpOutcome::Going => return Ok(()),
                HelpOutcome::Finished => {
                    tracing::info!(goal = %goal.label(), "favour done");
                    self.run.last_outcome = format!("{} done", goal.label());
                    Event::GoalDone(goal.label())
                }
                HelpOutcome::Failed(reason) => {
                    tracing::info!(goal = %goal.label(), %reason, "favour given up");
                    self.bot
                        .memory
                        .note(format!("could not {}: {reason}", goal.label()));
                    self.run.last_outcome = format!("{} failed: {reason}", goal.label());
                    Event::GoalFailed(format!("{}: {reason}", goal.label()))
                }
            };
            // The strategist must not see the favour as current any more, or it keeps it
            self.run.goal = None;
            self.run.ended_favour = Some(goal.clone());
            let obs = self.bot.observe().await?;
            let ws = world_state(&self.bot, &obs);
            self.publish(&obs, &ws);
            self.handle.events.send(event).ok();
            return Ok(());
        }
        // In a party, following the leader replaces the hunt; errands still run as planned.
        if let Some(outcome) = self.bot.party_step(&obs, self.run.goal.as_ref()).await? {
            self.run.last_outcome = match outcome {
                Outcome::Failed(reason) => format!("with the party: {reason}"),
                _ => "with the party".into(),
            };
            return Ok(());
        }
        let Some(goal) = self.run.goal.clone() else {
            Bot::pause(500).await;
            return Ok(());
        };

        if goal.satisfied(&ws) {
            tracing::info!(goal = %goal.label(), "goal reached");
            self.run.last_outcome = format!("{} done", goal.label());
            self.run.goal = None;
            self.handle.events.send(Event::GoalDone(goal.label())).ok();
            return Ok(());
        }
        let shop_map = if matches!(goal, Goal::SellLoot | Goal::BuyPotions) && !ws.in_training {
            self.bot.potion_shop(&obs.map.name).await?.map(|s| s.map)
        } else {
            self.bot.memory.potion_shop.as_ref().map(|s| s.map.clone())
        };
        let ctx = PlanContext {
            shop_map,
            job_map: Some(JOB_MAP.to_owned()),
            hops: Vec::new(),
        };
        self.bot.travel_spot = goal.spot();
        self.bot.exploring = match &goal {
            Goal::Explore { map } => Some(map.clone()),
            _ => None,
        };
        ws.hunted = false;
        let Some(steps) = plan(&ws, &goal, &ctx) else {
            self.give_up(&goal, "no plan reaches it");
            return Ok(());
        };
        let Some(step) = steps.first().cloned() else {
            return Ok(());
        };
        // A leader does not pull more while a member in sight is low on HP and nothing attacks it: it gets its HP back.
        if matches!(step, Step::Hunt { .. })
            && self.bot.member_in_trouble(&obs)
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
        if self.run.last_step.as_ref() != Some(&step) {
            tracing::info!(goal = %goal.label(), plan = ?steps, "plan");
        }
        self.run.last_step = Some(step.clone());

        match self.bot.execute(&step).await? {
            Outcome::Done => {
                self.run.step_failures = 0;
                self.run.last_outcome = format!("{step:?} done");
            }
            Outcome::Preempted => {
                tracing::info!(?step, "step preempted by a new goal");
                self.run.last_outcome = format!("{step:?} interrupted for a new goal");
            }
            Outcome::Failed(reason)
                if matches!(step, Step::Hunt { .. })
                    && !hopeless(&reason)
                    && !reason.contains("died") =>
            {
                // One target that runs off or sits out of reach says nothing about the map: it is blacklisted, go on.
                tracing::info!(%reason, "target skipped");
                self.run.last_outcome = format!("target skipped: {reason}");
            }
            Outcome::Failed(reason) => {
                self.run.step_failures += 1;
                tracing::info!(?step, %reason, "step failed");
                self.run.last_outcome = format!("{step:?} failed: {reason}");
                if hopeless(&reason) || self.run.step_failures >= MAX_STEP_FAILURES {
                    self.give_up(&goal, &reason);
                }
            }
        }
        let obs = self.bot.observe().await?;
        let ws = world_state(&self.bot, &obs);
        self.publish(&obs, &ws);
        Ok(())
    }

    fn give_up(&mut self, goal: &Goal, reason: &str) {
        tracing::info!(goal = %goal.label(), %reason, "goal given up");
        self.bot
            .memory
            .note(format!("gave up '{}': {reason}", goal.label()));
        self.run.failures.push((goal.clone(), Instant::now()));
        self.run.goal = None;
        self.run.step_failures = 0;
        // The strategist answers the event at once, maybe before the next snapshot: it must not choose the goal again.
        self.handle.snapshots.send_modify(|snap| {
            if let Some(snap) = snap {
                snap.recent_failures.push(goal.clone());
                snap.goal = None;
            }
        });
        self.handle
            .events
            .send(Event::GoalFailed(format!("{}: {reason}", goal.label())))
            .ok();
    }

    fn publish(&mut self, obs: &Observation, ws: &WorldState) {
        self.run
            .failures
            .retain(|(_, at)| at.elapsed() < FAILURE_MEMORY);
        let sorted = |m: &std::collections::BTreeMap<String, u64>| {
            let mut v: Vec<(String, u64)> = m.iter().map(|(k, v)| (k.clone(), *v)).collect();
            v.sort_by(|a, b| b.1.cmp(&a.1));
            v
        };
        let snap = Snapshot {
            obs: obs.clone(),
            ws: ws.clone(),
            goal: self.run.goal.clone(),
            goal_secs: self.run.goal_since.elapsed().as_secs(),
            last_outcome: self.run.last_outcome.clone(),
            weight_frac: obs.me.weight as f32 / max_weight(&obs.me.job, obs.me.stats.str) as f32,
            heal_hp: self.bot.knowledge.heal_stock(obs),
            heal_items: self.bot.knowledge.heal_items(obs),
            loot_items: obs
                .inventory
                .iter()
                .filter(|i| is_loot_item(i, &self.bot.keep))
                .map(|i| i.amount)
                .sum(),
            potion_price: self
                .bot
                .memory
                .potion_shop
                .as_ref()
                .map(|s| s.price)
                .unwrap_or(0),
            kills: self.bot.memory.counters.kills,
            deaths: self.bot.memory.counters.deaths,
            kills_by_mob: sorted(&self.bot.memory.kills_by_mob),
            deaths_by_cause: sorted(&self.bot.memory.deaths_by_cause),
            journal: self.bot.memory.journal.clone(),
            visited: self.bot.memory.visited_maps.iter().cloned().collect(),
            recent_failures: self.run.failures.iter().map(|(g, _)| g.clone()).collect(),
            bad_shops: self.bot.memory.bad_shops.clone(),
            losing_to: self.bot.memory.losing_to(obs.me.base_level),
            quests: self.bot.memory.quests.clone(),
            requests: self.bot.social.pending.clone(),
            party_play: self.bot.party.is_some(),
            friends: self
                .bot
                .party
                .as_ref()
                .map(|p| p.config.friends.clone())
                .unwrap_or_default(),
            notes: self.bot.memory.strategy.notes.clone(),
            job_target: self.bot.memory.strategy.job_target.clone(),
            can_heal: self.bot.can_heal(obs),
            deadly_ways: self.bot.recent_deadly_ways(),
        };
        self.handle.snapshots.send_replace(Some(snap));
    }
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
