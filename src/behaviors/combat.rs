//! Fighting one monster at a time, healing during the fight, picking up the drops, and resting.

use std::time::{Duration, Instant};

use anyhow::Result;
use rand::Rng;

use crate::api::{Action, Observation, dist};
use crate::bot::{Bot, Outcome};
use crate::goap::state::{HP_FULL, HP_LOW};
use crate::party::{member_ids, taken_by_others};

const FIGHT_TIMEOUT: Duration = Duration::from_secs(40);
const POLL_MS: u64 = 400;
/// HP share under which a potion is drunk during a fight.
const HP_POTION: f32 = 0.35;
/// Items this close to the kill are picked up after the fight.
const LOOT_RADIUS: u32 = 9;
/// Items this close to the bot are picked up before it looks for the next target.
const SWEEP_RADIUS: u32 = 12;
const REST_TIMEOUT: Duration = Duration::from_secs(150);
/// Search walks in a row with nothing to fight before a hunt gives up.
const MAX_SEARCHES: u32 = 8;
/// Largest step of a search walk on each axis, in cells.
const SEARCH_STEP: i32 = 30;
/// Search walks keep this far from the map edge, where the warps are.
const MAP_MARGIN: i32 = 12;
/// How much more a monster that attacks a party member is worth fighting.
const ASSIST_BONUS: f32 = 4.0;
/// Monsters and items this close to a warp are left alone: going for them can step on the warp.
const WARP_MARGIN: u16 = 2;

/// Offensive skills worth opening a fight with, best first, with their SP cost at level 1 or so.
const OPENERS: [(&str, u32); 8] = [
    ("MG_FIREBOLT", 12),
    ("MG_COLDBOLT", 12),
    ("MG_LIGHTNINGBOLT", 12),
    ("MG_SOULSTRIKE", 18),
    ("SM_BASH", 8),
    ("AC_DOUBLE", 12),
    ("MC_MAMMONITE", 5),
    ("TF_POISON", 12),
];

/// Whether the job's attack is its spells: a Mage's staff hits for little.
fn casts_to_fight(job: &str) -> bool {
    matches!(
        crate::knowledge::gear::normal_job(job).as_str(),
        "mage" | "wizard" | "sage"
    )
}

impl Bot {
    /// Picks the best monster to fight on this map, if any; within `area` (a cell and a radius) when given. Monsters
    /// another player fights are theirs; those that attack a party member come first.
    pub async fn pick_target(
        &mut self,
        obs: &Observation,
        area: Option<((u16, u16), u32)>,
    ) -> Result<Option<(u64, String)>> {
        let fighter = self.knowledge.fighter(obs).await?;
        let me = obs.pos();
        let avoid: Vec<String> = self
            .memory
            .strategy
            .avoided_mobs(obs.me.base_level)
            .iter()
            .chain(&self.memory.losing_to(obs.me.base_level))
            .map(|s| s.to_lowercase())
            .collect();
        let members = member_ids(obs);
        let mut best: Option<(f32, u64, String)> = None;
        let mobs = obs.mobs.clone();
        for mob in &mobs {
            if mob.hp == 0
                || self.is_blacklisted(mob.id)
                || avoid.contains(&mob.name.to_lowercase())
            {
                continue;
            }
            if taken_by_others(obs, mob.id, mob.target, &members) {
                continue;
            }
            if area.is_some_and(|(center, radius)| dist(center, (mob.x, mob.y)) > radius) {
                continue;
            }
            if obs.near_warp((mob.x, mob.y), WARP_MARGIN) {
                continue;
            }
            let Some(info) = self.mob_info(mob.mob_id).await else {
                continue;
            };
            if !fighter.can_fight(&info) {
                continue;
            }
            // Steer clear of targets standing next to monsters that would join in and are too strong.
            let mut crowded = false;
            for other in mobs
                .iter()
                .filter(|o| o.id != mob.id && dist((o.x, o.y), (mob.x, mob.y)) <= 5)
            {
                if let Some(o) = self.mob_info(other.mob_id).await
                    && o.aggressive()
                    && !fighter.can_fight(&o)
                {
                    crowded = true;
                }
            }
            if crowded {
                continue;
            }
            let a = fighter.assess(&info);
            let d = dist(me, (mob.x, mob.y)) as f32;
            // Walking costs ~0.15 s a cell: rank by experience per second including the walk.
            let mut value =
                info.exp as f32 / (a.kill_secs + d * 0.15 + 2.0) * (1.0 - a.danger * 0.5);
            if mob
                .target
                .is_some_and(|t| t != obs.me.char_id && members.contains(&t))
            {
                value *= ASSIST_BONUS;
            }
            if self.wanted_mobs.contains(&mob.mob_id) {
                value *= crate::behaviors::town_quests::WANTED_BONUS;
            }
            if best.as_ref().is_none_or(|(v, _, _)| value > *v) {
                best = Some((value, mob.id, mob.name.clone()));
            }
        }
        Ok(best.map(|(_, id, name)| (id, name)))
    }

    /// One fight on the current map, then the loot. `Failed` when there is nothing to fight or the bot died.
    pub async fn hunt_once(&mut self) -> Result<Outcome> {
        // Drops land a moment after a kill, and some fall out of reach of the first pick-up: sweep first.
        let pos = self.observe().await?.pos();
        self.loot_near(pos, SWEEP_RADIUS).await?;
        let obs = self.observe().await?;
        let Some((target, name)) = self.pick_target(&obs, None).await? else {
            // The observation only shows monsters in sight: walk somewhere else on the map before giving up.
            self.empty_searches += 1;
            if self.empty_searches > MAX_SEARCHES {
                self.empty_searches = 0;
                return Ok(if obs.mobs.is_empty() {
                    Outcome::failed(format!("no monsters on {}", obs.map.name))
                } else {
                    Outcome::failed(format!(
                        "nothing safe to fight on {} ({} monsters)",
                        obs.map.name,
                        obs.mobs.len()
                    ))
                });
            }
            return self.search(&obs).await;
        };
        self.empty_searches = 0;
        self.fight(target, &name).await
    }

    /// Walks to a random spot some distance away, to find monsters out of sight.
    async fn search(&mut self, obs: &Observation) -> Result<Outcome> {
        let (w, h) = (obs.map.width as i32, obs.map.height as i32);
        if w == 0 || h == 0 || obs.mobs.is_empty() {
            // Right after a map change the observation can come without the map size or its monsters yet.
            Bot::pause(1000).await;
            return Ok(Outcome::Done);
        }
        let (x, y) = (obs.me.x as i32, obs.me.y as i32);
        for _ in 0..4 {
            let (dx, dy) = {
                let mut rng = rand::rng();
                (
                    rng.random_range(-SEARCH_STEP..=SEARCH_STEP),
                    rng.random_range(-SEARCH_STEP..=SEARCH_STEP),
                )
            };
            let tx = (x + dx).clamp(MAP_MARGIN, (w - MAP_MARGIN).max(MAP_MARGIN)) as u16;
            let ty = (y + dy).clamp(MAP_MARGIN, (h - MAP_MARGIN).max(MAP_MARGIN)) as u16;
            if obs.near_warp((tx, ty), WARP_MARGIN) {
                continue;
            }
            if self
                .act(Action::Move {
                    x: tx,
                    y: ty,
                    wait: true,
                })
                .await
                .is_ok()
            {
                tracing::debug!(x = tx, y = ty, "searching for monsters");
                return Ok(Outcome::Done);
            }
        }
        Ok(Outcome::failed(format!(
            "cannot walk anywhere on {}",
            obs.map.name
        )))
    }

    /// Fights `target` until it is gone, the bot dies or the fight times out.
    pub async fn fight(&mut self, target: u64, name: &str) -> Result<Outcome> {
        let start = Instant::now();
        if let Err(error) = self
            .act(Action::Attack {
                target,
                wait: false,
            })
            .await
        {
            self.blacklist(target, 120);
            return Ok(Outcome::failed(format!("cannot attack {name}: {error}")));
        }
        let mut opened = false;
        // A caster's bolts are its attack, not just an opener: cast whenever the last cast is over and SP allows
        let caster = casts_to_fight(&self.observe().await?.me.job);
        let mut last_pos = None;
        let mut last_hp = None;
        let mut stalled_since = Instant::now();
        loop {
            Bot::pause(POLL_MS).await;
            let obs = self.observe().await?;
            if obs.me.dead {
                *self
                    .memory
                    .deaths_by_cause
                    .entry(name.to_owned())
                    .or_default() += 1;
                self.memory.record_fight(name, obs.me.base_level, false);
                if self
                    .memory
                    .losing_to(obs.me.base_level)
                    .iter()
                    .any(|m| m == name)
                {
                    self.memory.note(format!(
                        "{name} keeps killing me: leaving it alone for a few levels"
                    ));
                }
                self.memory
                    .note(format!("died fighting {name} on {}", obs.map.name));
                return Ok(Outcome::failed(format!("died fighting {name}")));
            }
            let Some(mob) = obs.mobs.iter().find(|m| m.id == target) else {
                self.memory.counters.kills += 1;
                *self.memory.kills_by_mob.entry(name.to_owned()).or_default() += 1;
                self.memory.record_fight(name, obs.me.base_level, true);
                let pos = last_pos.unwrap_or(obs.pos());
                Bot::pause(500).await;
                self.loot_near(pos, LOOT_RADIUS).await?;
                return Ok(Outcome::Done);
            };
            last_pos = Some((mob.x, mob.y));
            // A new goal, or someone talking to the bot, while it walks to a monster that is not fighting yet: that first
            if self.preempted()
                && mob.target != Some(obs.me.char_id)
                && dist(obs.pos(), (mob.x, mob.y)) > 3
            {
                self.act(Action::Stop).await.ok();
                return Ok(Outcome::Preempted);
            }
            if last_hp != Some(mob.hp) {
                last_hp = Some(mob.hp);
                stalled_since = Instant::now();
            }
            if obs.me.hp_frac() < HP_POTION
                && let Some(heal) = self.knowledge.heal_action(&obs, &self.keep)
            {
                self.act(heal.action).await.ok();
                self.act(Action::Attack {
                    target,
                    wait: false,
                })
                .await
                .ok();
                continue;
            }
            if (!opened || caster && obs.me.action != "using_skill")
                && dist(obs.pos(), (mob.x, mob.y)) <= 9
            {
                opened = true;
                if let Some((skill, _)) = OPENERS
                    .iter()
                    .find(|(s, sp)| obs.skill_level(s) > 0 && obs.me.sp >= *sp)
                {
                    self.act(Action::Skill {
                        skill: (*skill).to_owned(),
                        target: Some(target),
                    })
                    .await
                    .ok();
                    Bot::pause(600).await;
                    self.act(Action::Attack {
                        target,
                        wait: false,
                    })
                    .await
                    .ok();
                }
            }
            if obs.me.action != "attacking"
                && !obs.me.moving
                && stalled_since.elapsed() > Duration::from_secs(3)
            {
                // The fight stopped on its own (path blocked, target moved away): try again.
                if self
                    .act(Action::Attack {
                        target,
                        wait: false,
                    })
                    .await
                    .is_err()
                {
                    self.blacklist(target, 120);
                    return Ok(Outcome::failed(format!("lost {name}")));
                }
            }
            if start.elapsed() > FIGHT_TIMEOUT || stalled_since.elapsed() > Duration::from_secs(15)
            {
                self.act(Action::Stop).await.ok();
                self.blacklist(target, 300);
                return Ok(Outcome::failed(format!("fight with {name} took too long")));
            }
        }
    }

    /// Picks up the items lying within `radius` of `pos`, nearest to the bot first. Items that can not be picked
    /// up (locked to another player, too heavy) are not tried again for a while.
    async fn loot_near(&mut self, pos: (u16, u16), radius: u32) -> Result<()> {
        let obs = self.observe().await?;
        // An item on a warp would take the bot off the map, again each time it comes back.
        let mut items: Vec<_> = obs
            .items
            .iter()
            .filter(|i| dist((i.x, i.y), pos) <= radius && !obs.near_warp((i.x, i.y), WARP_MARGIN))
            .cloned()
            .collect();
        items.retain(|i| !self.is_blacklisted(i.id));
        items.sort_by_key(|i| dist((i.x, i.y), obs.pos()));
        let before: u32 = obs.inventory.iter().map(|i| i.amount).sum();
        let mut tried = 0;
        for item in items.into_iter().take(8) {
            tried += 1;
            if let Err(error) = self.act(Action::Use { target: item.id }).await {
                tracing::debug!(%error, item = %item.name, "pick up refused");
                self.blacklist(item.id, 120);
            }
        }
        if tried > 0 {
            let after: u32 = self
                .observe()
                .await?
                .inventory
                .iter()
                .map(|i| i.amount)
                .sum();
            let got = after.saturating_sub(before);
            self.memory.counters.loot_picked += got as u64;
            tracing::debug!(tried, got, "loot");
        }
        Ok(())
    }

    /// Sits until HP is back (sitting doubles regeneration). Fights back when something attacks meanwhile.
    pub async fn rest(&mut self) -> Result<Outcome> {
        self.act(Action::Stop).await.ok();
        self.act(Action::Sit).await.ok();
        let outcome = self.sit_out().await;
        self.act(Action::Stand).await.ok();
        outcome
    }

    async fn sit_out(&mut self) -> Result<Outcome> {
        let start = Instant::now();
        let first = self.observe().await?;
        let mut last_hp = first.me.hp;
        let from = first.me.hp_frac();
        while start.elapsed() < REST_TIMEOUT {
            Bot::pause(2000).await;
            let obs = self.observe().await?;
            if obs.me.dead {
                return Ok(Outcome::failed("died while resting"));
            }
            if obs.me.hp_frac() >= HP_FULL {
                return Ok(Outcome::Done);
            }
            if obs.me.hp < last_hp {
                // Something hits the bot: fight back against what attacks it, else the nearest monster.
                let me = obs.pos();
                let attacker = obs
                    .mobs
                    .iter()
                    .find(|m| m.target == Some(obs.me.char_id))
                    .or_else(|| {
                        obs.mobs
                            .iter()
                            .filter(|m| dist((m.x, m.y), me) <= 3)
                            .min_by_key(|m| dist((m.x, m.y), me))
                    });
                if let Some(m) = attacker {
                    let (id, name) = (m.id, m.name.clone());
                    self.act(Action::Stand).await.ok();
                    if obs.me.hp_frac() < HP_POTION
                        && let Some(heal) = self.knowledge.heal_action(&obs, &self.keep)
                    {
                        self.act(heal.action).await.ok();
                    }
                    tracing::info!(%name, "attacked while resting, fighting back");
                    return self.fight(id, &name).await;
                }
            }
            last_hp = obs.me.hp;
            // Natural regeneration is slow: heal with skills (SP comes back by itself), and with items when HP is low.
            // Neither works sitting down.
            if let Some(heal) = self.knowledge.heal_action(&obs, &self.keep)
                && (heal.skill || obs.me.hp_frac() < HP_LOW)
            {
                self.act(Action::Stand).await.ok();
                self.act(heal.action).await.ok();
                Bot::pause(500).await;
                self.act(Action::Sit).await.ok();
                last_hp = self.observe().await?.me.hp;
            }
        }
        // Slow regeneration: enough HP to fight again is enough to go on, and so is HP that came back some way.
        let hp = self.observe().await?.me.hp_frac();
        if hp >= HP_LOW || hp >= from + 0.2 {
            return Ok(Outcome::Done);
        }
        Ok(Outcome::failed("HP did not come back in time"))
    }
}
