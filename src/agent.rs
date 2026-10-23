//! The main loop: observe, react, follow the strategist's goal through GOAP one step at a time, report back.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use rand::Rng;

use crate::api::{Action, ApiError, Observation, dist};
use crate::behaviors::town_break::{TownBreak, partied};
use crate::behaviors::town_quests;
use crate::bot::{Bot, Outcome};
use crate::config::RunArgs;
use crate::goap::state::{Extra, is_loot_item};
use crate::goap::{Goal, PlanContext, Step, WorldState, plan};
use crate::memory::Counters;
use crate::party::chat::Said;
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
/// How long a bot whose session is over plays on to reach a town before it logs off where it is.
const LEAVE_WITHIN: Duration = Duration::from_secs(20 * 60);
/// Most warps to the town a bot whose session is over heads for.
const TOWN_HOPS: u32 = 8;
/// Minutes of play between two town breaks, and before the first.
const BREAK_EVERY: std::ops::Range<u64> = 50..70;
const FIRST_BREAK: std::ops::Range<u64> = 10..50;
/// A break that cannot start now (a quest, no town near) is tried again this much later.
const BREAK_LATER: Duration = Duration::from_secs(5 * 60);
/// Minutes a member's party break lasts beyond what the leader called, waiting for its "break's over".
const MEMBER_BREAK_SPARE: u32 = 2;

fn minutes_from_now(range: std::ops::Range<u64>) -> Instant {
    Instant::now() + Duration::from_secs(rand::rng().random_range(range) * 60)
}

/// What a bot says to its party before it logs off.
const GOODBYES: &[&str] = &[
    "gotta go, bye all",
    "logging off, see you",
    "time for me to go, bye",
    "that's it for me today, cya",
    "bye guys, thanks for the party",
];

/// What ends a run.
#[derive(Clone, Default)]
pub struct Stop {
    /// Ctrl-C or the end of `--minutes`: the bot stops after its current step.
    pub now: Arc<AtomicBool>,
    /// The bot's session is over: it plays on until it is in a town, says bye to its party there and logs off.
    pub in_town: Option<Arc<AtomicBool>>,
}

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
    /// The session is over since then: the bot logs off in the next town.
    leaving: Option<Instant>,
    /// The town it heads for to log off, when one is in reach.
    leave_to: Option<String>,
    /// The town break the bot is on.
    town_break: Option<TownBreak>,
    /// When the next town break is due; `None` without breaks.
    next_break: Option<Instant>,
    /// The goal taken for a stage of a quest, and the quest's id.
    quest_goal: Option<(Goal, String)>,
}

/// The bot, the strategist's handle and the state of the run: what the loop works on.
struct Agent {
    bot: Bot,
    handle: Handle,
    run: Run,
}

/// Plays until `stop` says so, the time is up or the level is reached.
pub async fn run(bot: Bot, handle: Handle, args: RunArgs, stop: Stop) -> Result<()> {
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
        leaving: None,
        leave_to: None,
        town_break: None,
        next_break: args
            .town_break
            .rotates()
            .then(|| minutes_from_now(FIRST_BREAK)),
        quest_goal: None,
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
        if stop.now.load(Ordering::SeqCst) {
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
        if agent.run.leaving.is_none()
            && stop
                .in_town
                .as_ref()
                .is_some_and(|f| f.load(Ordering::SeqCst))
        {
            agent.run.leaving = Some(Instant::now());
            if !agent.bot.is_town(&obs.map.name).await {
                agent.run.leave_to = agent.nearest_town(&obs.map.name).await;
                match &agent.run.leave_to {
                    Some(town) => {
                        tracing::info!(%town, "the session is over: logging off in the next town")
                    }
                    None => tracing::info!(
                        "the session is over, no town in reach: logging off in a town on the way, or here within 20 minutes"
                    ),
                }
            }
        }
        if let Some(since) = agent.run.leaving
            && !obs.me.dead
        {
            let in_town = agent.bot.is_town(&obs.map.name).await;
            if in_town || since.elapsed() >= LEAVE_WITHIN {
                if in_town {
                    tracing::info!(map = %obs.map.name, "in town: logging off");
                } else {
                    tracing::info!(map = %obs.map.name, "no town reached in time: logging off here");
                }
                if agent.run.town_break.is_some() {
                    agent.end_break(&obs, "the session is over").await;
                }
                agent.say_goodbye(&obs).await;
                break Ok(());
            }
        }
        // Taken now or never: a call heard on an errand is not followed when the errand is over
        let call = agent.bot.party.as_mut().and_then(|p| p.break_call.take());
        if let Some(b) = &mut agent.run.town_break {
            // The party broke up: the break goes on as the bot's own
            if b.with_party && !partied(&obs) {
                b.with_party = false;
            }
            let called_off = b.with_party
                && agent
                    .bot
                    .party
                    .as_mut()
                    .is_some_and(|p| std::mem::take(&mut p.break_over));
            let end = if !b.with_party && partied(&obs) {
                Some("found a party")
            } else if called_off {
                Some("the leader ended it")
            } else if obs.me.dead {
                Some("died")
            } else if b.over() {
                Some("the break is over")
            } else {
                None
            };
            if let Some(why) = end {
                agent.end_break(&obs, why).await;
            }
        } else if agent.run.leaving.is_some() || obs.me.dead {
        } else if let Some((town, minutes)) = call {
            agent.join_party_break(&obs, town, minutes);
        } else if agent.run.next_break.is_some_and(|at| Instant::now() >= at) {
            agent.start_break(&obs, &args).await;
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

impl Agent {
    /// The quest the current goal is a stage of.
    fn current_quest(&self) -> Option<&str> {
        self.run
            .quest_goal
            .as_ref()
            .filter(|(g, _)| self.run.goal.as_ref() == Some(g))
            .map(|(_, q)| q.as_str())
    }

    /// Away from the party on a goal of its own: a quest's talk, a favour, a job quest. Between two goals, the quest of
    /// the last one: a break called in the second between two stages of a job quest held it up for the break.
    fn busy(&self) -> bool {
        let quest = match self.run.goal {
            Some(_) => self.current_quest(),
            None => self.run.quest_goal.as_ref().map(|(_, q)| q.as_str()),
        };
        matches!(self.run.goal, Some(Goal::Quest { .. } | Goal::Help { .. }))
            || quest.is_some_and(|q| q.starts_with("job:"))
    }

    /// A break in the nearest town, as players take one, not in the middle of a quest or a favour. Alone, the break is
    /// for finding a party; the leader of a party calls the break of the whole party, and a member waits for that.
    async fn start_break(&mut self, obs: &Observation, args: &RunArgs) {
        let with_party = partied(obs);
        if with_party && !crate::party::leads(obs) {
            tracing::debug!("in a party: the leader calls the breaks");
            self.run.next_break = Some(minutes_from_now(BREAK_EVERY));
            return;
        }
        if self.busy() {
            self.run.next_break = Some(Instant::now() + BREAK_LATER);
            return;
        }
        let town = if self.bot.is_town(&obs.map.name).await {
            Some(obs.map.name.clone())
        } else {
            self.nearest_town(&obs.map.name).await
        };
        let Some(town) = town else {
            self.run.next_break = Some(Instant::now() + BREAK_LATER);
            return;
        };
        let length = args.town_break.pick();
        let minutes = (length.as_secs() / 60) as u32;
        tracing::info!(%town, minutes, with_party, "time for a break in town");
        let mut b = TownBreak::new(town.clone(), length);
        if with_party {
            b = b.of_party();
            self.say_surely(Said::Break { town, minutes }).await;
        }
        self.run.town_break = Some(b);
    }

    /// The leader called a break in `town`: the member goes there too, unless it is away on a goal of its own.
    fn join_party_break(&mut self, obs: &Observation, town: String, minutes: u32) {
        if self.busy() || !partied(obs) {
            tracing::info!(%town, "the party takes a break, the bot carries on");
            return;
        }
        tracing::info!(%town, minutes, "the party takes a break in town");
        // The leader's "break's over" ends it; the bot's own time is for when that line is missed. A member there before
        // the leader got up a minute before it
        let length = Duration::from_secs(u64::from(minutes.clamp(1, 30) + MEMBER_BREAK_SPARE) * 60);
        self.run.town_break = Some(TownBreak::new(town, length).of_party());
        self.run.next_break = Some(minutes_from_now(BREAK_EVERY));
    }

    /// Says it to the party, after the gap between two lines when one was just said.
    async fn say_surely(&mut self, said: Said) {
        if !self.bot.say_to_party(said.clone()).await {
            Bot::pause(2100).await;
            self.bot.say_to_party(said).await;
        }
    }

    /// The break ends: the store and the chat room close, and the strategist picks what comes next.
    async fn end_break(&mut self, obs: &Observation, why: &str) {
        let Some(b) = self.run.town_break.take() else {
            return;
        };
        tracing::info!(town = %b.town, %why, "the town break ends");
        self.bot.end_town_break(obs).await;
        if b.with_party && crate::party::leads(obs) {
            self.say_surely(Said::BreakOver).await;
        }
        self.run.next_break = Some(minutes_from_now(BREAK_EVERY));
        if self.run.goal
            == Some(Goal::Explore {
                map: b.town.clone(),
            })
        {
            self.run.goal = None;
        }
        self.run.last_outcome = format!("town break in {} over: {why}", b.town);
        self.handle
            .events
            .send(Event::GoalDone(format!("a break in {}", b.town)))
            .ok();
    }

    /// The town fewest warps from `from`, within `TOWN_HOPS`.
    async fn nearest_town(&self, from: &str) -> Option<String> {
        let maps = self
            .bot
            .knowledge
            .graph
            .neighbourhood(from, TOWN_HOPS)
            .await
            .ok()?;
        for (map, _) in maps {
            if self.bot.is_town(&map).await {
                return Some(map);
            }
        }
        None
    }

    /// Bye to the party members in the game, then out of a party found on the booking board and its ad off the board:
    /// the bot will not be back for a while. A party of `--party` waits for it.
    async fn say_goodbye(&mut self, obs: &Observation) {
        let others = obs.party.as_ref().is_some_and(|p| {
            p.members
                .iter()
                .any(|m| m.online && !m.name.eq_ignore_ascii_case(&obs.me.name))
        });
        if others {
            use rand::seq::IndexedRandom;
            let text = GOODBYES
                .choose(&mut rand::rng())
                .copied()
                .unwrap_or("bye")
                .to_owned();
            match self.bot.act(Action::PartyChat { text: text.clone() }).await {
                Ok(_) => tracing::info!(%text, "said to the party"),
                Err(error) => tracing::debug!(%error, "party chat refused"),
            }
            // The words reach the party before the bot is gone
            Bot::pause(1500).await;
        }
        let found = self.bot.party.as_ref().is_some_and(|p| p.config.auto);
        if found {
            if obs.booking.is_some() {
                self.bot.act(Action::BookingDelete).await.ok();
            }
            if obs.party.is_some() {
                self.bot.act(Action::PartyLeave).await.ok();
            }
        }
    }
}

/// How far from its shop spot the bot still stands at the shop.
pub const AT_SHOP: u32 = 6;

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
        .is_some_and(|(job, lvl)| *job == obs.me.class && *lvl == obs.me.job_level)
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
        // A town quest taken or done changes what is worth doing
        let quests = |o: &Observation| {
            let held = town_quests::held(o);
            (held.len(), held.iter().filter(|q| q.done).count())
        };
        let (now, before) = (quests(obs), quests(prev));
        if now.1 > before.1 {
            self.handle
                .events
                .send(Event::GoalDone("a town quest".into()))
                .ok();
        } else if now.0 > before.0 {
            self.handle
                .events
                .send(Event::GoalDone("took a town quest".into()))
                .ok();
        }
        if obs.me.dead && !prev.me.dead {
            // Counted here so deaths on the way somewhere count too, not only those in a fight.
            self.bot.memory.counters.deaths += 1;
            self.handle.events.send(Event::Died).ok();
        }
    }

    async fn tick(&mut self, obs: Observation) -> Result<()> {
        self.bot.knowledge.learn_heals(&obs).await;
        // The line a rebirth follows: the second job, kept once the class no longer shows it
        if !obs.me.reborn
            && obs.me.past_job.is_some()
            && self.bot.memory.past_job != obs.me.past_job
        {
            self.bot.memory.past_job = obs.me.past_job.clone();
        }
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
            && let Some(heal) = self.bot.knowledge.heal_action(&obs, &self.bot.keep)
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
        // A bot on its way out neither recruits nor joins
        if self.run.leaving.is_none()
            && self
                .bot
                .party_reflexes(&obs, self.run.goal.as_ref())
                .await?
        {
            return Ok(());
        }

        self.bot.keep = crate::quests::kept_items(&self.bot.memory.quests);
        self.bot.keep.extend(town_quests::kept_items(&obs));
        self.bot.follow_town_quests(&obs).await;
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
        // A High Novice's job is the one of its line
        let awaits_job = obs.me.job.eq_ignore_ascii_case("novice")
            && !obs.me.reborn
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
            // A favour asked of a bot on a break ends the break; other goals wait for its end
            if self.run.town_break.is_some() && matches!(d.goal, Goal::Help { .. }) {
                self.end_break(&obs, "asked for a favour").await;
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
            } else if self.run.town_break.is_some() {
                tracing::debug!(goal = %d.goal.label(), "on a break: the goal waits");
            } else if self.run.leave_to.is_none() && self.run.goal.as_ref() != Some(&d.goal) {
                tracing::info!(goal = %d.goal.label(), reason = %d.reason, by_llm = d.by_llm, "new goal");
                self.run.quest_goal = d
                    .option
                    .strip_prefix("quest:")
                    .map(|q| (d.goal.clone(), q.to_owned()));
                self.run.goal = Some(d.goal);
                self.run.goal_since = Instant::now();
                self.run.step_failures = 0;
            }
        }
        // A bot whose session is over walks to a town to log off there, whatever the strategist says
        if let Some(town) = &self.run.leave_to {
            let goal = Goal::Explore { map: town.clone() };
            let failed = self
                .run
                .failures
                .iter()
                .any(|(g, at)| *g == goal && at.elapsed() < FAILURE_MEMORY);
            if !failed && self.run.goal.as_ref() != Some(&goal) {
                tracing::info!(%town, "heading for a town to log off");
                self.run.goal = Some(goal);
                self.run.goal_since = Instant::now();
                self.run.step_failures = 0;
            }
        }
        // In a town of the alive_towns plugin, the bot asks what the townsfolk offer and takes the quests it can do. A
        // member does it on the party's break, not while following; a bot on a break before it sits down
        let free = !partied(&obs) || crate::party::leads(&obs) || self.run.town_break.is_some();
        if self.run.leave_to.is_none()
            && free
            && !self.busy()
            && !self.run.town_break.as_ref().is_some_and(|b| b.settled())
            && obs.dialog.is_none()
            && self.bot.take_town_quests(&obs).await?
        {
            self.run.last_outcome = "asked the townsfolk for quests".into();
            return Ok(());
        }
        // A bot on a break walks to its town, and there sits among the others
        if self.run.leave_to.is_none()
            && let Some(mut b) = self.run.town_break.take()
        {
            if obs.map.name == b.town {
                self.publish(&obs, &ws);
                let step = self.bot.town_break_step(&obs, &mut b).await;
                self.run.town_break = Some(b);
                self.run.last_outcome = "on a break in town".into();
                return step;
            }
            let goal = Goal::Explore {
                map: b.town.clone(),
            };
            let failed = self
                .run
                .failures
                .iter()
                .any(|(g, at)| *g == goal && at.elapsed() < FAILURE_MEMORY);
            if failed {
                tracing::info!(town = %b.town, "the town of the break is out of reach: no break");
                self.run.town_break = Some(b);
                self.end_break(&obs, "the town is out of reach").await;
            } else {
                if self.run.goal.as_ref() != Some(&goal) {
                    tracing::info!(town = %b.town, "heading for a town for a break");
                    self.run.goal = Some(goal);
                    self.run.goal_since = Instant::now();
                    self.run.step_failures = 0;
                }
                self.run.town_break = Some(b);
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
        // In a party, following the leader replaces the hunt; errands still run as planned. A bot on its way out goes alone.
        let quest = self.current_quest().map(str::to_owned);
        if self.run.leave_to.is_none()
            && let Some(outcome) = self
                .bot
                .party_step(&obs, self.run.goal.as_ref(), quest.as_deref())
                .await?
        {
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
            // Met as soon as taken (rest at full HP, when nothing else is offered): the strategist would hand it
            // straight back, twice a second
            if self.run.goal_since.elapsed() < Duration::from_secs(2) {
                Bot::pause(5000).await;
            }
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
            town_quests: self.bot.memory.town_quests.clone(),
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
