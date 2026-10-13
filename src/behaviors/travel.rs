//! Walking from map to map along the warp graph, and leaving the training grounds.

use std::collections::HashSet;

use anyhow::Result;
use serde_json::Value;
use tokio::task::JoinHandle;

use super::dialog::{DialogEnd, Intent, dialog_of};
use crate::api::observation::Warp;
use crate::api::{Action, Observation, dist};
use crate::bot::{Bot, Outcome};
use crate::goap::state::HP_LOW;
use crate::knowledge::graph::Edge;
use crate::knowledge::grounds::lethal_monster;

/// Most warps one trip takes before giving up.
const MAX_LEGS: usize = 30;
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
    "prontera", "geffen", "payon", "alberta", "izlude", "aldebaran", "comodo", "yuno", "amatsu", "gonryun", "umbala",
    "niflheim", "louyang", "jawaii", "ayothaya", "einbroch", "lighthalzen", "einbech", "hugel", "rachel", "veins",
    "moscovia",
];
/// Maps around the destination searched for a town to `@go` to.
const GO_SEARCH_HOPS: u32 = 6;
/// Start of the failure reason of a warp in another part of the map (another room of an interior map).
const UNREACHABLE: &str = "unreachable from here";

/// How a guarded walk to a warp ended.
enum Walk {
    /// The walk returned (`map_changed`, `arrived`, `stopped`, `timeout`) or was refused.
    Ended(Result<Value>),
    MapChanged,
    Dead { attacker: Option<String> },
    /// Something the bot can beat attacks it: fight back, then walk on.
    FightBack { target: u64, name: String },
    /// A new goal, or someone talking to the bot: it stops where it is.
    Preempted,
}

fn journey(result: &Value) -> &str {
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

/// The warp of the observation that matches a warp of the database: same destination, nearest position.
fn matching_warp<'a>(obs: &'a Observation, edge: &Edge) -> Option<&'a Warp> {
    obs.warps.iter().filter(|w| w.to_map == edge.to).min_by_key(|w| dist((w.x, w.y), (edge.x, edge.y)))
}

impl Bot {
    /// The warp from `here` into the room of `target` where `spot` is: one landing in the spot's walkable region,
    /// else the one landing nearest to it.
    async fn door_into(&self, here: &str, target: &str, spot: (u16, u16), skip: &HashSet<String>) -> Result<Option<Edge>> {
        let doors: Vec<Edge> =
            self.knowledge.graph.edges(here).await?.into_iter().filter(|e| e.to == target && !skip.contains(&e.key())).collect();
        if let Ok(data) = self.knowledge.db.map(target).await {
            let region = data.npcs.iter().filter(|n| dist((n.x, n.y), spot) <= 2).min_by_key(|n| dist((n.x, n.y), spot)).and_then(|n| n.region);
            if let Some(region) = region {
                let lands_there = |e: &&Edge| {
                    data.arrivals.iter().any(|a| a.from.map.id_str() == here && a.from.x == e.x && a.from.y == e.y && a.land.region == Some(region))
                };
                if let Some(e) = doors.iter().find(lands_there) {
                    return Ok(Some(e.clone()));
                }
            }
        }
        Ok(doors.into_iter().min_by_key(|e| dist((e.to_x, e.to_y), spot)))
    }

    /// Goes into the part `region` of `map` by any warps, out of the map and back in through another door when the
    /// rooms do not connect inside (Nami's room and the Chivalry Member's on `prt_in`).
    pub async fn travel_into(&mut self, map: &str, region: u32) -> Result<Outcome> {
        let mut skip: HashSet<String> = HashSet::new();
        let avoid: HashSet<String> = self.memory.strategy.avoid_maps.iter().cloned().collect();
        for _ in 0..MAX_LEGS {
            if self.preempted() {
                return Ok(Outcome::Preempted);
            }
            let obs = self.observe().await?;
            if obs.map.name == map && obs.me.region.is_none_or(|r| r == region) {
                return Ok(Outcome::Done);
            }
            let route = self.knowledge.graph.route_into(&obs.map.name, obs.me.region, map, Some(region), &avoid, &skip).await?;
            let Some(edge) = route.as_ref().and_then(|r| r.first()) else {
                return Ok(Outcome::failed(format!("no known way into that room of {map}")));
            };
            tracing::info!(from = %obs.map.name, to = %edge.to, room = region, "into a room");
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
        Ok(Outcome::failed(format!("too many warps on the way into {map}")))
    }

    /// Walks to another room (region) of the map the bot is on, through the warps between its rooms. True once there.
    pub async fn reach_region(&mut self, target: u32) -> Result<bool> {
        for _ in 0..8 {
            let obs = self.observe().await?;
            let Some(at) = obs.me.region else { return Ok(false) };
            if at == target {
                return Ok(true);
            }
            let here = obs.map.name.clone();
            let Ok(data) = self.knowledge.db.map(&here).await else { return Ok(false) };
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
            let action = match obs.warps.iter().find(|w| w.region.is_none_or(|r| r == at) && dist((w.x, w.y), first) <= 1) {
                Some(w) => Action::Use { target: w.id },
                None => Action::Move { x: first.0, y: first.1, wait: true },
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
        // Warps in another room than the bot's, left out for this trip only.
        let mut skip: HashSet<String> = HashSet::new();
        // Interior maps are several rooms: into the shop's map, take the warp that lands nearest to the shop.
        let near = match &self.travel_spot {
            Some((map, spot)) if map == target => Some(*spot),
            _ => self.memory.potion_shop.as_ref().filter(|s| s.map == target).map(|s| (s.x, s.y)),
        };
        // Maps on the way with an aggressive monster that kills in two hits, avoided when another way exists.
        let mut deadly: HashSet<String> = HashSet::new();
        let mut checked: HashSet<String> = HashSet::new();
        let fighter = self.knowledge.fighter(&self.observe().await?).await?;
        for _ in 0..MAX_LEGS {
            if self.preempted() {
                return Ok(Outcome::Preempted);
            }
            let obs = self.observe().await?;
            let here = obs.map.name.clone();
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
            let mut route = None;
            for _ in 0..5 {
                let around: HashSet<String> = avoid.union(&deadly).cloned().collect();
                let Some(r) = self.knowledge.graph.route_skipping(&here, obs.me.region, target, &around, &skip).await? else { break };
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
                route = self.knowledge.graph.route_skipping(&here, obs.me.region, target, &avoid, &skip).await?;
            }
            let Some(mut route) = route else {
                if !went && self.go_near(target, &here).await? {
                    went = true;
                    continue;
                }
                return Ok(Outcome::failed(format!("no known route from {here} to {target}")));
            };
            if let (1, Some(near)) = (route.len(), near)
                && let Some(e) = self.door_into(&here, target, near, &skip).await?
            {
                route = vec![e];
            }
            let Some(edge) = route.first() else { return Ok(Outcome::Done) };
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
        Ok(Outcome::failed(format!("too many warps on the way to {target}")))
    }

    /// Steps on the warp of `edge`, from the map the bot is on. Attacked on the way, it fights back what it can beat
    /// and flees from the rest: walking on to the warp leaves the monster behind.
    async fn take_warp(&mut self, obs: &Observation, edge: &Edge) -> Result<Outcome> {
        let from = obs.map.name.clone();
        // A warp inside the map lands on another part of it, not another map
        let (inner, start_region) = (edge.to == from, obs.me.region);
        let crossed = |obs: &Observation| obs.map.name != from || inner && obs.me.region != start_region;
        let (mut tries, mut attacked_walks, mut fights) = (0, 0, 0);
        while tries < WARP_TRIES && attacked_walks < ATTACKED_WALKS {
            let obs = self.observe().await?;
            if obs.me.dead {
                return Ok(Outcome::failed("dead"));
            }
            if crossed(&obs) {
                return Ok(Outcome::Done);
            }
            let action = match matching_warp(&obs, edge) {
                Some(w) => Action::Use { target: w.id },
                // A warp the observation does not show (an NPC warp of the database): walk onto its cell.
                None => Action::Move { x: edge.x, y: edge.y, wait: true },
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
                Walk::Ended(Ok(r)) => tracing::debug!(journey = journey(&r), "walk to warp not finished"),
                // The warp is there, but not in the part of the map the bot is in.
                Walk::Ended(Err(error)) if error.to_string().contains("No reachable cell") => {
                    return Ok(Outcome::failed(format!("{UNREACHABLE}: the warp to {} at ({}, {})", edge.to, edge.x, edge.y)));
                }
                Walk::Ended(Err(error)) => {
                    tracing::warn!(%error, "walk to warp refused");
                    Bot::pause(500).await;
                }
            }
        }
        let obs = self.observe().await?;
        if crossed(&obs) {
            return Ok(Outcome::Done);
        }
        // Every walk ended without a warp and nothing got in the way: there is no usable warp there (a guessed way
        // back, or an entrance behind a quest NPC). Routes leave it out from now on. When a guessed way out fails,
        // the map's other guesses are no better (moc_fild20 has guards instead of warps): they all go.
        if tries >= WARP_TRIES && attacked_walks == 0 {
            let (key, note) = if edge.guessed {
                (Edge::guessed_key(&from), format!("the guessed ways out of {from} do not work"))
            } else {
                (edge.key(), format!("no usable warp to {} at ({}, {}) on {from}", edge.to, edge.x, edge.y))
            };
            if !self.memory.bad_warps.contains(&key) {
                self.memory.bad_warps.push(key.clone());
                self.memory.note(note);
            }
            self.knowledge.graph.forbid([key]);
        }
        Ok(Outcome::failed(format!("could not reach the warp to {} at ({}, {})", edge.to, edge.x, edge.y)))
    }

    /// Watches a walk until it ends, the map changes, the bot dies, or an attacker worth fighting shows up. Weaker
    /// attacks are fled from: the walk goes on, with healing when HP runs low. The flag tells whether the bot was hit.
    async fn guard_walk(&mut self, from: &str, mut walk: JoinHandle<Result<Value>>, may_fight: bool) -> Result<(Walk, bool)> {
        let mut last_hp = self.observe().await?.me.hp;
        let mut hit = false;
        let mut attacker: Option<String> = None;
        loop {
            if walk.is_finished() {
                let result = (&mut walk).await.unwrap_or_else(|e| Err(anyhow::anyhow!("walk task failed: {e}")));
                return Ok((Walk::Ended(result), hit));
            }
            Bot::pause(WALK_POLL_MS).await;
            let obs = self.observe().await?;
            let me = obs.pos();
            if obs.me.dead {
                walk.abort();
                // Killed between two checks (a monster far above the bot's level): the nearest one did it.
                let attacker = attacker.or_else(|| obs.mobs.iter().min_by_key(|m| dist((m.x, m.y), me)).map(|m| m.name.clone()));
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
                && let Some(heal) = self.knowledge.heal_action(&obs)
            {
                self.act(heal.action).await.ok();
            }
            let Some(mob) = obs.mobs.iter().filter(|m| dist((m.x, m.y), me) <= ATTACKER_RANGE).min_by_key(|m| dist((m.x, m.y), me))
            else {
                continue;
            };
            let (target, name) = (mob.id, mob.name.clone());
            let beatable = match self.mob_info(mob.mob_id).await {
                Some(info) => {
                    let fighter = self.knowledge.fighter(&obs).await?;
                    fighter.can_fight(&info) && obs.me.hp_frac() - fighter.assess(&info).danger >= FIGHT_MARGIN
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
        let Some((town, _)) = self.knowledge.graph.nearest_upstream(target, GO_TOWNS, GO_SEARCH_HOPS).await? else {
            tracing::info!(%target, "no town leads to the destination, not using @go");
            return Ok(false);
        };
        if town == here {
            return Ok(false);
        }
        if let Err(error) = self.act(Action::Command { text: format!("@go {town}") }).await {
            tracing::warn!(%error, %town, "@go refused");
            return Ok(false);
        }
        for _ in 0..10 {
            Bot::pause(500).await;
            let obs = self.observe().await?;
            if obs.map.name != here {
                self.memory.note(format!("no way from {here} to {target}: @go {town}"));
                tracing::info!(from = %here, to = %obs.map.name, "used @go");
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn died_on_the_way(&mut self, map: &str, attacker: Option<String>) -> Outcome {
        let cause = attacker.unwrap_or_else(|| "unknown attacker".into());
        *self.memory.deaths_by_cause.entry(cause.clone()).or_default() += 1;
        self.memory.note(format!("died to {cause} on the way through {map}"));
        Outcome::failed(format!("died on the way through {map} ({cause})"))
    }

    /// Talks the Training Grounds receptionist (on `new_1-2`) into sending the bot to a town. One-way.
    pub async fn exit_training(&mut self) -> Result<Outcome> {
        let obs = self.observe().await?;
        if !obs.map.name.starts_with("new_") {
            return Ok(Outcome::Done);
        }
        let receptionist = obs.npcs.iter().find(|n| n.name.to_lowercase().contains("receptionist")).map(|n| n.id);
        let Some(npc) = receptionist else {
            // Not on new_1-2 yet: walk there.
            let avoid = HashSet::new();
            let route = self.knowledge.graph.route(&obs.map.name, "new_1-2", &avoid).await?;
            let Some(edge) = route.and_then(|r| r.into_iter().next()) else {
                return Ok(Outcome::failed(format!("no way from {} to the receptionist", obs.map.name)));
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
            return Ok(Outcome::failed(format!("still in the training grounds after the receptionist ({end:?})")));
        }
        if let DialogEnd::GaveUp(why) = &end {
            tracing::debug!(%why, "receptionist dialogue closed early but the bot left");
        }
        self.memory.note(format!("left the training grounds, arrived in {}", obs.map.name));
        self.memory.visited_maps.insert(obs.map.name.clone());
        if let DialogEnd::Shop = end {
            self.act(Action::DialogClose).await.ok();
        }
        Ok(Outcome::Done)
    }
}

#[cfg(test)]
mod tests {
    use super::first_door;

    #[test]
    fn the_first_door_leads_on_the_shortest_way_between_rooms() {
        // 1 -> 2 at (10, 10), 2 -> 3 at (20, 20), 1 -> 4 at (30, 30), 4 -> 5 at (40, 40), 2 -> 1 at (11, 11).
        let doors = [(1, (10, 10), 2), (2, (20, 20), 3), (1, (30, 30), 4), (4, (40, 40), 5), (2, (11, 11), 1)];
        assert_eq!(first_door(&doors, 1, 3), Some((10, 10)));
        assert_eq!(first_door(&doors, 1, 5), Some((30, 30)));
        assert_eq!(first_door(&doors, 3, 1), None);
        assert_eq!(first_door(&doors, 2, 4), Some((11, 11)));
    }
}
