//! Walking from map to map along the warp graph, and leaving the training grounds.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::Value;
use tokio::task::JoinHandle;

use super::curiosity::{Curiosity, EXPLORE_TALKS, TOWN_CHANCE};
use super::dialog::{DialogEnd, Intent, dialog_of};
use crate::api::observation::Warp;
use crate::api::{Action, Observation, dist};
use crate::bot::{Bot, Outcome};
use crate::goap::state::HP_LOW;
use crate::knowledge::graph::{Edge, FARE_SAVES, Trip};
use crate::knowledge::grounds::lethal_monster;
use crate::knowledge::talks::shown_name;
use crate::party::{CATCH_UP_KEPT, CATCH_UP_SAVES};

/// Most warps one trip takes before giving up.
const MAX_LEGS: usize = 30;
/// Cells from a random warp past which the bot was sent on: a landing is at least 4 cells from the warp it came by.
/// Most random warps one trip into a room takes before giving up.
const MAX_RANDOM_LEGS: usize = 80;
const RANDOM_WARP_REACH: u32 = 3;
/// Cells between a warp inside a map as the reference places it and as the observation shows it.
const INNER_WARP_OFF: u32 = 2;
/// Tries to step on one warp, a walk being cut by the 30 s action timeout.
const WARP_TRIES: usize = 5;
/// Walks to one warp cut short by attacks, before giving up.
const ATTACKED_WALKS: usize = 20;
/// Fights back on the way to one warp; after that the bot only flees.
const MAX_FIGHTS: usize = 4;
/// HP checks while walking.
const WALK_POLL_MS: u64 = 400;
/// A monster this close when HP drops is taken for the attacker.
const ATTACKER_RANGE: u32 = 3;
/// Fight back only when the expected damage of the fight leaves at least this share of max HP.
const FIGHT_MARGIN: f32 = 0.2;
/// Towns `@go` sends a bot to under their own map name. `@go morroc` lands on `old_moc`, and `novice` and `jail` are
/// no way out.
pub const GO_TOWNS: &[&str] = &[
    "prontera",
    "geffen",
    "payon",
    "alberta",
    "izlude",
    "aldebaran",
    "comodo",
    "yuno",
    "amatsu",
    "gonryun",
    "umbala",
    "niflheim",
    "louyang",
    "jawaii",
    "ayothaya",
    "einbroch",
    "lighthalzen",
    "einbech",
    "hugel",
    "rachel",
    "veins",
    "moscovia",
];
/// How near the spot of an NPC way the NPC asked stands: the NPCs of a way are spots apart.
const NPC_NEAR: u32 = 5;
/// An NPC way out of the map the bot is on (a guard, not a town's Kafra) further than this is left out: the walk to
/// it crosses the map, and moc_fild20 is a level 80 field with a guard at every landing.
const NPC_FAR: u32 = 40;
/// Maps around the destination searched for a town to `@go` to.
const GO_SEARCH_HOPS: u32 = 6;
/// Start of the failure reason of a warp in another part of the map (another room of an interior map).
const UNREACHABLE: &str = "unreachable from here";
/// Start of the failure reason of a walk that ended on another warp than the one it went for.
const STRAY: &str = "stepped on another warp";
/// Other warps stepped on by mistake in one trip before giving up: a walk that crosses a door keeps going in.
const MAX_STRAYS: usize = 4;
/// How long a warp stepped off by mistake stays out of the routes of trips, once it was stepped off twice.
pub const STRAY_MEMORY: Duration = Duration::from_secs(10 * 60);
/// Times a walk to a warp landed on another map, within `STRAY_MEMORY`, after which the trips leave that warp out. One
/// is enough: the walk to the same warp landed on the same other warp 172 times in 30 minutes of one run.
const STRAYS_TO_AVOID: usize = 1;
/// How long a map the bot died crossing stays out of its routes and hunting grounds.
pub const DEADLY_WAY_MEMORY: Duration = Duration::from_secs(30 * 60);
/// A walk to a warp this far or further is a long one: the server walks it in legs found over the whole map, which go
/// through other warps' cells, while it takes a shorter walk around them.
const SHORT_WALK: u32 = 10;
/// Another warp this near makes the start of a long walk go in short legs first.
const WARP_NEAR: u32 = 12;
/// Short legs at most before the long walk.
const MAX_CLEAR_LEGS: usize = 4;

/// The cell `len` steps from `from` toward `to`, in the eight directions of a walk.
pub fn leg_toward(from: (u16, u16), to: (u16, u16), len: u16) -> (u16, u16) {
    let step = |a: u16, b: u16| {
        if b > a {
            a + (b - a).min(len)
        } else {
            a - (a - b).min(len)
        }
    };
    (step(from.0, to.0), step(from.1, to.1))
}

/// The warps that a walk kept landing off, `STRAYS_TO_AVOID` times or more within `STRAY_MEMORY`: a trip leaves them out.
/// A party member following its leader through a door next to a town's warp went round it 4,470 times in one run.
fn avoided_strays(edges: &[(String, Instant)]) -> HashSet<String> {
    let mut count: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (key, _) in edges.iter().filter(|(_, at)| at.elapsed() < STRAY_MEMORY) {
        *count.entry(key.as_str()).or_default() += 1;
    }
    count
        .into_iter()
        .filter(|(_, times)| *times >= STRAYS_TO_AVOID)
        .map(|(key, _)| key.to_owned())
        .collect()
}

/// How a guarded walk to a warp ended.
enum Walk {
    /// The walk returned (`map_changed`, `arrived`, `stopped`, `timeout`) or was refused.
    Ended(Result<Value>),
    MapChanged,
    Dead {
        attacker: Option<String>,
    },
    /// Something the bot can beat attacks it: fight back, then walk on.
    FightBack {
        target: u64,
        name: String,
    },
    /// A new goal, or someone talking to the bot: it stops where it is.
    Preempted,
}

pub(crate) fn journey(result: &Value) -> &str {
    result.get("journey").and_then(Value::as_str).unwrap_or("")
}

/// The first door of the shortest way between two rooms, doors being (from room, cell, to room).
fn first_door(doors: &[(u32, (u16, u16), u32)], from: u32, to: u32) -> Option<(u16, u16)> {
    let mut first: std::collections::HashMap<u32, (u16, u16)> = std::collections::HashMap::new();
    let mut queue = std::collections::VecDeque::from([from]);
    let mut seen = HashSet::from([from]);
    while let Some(room) = queue.pop_front() {
        if room == to {
            return first.get(&to).copied();
        }
        for (a, cell, b) in doors.iter().filter(|(a, _, _)| *a == room) {
            if seen.insert(*b) {
                let door = if *a == from { *cell } else { first[a] };
                first.insert(*b, door);
                queue.push_back(*b);
            }
        }
    }
    None
}

/// The warp of the observation that matches a warp of the database: same destination, nearest position. Inside one
/// map every warp leads to that map, so the warp must stand where the edge's does: on `yuno_in05`, the nearest one
/// to the first random warp was `yun76`, out of reach in another part of the map. So must a random warp's, which
/// shares its landings with other warps of the map (the random warps of `c_tower2` all lead into `c_tower3`).
fn matching_warp<'a>(obs: &'a Observation, edge: &Edge) -> Option<&'a Warp> {
    let in_place = edge.random || edge.to == edge.from;
    obs.warps
        .iter()
        .filter(|w| w.to_map == edge.to)
        .filter(|w| !in_place || dist((w.x, w.y), (edge.x, edge.y)) <= INNER_WARP_OFF)
        .min_by_key(|w| dist((w.x, w.y), (edge.x, edge.y)))
}

impl Bot {
    /// The warp from `here` into the room of `target` where `spot` is: one landing in the spot's walkable region,
    /// else the one landing nearest to it.
    async fn door_into(
        &self,
        here: &str,
        target: &str,
        spot: (u16, u16),
        skip: &HashSet<String>,
    ) -> Result<Option<Edge>> {
        let doors: Vec<Edge> = self
            .knowledge
            .graph
            .edges(here)
            .await?
            .into_iter()
            .filter(|e| e.to == target && e.talk.is_none() && !skip.contains(&e.key()))
            .collect();
        if let Ok(data) = self.knowledge.db.map(target).await {
            let region = data
                .npcs
                .iter()
                .filter(|n| dist((n.x, n.y), spot) <= 2)
                .min_by_key(|n| dist((n.x, n.y), spot))
                .and_then(|n| n.region);
            if let Some(region) = region {
                let lands_there = |e: &&Edge| {
                    data.arrivals.iter().any(|a| {
                        a.from.map.id_str() == here
                            && a.from.x == e.x
                            && a.from.y == e.y
                            && a.land.region == Some(region)
                    })
                };
                if let Some(e) = doors.iter().find(lands_there) {
                    return Ok(Some(e.clone()));
                }
            }
        }
        Ok(doors
            .into_iter()
            .min_by_key(|e| dist((e.to_x, e.to_y), spot)))
    }

    /// Goes into the part `region` of `map` by any warps, out of the map and back in through another door when the
    /// rooms do not connect inside (Nami's room and the Chivalry Member's on `prt_in`).
    pub async fn travel_into(&mut self, map: &str, region: u32) -> Result<Outcome> {
        let mut skip: HashSet<String> = HashSet::new();
        let avoid: HashSet<String> = self.memory.strategy.avoid_maps.iter().cloned().collect();
        // A maze of random warps takes 12 of them on average from the entrance of yuno_in05, and many more at times
        let (mut legs, mut random_legs) = (0, 0);
        while legs < MAX_LEGS && random_legs < MAX_RANDOM_LEGS {
            if self.preempted() {
                return Ok(Outcome::Preempted);
            }
            let obs = self.observe().await?;
            if obs.map.name == map && obs.me.region.is_none_or(|r| r == region) {
                return Ok(Outcome::Done);
            }
            let route = self
                .knowledge
                .graph
                .route_for(
                    Trip::new(&obs.map.name, map, &avoid)
                        .starting_in(obs.me.region)
                        .ending_in(Some(region))
                        .skipping(&skip),
                )
                .await?;
            let Some(edge) = route.as_ref().and_then(|r| r.first()) else {
                return Ok(Outcome::failed(format!(
                    "no known way into that room of {map}"
                )));
            };
            tracing::info!(from = %obs.map.name, to = %edge.to, room = region, "into a room");
            if edge.random {
                random_legs += 1;
            } else {
                legs += 1;
            }
            match self.take_warp(&obs, edge).await? {
                Outcome::Done => {}
                Outcome::Preempted => return Ok(Outcome::Preempted),
                Outcome::Failed(reason) => {
                    if self.observe().await?.me.dead {
                        return Ok(Outcome::Failed(reason));
                    }
                    skip.insert(edge.key());
                }
            }
        }
        Ok(Outcome::failed(format!(
            "too many warps on the way into {map}"
        )))
    }

    /// Walks to another room (region) of the map the bot is on, through the warps between its rooms. True once there.
    pub async fn reach_region(&mut self, target: u32) -> Result<bool> {
        for _ in 0..8 {
            let obs = self.observe().await?;
            let Some(at) = obs.me.region else {
                return Ok(false);
            };
            if at == target {
                return Ok(true);
            }
            let here = obs.map.name.clone();
            let Ok(data) = self.knowledge.db.map(&here).await else {
                return Ok(false);
            };
            // The warps from room to room: where each is, and the room it lands in.
            let doors: Vec<(u32, (u16, u16), u32)> = data
                .arrivals
                .iter()
                .filter(|a| a.from.map.id_str() == here)
                .filter_map(|a| Some((a.from.region?, (a.from.x, a.from.y), a.land.region?)))
                .collect();
            let Some(first) = first_door(&doors, at, target) else {
                tracing::info!(map = %here, from = at, to = target, "no way between the rooms");
                return Ok(false);
            };
            tracing::info!(map = %here, from = at, to = target, door = ?first, "through a door to another room");
            let action = match obs
                .warps
                .iter()
                .find(|w| w.region.is_none_or(|r| r == at) && dist((w.x, w.y), first) <= 1)
            {
                Some(w) => Action::Use { target: w.id },
                None => Action::Move {
                    x: first.0,
                    y: first.1,
                    wait: true,
                },
            };
            if let Err(error) = self.act(action).await {
                tracing::warn!(%error, "could not take the door");
                return Ok(false);
            }
            Bot::pause(600).await;
        }
        Ok(false)
    }

    pub async fn travel(&mut self, target: &str) -> Result<Outcome> {
        let mut failures = 0;
        // `@go` is the way out of a map the graph cannot leave (guards instead of warps, wrong guesses), once a trip.
        let mut went = false;
        // Warps in another room than the bot's, left out for this trip only; and those a walk kept landing off.
        self.stray_edges
            .retain(|(_, at)| at.elapsed() < STRAY_MEMORY);
        let mut skip = avoided_strays(&self.stray_edges);
        // Interior maps are several rooms: into the shop's map, take the warp that lands nearest to the shop.
        let near = match &self.travel_spot {
            Some((map, spot)) if map == target => Some(*spot),
            _ => self
                .memory
                .potion_shop
                .as_ref()
                .filter(|s| s.map == target)
                .map(|s| (s.x, s.y)),
        };
        // Maps on the way with an aggressive monster that kills in two hits, avoided when another way exists.
        let mut deadly: HashSet<String> = HashSet::new();
        let mut checked: HashSet<String> = HashSet::new();
        // Maps the bot died crossing lately, unless it goes there
        self.deadly_ways
            .retain(|(_, at)| at.elapsed() < DEADLY_WAY_MEMORY);
        for (map, _) in &self.deadly_ways {
            if map != target {
                deadly.insert(map.clone());
                checked.insert(map.clone());
            }
        }
        let mut strays = 0;
        // The warps this trip stepped off by mistake and left out since: taken again when they were the only way
        let mut strayed: HashSet<String> = HashSet::new();
        let fighter = self.knowledge.fighter(&self.observe().await?).await?;
        // NPCs talked to on the map the goal explores, and the map of the last leg: a town entered gets a roll.
        let mut explore_talks = 0;
        let mut last_map: Option<String> = None;
        for _ in 0..MAX_LEGS {
            if self.preempted() {
                return Ok(Outcome::Preempted);
            }
            let obs = self.observe().await?;
            let here = obs.map.name.clone();
            // Unknown NPCs: a few on the map the goal explores, now and then one in a town entered or near the way
            let entered = last_map.as_deref().is_some_and(|m| m != here);
            last_map = Some(here.clone());
            let why = if here == target
                && self.exploring.as_deref() == Some(target)
                && explore_talks < EXPLORE_TALKS
            {
                Some(Curiosity::Explore)
            } else if entered && rand::random::<f64>() < TOWN_CHANCE && self.is_town(&here).await {
                Some(Curiosity::Town)
            } else {
                (here != target).then_some(Curiosity::Near)
            };
            if let Some(why) = why
                && self.curious(&obs, why).await?
            {
                explore_talks += usize::from(why == Curiosity::Explore);
                continue;
            }
            if here == target {
                return Ok(Outcome::Done);
            }
            if here.starts_with("new_") && !target.starts_with("new_") {
                match self.exit_training().await? {
                    Outcome::Done => continue,
                    failed => return Ok(failed),
                }
            }
            let avoid: HashSet<String> = self.memory.strategy.avoid_maps.iter().cloned().collect();
            // Catching up with a party too far on foot is worth a Kafra sooner, with zeny kept for potions
            let catch_up = self.following && self.party.as_ref().is_some_and(|p| p.catch_up);
            let (fare_budget, fare_saves) = if catch_up {
                (obs.me.zeny.saturating_sub(CATCH_UP_KEPT), CATCH_UP_SAVES)
            } else {
                (obs.me.zeny, FARE_SAVES)
            };
            // A guard's way across the map is no way for this trip
            let mut leg_skip = skip.clone();
            for e in self.knowledge.graph.edges(&here).await? {
                if e.talk.as_ref().is_some_and(|t| !t.through)
                    && dist(obs.pos(), (e.x, e.y)) > NPC_FAR
                {
                    leg_skip.insert(e.key());
                }
            }
            let mut route = None;
            for _ in 0..5 {
                let around: HashSet<String> = avoid.union(&deadly).cloned().collect();
                let Some(r) = self
                    .knowledge
                    .graph
                    .route_paying(
                        Trip::new(&here, target, &around)
                            .starting_in(obs.me.region)
                            .skipping(&leg_skip),
                        fare_budget,
                        fare_saves,
                    )
                    .await?
                else {
                    break;
                };
                let mut found = false;
                for map in r.iter().map(|e| e.to.as_str()).filter(|m| *m != target) {
                    if !checked.insert(map.to_owned()) {
                        continue;
                    }
                    if let Some(mob) = lethal_monster(&self.knowledge.db, map, &fighter).await {
                        tracing::info!(%map, %mob, "deadly map on the way, looking for another route");
                        deadly.insert(map.to_owned());
                        found = true;
                    }
                }
                route = Some(r);
                if !found {
                    break;
                }
                route = None;
            }
            // No way around: cross it anyway, fleeing what attacks.
            if route.is_none() && !deadly.is_empty() {
                route = self
                    .knowledge
                    .graph
                    .route_paying(
                        Trip::new(&here, target, &avoid)
                            .starting_in(obs.me.region)
                            .skipping(&leg_skip),
                        fare_budget,
                        fare_saves,
                    )
                    .await?;
            }
            let Some(mut route) = route else {
                // The door into prt_church is the only way in: a walk to it that went into prt_in by mistake left
                // the bot with no route at all, and the Acolyte gave up its last step
                if !strayed.is_empty() {
                    for key in strayed.drain() {
                        skip.remove(&key);
                    }
                    continue;
                }
                if !went && self.go_near(target, &here).await? {
                    went = true;
                    continue;
                }
                return Ok(Outcome::failed(format!(
                    "no known route from {here} to {target}"
                )));
            };
            if let (1, Some(near)) = (route.len(), near)
                && let Some(e) = self.door_into(&here, target, near, &skip).await?
            {
                route = vec![e];
            }
            let Some(edge) = route.first() else {
                return Ok(Outcome::Done);
            };
            tracing::info!(from = %here, to = %edge.to, legs_left = route.len(), "travel");
            match self.take_warp(&obs, edge).await? {
                Outcome::Preempted => return Ok(Outcome::Preempted),
                Outcome::Done => {
                    let obs = self.observe().await?;
                    if self.memory.visited_maps.insert(obs.map.name.clone()) {
                        self.memory.note(format!("first visit to {}", obs.map.name));
                    }
                }
                Outcome::Failed(reason) if reason.starts_with(UNREACHABLE) => {
                    tracing::info!(%reason, "warp in another part of the map, trying another");
                    skip.insert(edge.key());
                }
                // On another map than planned: the next leg starts from there, and walks go around that warp now
                Outcome::Failed(reason) if reason.starts_with(STRAY) => {
                    strays += 1;
                    // The rest of this trip goes another way, or gives up
                    skip.insert(edge.key());
                    strayed.insert(edge.key());
                    self.stray_edges.push((edge.key(), Instant::now()));
                    if strays >= MAX_STRAYS {
                        return Ok(Outcome::failed(format!(
                            "kept stepping on other warps on the way to {target}: {reason}"
                        )));
                    }
                }
                Outcome::Failed(reason) => {
                    failures += 1;
                    tracing::warn!(%reason, "warp failed");
                    // The death reflex respawns the bot; the trip is over.
                    if self.observe().await?.me.dead {
                        return Ok(Outcome::Failed(reason));
                    }
                    if failures >= 3 {
                        if !went && self.go_near(target, &here).await? {
                            went = true;
                            failures = 0;
                            continue;
                        }
                        return Ok(Outcome::failed(format!("stuck on {here}: {reason}")));
                    }
                }
            }
        }
        Ok(Outcome::failed(format!(
            "too many warps on the way to {target}"
        )))
    }

    /// Steps on the warp of `edge`, from the map the bot is on, around the warps it stepped on by mistake before. A
    /// walk that ends on another map than the edge's is a failure: the warp it crossed is kept, to go around it.
    async fn take_warp(&mut self, obs: &Observation, edge: &Edge) -> Result<Outcome> {
        if edge.talk.is_some() {
            return self.ask_way(edge).await;
        }
        let from = obs.map.name.clone();
        if edge.to != from
            && let Some(outcome) = self.clear_of_warps(obs, edge).await?
        {
            return Ok(outcome);
        }
        let obs = &self.observe().await?;
        let outcome = if obs.map.name == from {
            self.step_on_warp(obs, edge).await?
        } else {
            Outcome::Done
        };
        if !matches!(outcome, Outcome::Done) || edge.to == from {
            return Ok(outcome);
        }
        let now = self.observe().await?;
        if now.map.name == from || now.map.name == edge.to {
            return Ok(outcome);
        }
        tracing::info!(on = %from, at = ?now.pos(), landed = %now.map.name, wanted = %edge.to, "stepped on another warp on the way");
        Ok(Outcome::failed(format!(
            "{STRAY}: the warp to {} on {from}, going to {}",
            now.map.name, edge.to
        )))
    }

    /// Before a long walk to the warp of `edge`, short legs toward it while another warp is near: out of an alberta_in
    /// door, the walk to the pay_fild03 exit went back in at once, 2840 times. `Some` when a leg ended the step: the
    /// bot died or a new goal came. A leg that crosses a warp after all leaves the bot on the other map, for
    /// `take_warp` to tell.
    async fn clear_of_warps(&mut self, obs: &Observation, edge: &Edge) -> Result<Option<Outcome>> {
        let from = obs.map.name.clone();
        let goal = matching_warp(obs, edge).map_or((edge.x, edge.y), |w| (w.x, w.y));
        let mut obs = obs.clone();
        for _ in 0..MAX_CLEAR_LEGS {
            let at = obs.pos();
            let near = obs
                .warps
                .iter()
                .any(|w| w.to_map != edge.to && dist((w.x, w.y), at) <= WARP_NEAR);
            if obs.map.name != from || !near || dist(at, goal) <= SHORT_WALK + WARP_NEAR {
                break;
            }
            let (x, y) = leg_toward(at, goal, SHORT_WALK as u16);
            tracing::debug!(on = %from, x, y, "a short leg away from the warps near by");
            let (api, name) = (self.api.clone(), self.name.clone());
            let action = Action::Move { x, y, wait: true };
            let walk = tokio::spawn(async move { api.act(&name, &action).await });
            match self.guard_walk(&from, walk, true).await?.0 {
                Walk::MapChanged => break,
                Walk::Preempted => return Ok(Some(Outcome::Preempted)),
                Walk::Dead { attacker } => return Ok(Some(self.died_on_the_way(&from, attacker))),
                Walk::FightBack { target, name } => {
                    if let Outcome::Failed(reason) = self.fight(target, &name).await?
                        && self.observe().await?.me.dead
                    {
                        return Ok(Some(Outcome::Failed(reason)));
                    }
                }
                // A cell it cannot stand on: the rest of the way in one walk, as before
                Walk::Ended(Err(_)) => break,
                Walk::Ended(Ok(_)) => {}
            }
            obs = self.observe().await?;
        }
        Ok(None)
    }

    /// Steps on the warp of `edge`, from the map the bot is on. Attacked on the way, it fights back what it can beat
    /// and flees from the rest: walking on to the warp leaves the monster behind.
    async fn step_on_warp(&mut self, obs: &Observation, edge: &Edge) -> Result<Outcome> {
        let from = obs.map.name.clone();
        // A warp inside the map lands on another part of it, not another map
        let (inner, start_region) = (edge.to == from, obs.me.region);
        let crossed =
            |obs: &Observation| obs.map.name != from || inner && obs.me.region != start_region;
        // A random warp may land in the part of the map it stands in: taken once a walk onto it left the bot elsewhere
        let jumped = |obs: &Observation| {
            edge.random && dist(obs.pos(), (edge.x, edge.y)) > RANDOM_WARP_REACH
        };
        let (mut tries, mut attacked_walks, mut fights) = (0, 0, 0);
        let mut walked = false;
        while tries < WARP_TRIES && attacked_walks < ATTACKED_WALKS {
            let obs = self.observe().await?;
            if obs.me.dead {
                return Ok(Outcome::failed("dead"));
            }
            if crossed(&obs) || walked && jumped(&obs) {
                return Ok(Outcome::Done);
            }
            walked = true;
            let action = match matching_warp(&obs, edge) {
                Some(w) => Action::Use { target: w.id },
                // A warp the observation does not show (an NPC warp of the database): walk onto its cell.
                None => Action::Move {
                    x: edge.x,
                    y: edge.y,
                    wait: true,
                },
            };
            // The walk runs on its own so the bot can watch its HP meanwhile; any other command (an attack) ends it.
            let (api, name) = (self.api.clone(), self.name.clone());
            let walk = tokio::spawn(async move { api.act(&name, &action).await });
            let (ended, hit) = self.guard_walk(&from, walk, fights < MAX_FIGHTS).await?;
            if hit {
                attacked_walks += 1;
            } else {
                tries += 1;
            }
            match ended {
                Walk::MapChanged => return Ok(Outcome::Done),
                Walk::Preempted => return Ok(Outcome::Preempted),
                Walk::Dead { attacker } => return Ok(self.died_on_the_way(&from, attacker)),
                Walk::FightBack { target, name } => {
                    fights += 1;
                    tracing::info!(%name, on = %from, "attacked on the way, fighting back");
                    if let Outcome::Failed(reason) = self.fight(target, &name).await?
                        && self.observe().await?.me.dead
                    {
                        return Ok(Outcome::Failed(reason));
                    }
                }
                Walk::Ended(Ok(r)) if journey(&r) == "map_changed" => return Ok(Outcome::Done),
                Walk::Ended(Ok(r)) => {
                    tracing::debug!(journey = journey(&r), "walk to warp not finished")
                }
                // The warp is there, but not in the part of the map the bot is in.
                Walk::Ended(Err(error)) if error.to_string().contains("No reachable cell") => {
                    return Ok(Outcome::failed(format!(
                        "{UNREACHABLE}: the warp to {} at ({}, {})",
                        edge.to, edge.x, edge.y
                    )));
                }
                Walk::Ended(Err(error)) => {
                    tracing::warn!(%error, "walk to warp refused");
                    Bot::pause(500).await;
                }
            }
        }
        let obs = self.observe().await?;
        if crossed(&obs) || jumped(&obs) {
            return Ok(Outcome::Done);
        }
        // Every walk ended without a warp and nothing got in the way: there is no usable warp there (a guessed way
        // back, or an entrance behind a quest NPC). Routes leave it out from now on. When a guessed way out fails,
        // the map's other guesses are no better (moc_fild20 has guards instead of warps): they all go.
        if tries >= WARP_TRIES && attacked_walks == 0 && !edge.random {
            let (key, note) = if edge.guessed {
                (
                    Edge::guessed_key(&from),
                    format!("the guessed ways out of {from} do not work"),
                )
            } else {
                (
                    edge.key(),
                    format!(
                        "no usable warp to {} at ({}, {}) on {from}",
                        edge.to, edge.x, edge.y
                    ),
                )
            };
            if !self.memory.bad_warps.contains(&key) {
                self.memory.bad_warps.push(key.clone());
                self.memory.note(note);
            }
            self.knowledge.graph.forbid([key]);
        }
        Ok(Outcome::failed(format!(
            "could not reach the warp to {} at ({}, {})",
            edge.to, edge.x, edge.y
        )))
    }

    /// Asks the NPC of `edge` to send the bot on (a guard, a Kafra): the nearest NPC of that way, walked to when out of
    /// sight. One that does not send the bot is no way from then on, unless it was for want of zeny.
    async fn ask_way(&mut self, edge: &Edge) -> Result<Outcome> {
        let Some(talk) = edge.talk.clone() else {
            return Ok(Outcome::failed(format!("no talk to {}", edge.to)));
        };
        let from = edge.from.clone();
        let kind = talk.names.first().cloned().unwrap_or_default();
        // Observations show no hidden `#` part: the guard of the way is the one standing at its spot
        let of_kind = |name: &str| talk.names.iter().any(|n| shown_name(n) == shown_name(name));
        let fail = |bot: &mut Bot, reason: String| {
            if !bot.memory.bad_warps.contains(&edge.key()) {
                bot.memory.bad_warps.push(edge.key());
                bot.memory.note(format!(
                    "the {} on {from} does not send me to {}",
                    talk.npc, edge.to
                ));
            }
            bot.knowledge.graph.forbid([edge.key()]);
            Outcome::failed(reason)
        };
        let mut obs = self.observe().await?;
        // The nearest of its kind: there is a guard at every landing on moc_fild20, and a town has several Kafras
        let spot = self
            .knowledge
            .graph
            .edges(&from)
            .await?
            .into_iter()
            .filter(|e| e.to == edge.to && e.talk.as_ref().is_some_and(|t| t.names == talk.names))
            .map(|e| (e.x, e.y))
            .min_by_key(|s| dist(*s, obs.pos()))
            .unwrap_or((edge.x, edge.y));
        let seen = |obs: &Observation| {
            obs.npcs
                .iter()
                .filter(|n| of_kind(&n.name) && dist((n.x, n.y), spot) <= NPC_NEAR)
                .min_by_key(|n| dist((n.x, n.y), spot))
                .map(|n| n.id)
        };
        if seen(&obs).is_none() {
            tracing::info!(on = %from, npc = %kind, to = %edge.to, ?spot, "walking to an NPC way");
            let (api, name) = (self.api.clone(), self.name.clone());
            let action = Action::Move {
                x: spot.0,
                y: spot.1,
                wait: true,
            };
            let walk = tokio::spawn(async move { api.act(&name, &action).await });
            match self.guard_walk(&from, walk, true).await?.0 {
                Walk::MapChanged => return Ok(Outcome::Done),
                Walk::Preempted => return Ok(Outcome::Preempted),
                Walk::Dead { attacker } => return Ok(self.died_on_the_way(&from, attacker)),
                Walk::FightBack { target, name } => {
                    self.fight(target, &name).await?;
                    return Ok(Outcome::failed(format!(
                        "fought {name} on the way to the {kind}"
                    )));
                }
                Walk::Ended(_) => {}
            }
            obs = self.observe().await?;
        }
        let Some(npc) = seen(&obs) else {
            return Ok(fail(
                self,
                format!("no {kind} in sight at {spot:?} on {from}"),
            ));
        };
        tracing::info!(on = %from, npc = %kind, to = %edge.to, fare = talk.fare, "asking an NPC for the way");
        let result = match self.act(Action::Use { target: npc }).await {
            Ok(r) => r,
            Err(error) => {
                return Ok(Outcome::failed(format!(
                    "could not talk to the {kind}: {error}"
                )));
            }
        };
        let intent = Intent {
            goal: format!("Be sent to {}", edge.to),
            keywords: talk.keywords.clone(),
            strict: true,
            ..Default::default()
        };
        let zeny = obs.me.zeny;
        let end = self.run_dialog(dialog_of(&result), &intent).await?;
        // The script warps the bot a moment after the dialogue closes.
        for _ in 0..10 {
            let obs = self.observe().await?;
            if obs.map.name != from {
                if obs.dialog.is_some() {
                    self.act(Action::DialogClose).await.ok();
                }
                let paid = zeny.saturating_sub(obs.me.zeny);
                tracing::info!(from = %from, to = %obs.map.name, npc = %kind, paid, "an NPC sent the bot on");
                if paid > 0 {
                    self.memory.note(format!(
                        "paid the {kind} on {from} {paid} zeny to go to {}",
                        obs.map.name
                    ));
                }
                return Ok(Outcome::Done);
            }
            Bot::pause(300).await;
        }
        if self.observe().await?.dialog.is_some() {
            self.act(Action::DialogClose).await.ok();
        }
        let reason = format!(
            "the {kind} on {from} did not send the bot to {} ({end:?})",
            edge.to
        );
        // Too poor this time, not no way
        if self.last_dialog.iter().any(|l| l.contains("enough zeny")) {
            return Ok(Outcome::failed(reason));
        }
        Ok(fail(self, reason))
    }

    /// Watches a walk until it ends, the map changes, the bot dies, or an attacker worth fighting shows up. Weaker
    /// attacks are fled from: the walk goes on, with healing when HP runs low. The flag tells whether the bot was hit.
    async fn guard_walk(
        &mut self,
        from: &str,
        mut walk: JoinHandle<Result<Value>>,
        may_fight: bool,
    ) -> Result<(Walk, bool)> {
        let mut last_hp = self.observe().await?.me.hp;
        let mut hit = false;
        let mut attacker: Option<String> = None;
        loop {
            if walk.is_finished() {
                let result = (&mut walk)
                    .await
                    .unwrap_or_else(|e| Err(anyhow::anyhow!("walk task failed: {e}")));
                return Ok((Walk::Ended(result), hit));
            }
            Bot::pause(WALK_POLL_MS).await;
            let obs = self.observe().await?;
            let me = obs.pos();
            if obs.me.dead {
                walk.abort();
                // Killed between two checks (a monster far above the bot's level): the nearest one did it.
                let attacker = attacker.or_else(|| {
                    obs.mobs
                        .iter()
                        .min_by_key(|m| dist((m.x, m.y), me))
                        .map(|m| m.name.clone())
                });
                return Ok((Walk::Dead { attacker }, true));
            }
            if obs.map.name != from {
                return Ok((Walk::MapChanged, hit));
            }
            // A long walk across a map would keep the bot deaf for a minute; under attack, it walks on
            if self.preempted() && !hit {
                walk.abort();
                self.act(Action::Stop).await.ok();
                return Ok((Walk::Preempted, hit));
            }
            if obs.me.hp >= last_hp {
                last_hp = obs.me.hp;
                continue;
            }
            last_hp = obs.me.hp;
            hit = true;
            // Healing does not stop the walk.
            if obs.me.hp_frac() < HP_LOW
                && let Some(heal) = self.knowledge.heal_action(&obs, &self.keep)
            {
                self.act(heal.action).await.ok();
            }
            let Some(mob) = obs
                .mobs
                .iter()
                .filter(|m| dist((m.x, m.y), me) <= ATTACKER_RANGE)
                .min_by_key(|m| dist((m.x, m.y), me))
            else {
                continue;
            };
            let (target, name) = (mob.id, mob.name.clone());
            let beatable = match self.mob_info(mob.mob_id).await {
                Some(info) => {
                    let fighter = self.knowledge.fighter(&obs).await?;
                    fighter.can_fight(&info)
                        && obs.me.hp_frac() - fighter.assess(&info).danger >= FIGHT_MARGIN
                }
                None => false,
            };
            if may_fight && beatable {
                return Ok((Walk::FightBack { target, name }, true));
            }
            if attacker.as_deref() != Some(name.as_str()) {
                tracing::info!(%name, on = %from, hp = obs.me.hp, "attacked on the way, fleeing to the warp");
            }
            attacker = Some(name);
        }
    }

    /// `@go` to the town with the fewest real warps to `target`, when the graph has no way there from `here`. Tells
    /// whether the bot moved; no town leads there, it stays.
    async fn go_near(&mut self, target: &str, here: &str) -> Result<bool> {
        let Some((town, _)) = self
            .knowledge
            .graph
            .nearest_upstream(target, GO_TOWNS, GO_SEARCH_HOPS)
            .await?
        else {
            tracing::info!(%target, "no town leads to the destination, not using @go");
            return Ok(false);
        };
        if town == here {
            return Ok(false);
        }
        if let Err(error) = self
            .act(Action::Command {
                text: format!("@go {town}"),
            })
            .await
        {
            tracing::warn!(%error, %town, "@go refused");
            return Ok(false);
        }
        for _ in 0..10 {
            Bot::pause(500).await;
            let obs = self.observe().await?;
            if obs.map.name != here {
                self.memory
                    .note(format!("no way from {here} to {target}: @go {town}"));
                tracing::info!(from = %here, to = %obs.map.name, "used @go");
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn died_on_the_way(&mut self, map: &str, attacker: Option<String>) -> Outcome {
        let cause = attacker.unwrap_or_else(|| "unknown attacker".into());
        *self
            .memory
            .deaths_by_cause
            .entry(cause.clone())
            .or_default() += 1;
        self.memory
            .note(format!("died to {cause} on the way through {map}"));
        self.deadly_ways.retain(|(m, _)| m != map);
        self.deadly_ways.push((map.to_owned(), Instant::now()));
        Outcome::failed(format!("died on the way through {map} ({cause})"))
    }

    /// Talks the Training Grounds receptionist (on `new_1-2`) into sending the bot to a town. One-way.
    pub async fn exit_training(&mut self) -> Result<Outcome> {
        let obs = self.observe().await?;
        if !obs.map.name.starts_with("new_") {
            return Ok(Outcome::Done);
        }
        let receptionist = obs
            .npcs
            .iter()
            .find(|n| n.name.to_lowercase().contains("receptionist"))
            .map(|n| n.id);
        let Some(npc) = receptionist else {
            // Not on new_1-2 yet: walk there.
            let avoid = HashSet::new();
            let route = self
                .knowledge
                .graph
                .route(&obs.map.name, "new_1-2", &avoid)
                .await?;
            let Some(edge) = route.and_then(|r| r.into_iter().next()) else {
                return Ok(Outcome::failed(format!(
                    "no way from {} to the receptionist",
                    obs.map.name
                )));
            };
            return self.take_warp(&obs, &edge).await;
        };
        let result = self.act(Action::Use { target: npc }).await?;
        let intent = Intent {
            goal: "Leave the training grounds and go straight to a town of Ragnarok Online".into(),
            keywords: vec!["direct access".into(), "ragnarok online".into()],
            text_answer: Some(self.name.clone()),
            ..Default::default()
        };
        let end = self.run_dialog(dialog_of(&result), &intent).await?;
        // The warp lands a moment after the dialogue closes.
        let mut obs = self.observe().await?;
        for _ in 0..10 {
            if !obs.map.name.starts_with("new_") {
                break;
            }
            Bot::pause(300).await;
            obs = self.observe().await?;
        }
        if obs.map.name.starts_with("new_") {
            return Ok(Outcome::failed(format!(
                "still in the training grounds after the receptionist ({end:?})"
            )));
        }
        if let DialogEnd::GaveUp(why) = &end {
            tracing::debug!(%why, "receptionist dialogue closed early but the bot left");
        }
        self.memory.note(format!(
            "left the training grounds, arrived in {}",
            obs.map.name
        ));
        self.memory.visited_maps.insert(obs.map.name.clone());
        if let DialogEnd::Shop = end {
            self.act(Action::DialogClose).await.ok();
        }
        Ok(Outcome::Done)
    }
}

#[cfg(test)]
mod tests {
    use super::{avoided_strays, first_door, leg_toward, matching_warp};

    #[test]
    fn a_warp_inside_a_map_matches_only_where_it_stands() {
        use crate::api::Observation;
        use crate::api::observation::Warp;
        use crate::knowledge::graph::Edge;
        let mut obs = Observation::default();
        obs.map.name = "yuno_in05".into();
        // yun76, the way into the Heart's room, is the only warp the observation shows
        obs.warps.push(Warp {
            id: 76,
            x: 31,
            y: 167,
            to_map: "yuno_in05".into(),
            ..Default::default()
        });
        let random = Edge {
            from: "yuno_in05".into(),
            x: 153,
            y: 141,
            to: "yuno_in05".into(),
            random: true,
            ..Default::default()
        };
        assert!(matching_warp(&obs, &random).is_none());
        let yun76 = Edge {
            x: 31,
            y: 168,
            ..random.clone()
        };
        assert_eq!(matching_warp(&obs, &yun76).map(|w| w.id), Some(76));
        // A warp to another map still matches the nearest of the observation's
        obs.warps[0].to_map = "yuno_in02".into();
        let out = Edge {
            x: 40,
            y: 160,
            to: "yuno_in02".into(),
            random: false,
            ..random.clone()
        };
        assert_eq!(matching_warp(&obs, &out).map(|w| w.id), Some(76));
        // A random warp to another map does not: other warps lead there as well
        let random_out = Edge {
            random: true,
            ..out
        };
        assert!(matching_warp(&obs, &random_out).is_none());
    }

    #[test]
    fn a_warp_stepped_off_is_avoided_by_the_trips() {
        let now = std::time::Instant::now();
        let edges = vec![
            ("a".to_owned(), now),
            ("b".to_owned(), now),
            ("a".to_owned(), now),
        ];
        assert_eq!(
            avoided_strays(&edges),
            std::collections::HashSet::from(["a".to_owned(), "b".to_owned()])
        );
        assert!(avoided_strays(&[]).is_empty());
    }

    #[test]
    fn a_leg_goes_part_of_the_way() {
        assert_eq!(leg_toward((102, 153), (15, 234), 10), (92, 163));
        assert_eq!(leg_toward((102, 153), (105, 140), 10), (105, 143));
        assert_eq!(leg_toward((5, 5), (5, 5), 10), (5, 5));
    }

    #[test]
    fn the_first_door_leads_on_the_shortest_way_between_rooms() {
        // 1 -> 2 at (10, 10), 2 -> 3 at (20, 20), 1 -> 4 at (30, 30), 4 -> 5 at (40, 40), 2 -> 1 at (11, 11).
        let doors = [
            (1, (10, 10), 2),
            (2, (20, 20), 3),
            (1, (30, 30), 4),
            (4, (40, 40), 5),
            (2, (11, 11), 1),
        ];
        assert_eq!(first_door(&doors, 1, 3), Some((10, 10)));
        assert_eq!(first_door(&doors, 1, 5), Some((30, 30)));
        assert_eq!(first_door(&doors, 3, 1), None);
        assert_eq!(first_door(&doors, 2, 4), Some((11, 11)));
    }
}
