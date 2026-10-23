//! Hunting grounds: maps near the bot ranked by the experience it can safely make there.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use anyhow::Result;

use super::Knowledge;
use super::combat::{Fighter, SAFE_DANGER};
use super::db::{Db, ItemData, MapData, MobData};

#[derive(Debug, Clone)]
pub struct Ground {
    pub map: String,
    pub hops: u32,
    /// Experience per second of the monsters the bot can fight there, weighted by how many spawn.
    pub exp_rate: f32,
    /// Share of the spawns that are aggressive and too strong.
    pub threat: f32,
    pub score: f32,
    /// `(name, level, amount)` of the safe targets, most common first.
    pub targets: Vec<(String, u32, u32)>,
}

impl Ground {
    /// One line for the strategist.
    pub fn describe(&self) -> String {
        let mobs: Vec<String> = self
            .targets
            .iter()
            .take(4)
            .map(|(n, l, a)| format!("{n} lv{l} x{a}"))
            .collect();
        format!(
            "{} ({} warps away, exp/s {:.2}, threat {:.0}%): {}",
            self.map,
            self.hops,
            self.exp_rate,
            self.threat * 100.0,
            mobs.join(", ")
        )
    }
}

/// What a search for hunting grounds is asked: who hunts, from where, how far it goes and what it keeps away from.
#[derive(Clone, Copy)]
pub struct Hunt<'a> {
    pub knowledge: &'a Knowledge,
    pub fighter: &'a Fighter,
    /// The map the bot is on.
    pub from: &'a str,
    /// Most warps to a ground.
    pub max_hops: u32,
    /// Maps not to hunt on or cross.
    pub avoid_maps: &'a HashSet<String>,
    /// Monsters not to fight.
    pub avoid_mobs: &'a HashSet<String>,
}

static NOTHING: LazyLock<HashSet<String>> = LazyLock::new(HashSet::new);

impl<'a> Hunt<'a> {
    /// `fighter` hunting within `max_hops` warps of `from`, avoiding nothing.
    pub fn new(
        knowledge: &'a Knowledge,
        fighter: &'a Fighter,
        from: &'a str,
        max_hops: u32,
    ) -> Self {
        Self {
            knowledge,
            fighter,
            from,
            max_hops,
            avoid_maps: &NOTHING,
            avoid_mobs: &NOTHING,
        }
    }

    /// Keeping off the maps of `maps`.
    pub fn avoiding_maps(self, maps: &'a HashSet<String>) -> Self {
        Self {
            avoid_maps: maps,
            ..self
        }
    }

    /// Leaving the monsters of `mobs` alone.
    pub fn avoiding_mobs(self, mobs: &'a HashSet<String>) -> Self {
        Self {
            avoid_mobs: mobs,
            ..self
        }
    }
}

/// Danger of a fight (`Assessment::danger`, the share of the bot's HP it costs) the bot does not live through.
const LOST_FIGHT: f32 = 1.0;

/// Scores one map. `None` when nothing on it is worth fighting.
pub fn score_map(
    map: &MapData,
    mobs: &[MobData],
    fighter: &Fighter,
    hops: u32,
    avoid_mobs: &HashSet<String>,
) -> Option<Ground> {
    let mut total = 0u32;
    let mut threat_amount = 0u32;
    let mut weighted = 0f32;
    let mut safe_amount = 0u32;
    let mut targets = Vec::new();
    for spawn in &map.mob_spawns {
        let Some(mob) = mobs.iter().find(|m| Some(m.id) == spawn.mob.id_u32()) else {
            continue;
        };
        total += spawn.amount;
        let a = fighter.assess(mob);
        let avoided = avoid_mobs
            .iter()
            .any(|n| n.eq_ignore_ascii_case(&mob.name_english));
        // However few, an aggressive monster that kills in two hits makes the map no place to hunt (Drosera lv 46 on
        // ra_fild08, among Drops and Stapo, killed a lv 28 Swordsman with 548 HP); nor one the bot cannot outlast:
        // the lone Vocal of prt_fild07 (lv 18, 3016 HP, and Rockers it calls) killed 77 novices in an hour.
        if mob.aggressive()
            && !mob.boss()
            && (fighter.lethal_in_two(mob) || a.danger >= LOST_FIGHT)
        {
            return None;
        }
        if mob.boss() || a.danger >= SAFE_DANGER || avoided {
            if mob.aggressive() || mob.boss() {
                threat_amount += spawn.amount;
            }
            continue;
        }
        weighted += a.exp_per_sec * spawn.amount as f32;
        safe_amount += spawn.amount;
        targets.push((mob.name_english.clone(), mob.level, spawn.amount));
    }
    if safe_amount == 0 || total == 0 {
        return None;
    }
    targets.sort_by(|a, b| b.2.cmp(&a.2));
    let exp_rate = weighted / safe_amount as f32;
    let threat = threat_amount as f32 / total as f32;
    // Density matters: a map where most spawns are targets keeps the bot busy.
    let density = (safe_amount as f32 / 100.0).min(1.5);
    let score = exp_rate * (0.5 + density) * (1.0 - threat).powi(3) / (1.0 + 0.15 * hops as f32);
    Some(Ground {
        map: map.name.clone(),
        hops,
        exp_rate,
        threat,
        score,
        targets,
    })
}

/// Rules out the training grounds, arenas, event and quest rooms, whose spawns are not for levelling.
pub fn is_hunting_map_name(name: &str) -> bool {
    !name.contains("_in") && is_explorable_map_name(name)
}

/// A map worth a visit to see it: the hunting maps and the insides of towns, not test, event, arena or guild maps.
pub fn is_explorable_map_name(name: &str) -> bool {
    const SKIP: [&str; 9] = [
        "new_", "arena", "room", "quiz", "job_", "que_", "evt", "pvp", "gld",
    ];
    !SKIP.iter().any(|s| name.contains(s))
}

/// Spawns of a monster on a map, at least, for it to make crossing the map deadly.
const LETHAL_MIN_SPAWNS: u32 = 5;

/// An aggressive monster of `map` that kills the bot in two hits, if any: crossing the map is a gamble.
pub async fn lethal_monster(db: &Db, map: &str, fighter: &Fighter) -> Option<String> {
    let data = db.map(map).await.ok()?;
    let mut amounts: Vec<(u32, u32)> = Vec::new();
    for spawn in &data.mob_spawns {
        let Some(id) = spawn.mob.id_u32() else {
            continue;
        };
        match amounts.iter_mut().find(|(m, _)| *m == id) {
            Some((_, n)) => *n += spawn.amount,
            None => amounts.push((id, spawn.amount)),
        }
    }
    for (id, amount) in amounts {
        // A lone monster on a whole field is rarely met on the way: the Orc Hero, Choco and High Orc of gef_fild02
        // and 03 cut Geffen off from Prontera's fields. Those that killed bots on the way had 20 or more.
        if amount < LETHAL_MIN_SPAWNS {
            continue;
        }
        let Ok(mob) = db.mob(id).await else { continue };
        if mob.aggressive() && fighter.lethal_in_two(&mob) {
            return Some(mob.name_english);
        }
    }
    None
}

impl Hunt<'_> {
    /// Warps from `from` to `to` on a way that crosses no map with an aggressive monster able to kill the bot in two
    /// hits (the destination itself is judged by `score_map`). None when every way crosses one. `lethal` caches the maps
    /// judged.
    pub async fn safe_hops(
        &self,
        to: &str,
        lethal: &mut HashMap<String, bool>,
    ) -> Result<Option<u32>> {
        let Hunt {
            knowledge,
            fighter,
            from,
            avoid_maps,
            ..
        } = *self;
        let (db, graph) = (&knowledge.db, &knowledge.graph);
        if from == to {
            return Ok(Some(0));
        }
        let mut around = avoid_maps.clone();
        for _ in 0..5 {
            let Some(route) = graph.route(from, to, &around).await? else {
                return Ok(None);
            };
            let mut crossed = false;
            for map in route.iter().map(|e| e.to.as_str()).filter(|m| *m != to) {
                let deadly = match lethal.get(map) {
                    Some(d) => *d,
                    None => {
                        let d = lethal_monster(db, map, fighter).await.is_some();
                        lethal.insert(map.to_owned(), d);
                        d
                    }
                };
                if deadly {
                    around.insert(map.to_owned());
                    crossed = true;
                }
            }
            if !crossed {
                return Ok(Some(route.len() as u32));
            }
        }
        Ok(None)
    }

    /// Where to hunt for an item: the map within `max_hops` warps, reached without crossing a deadly map, where the bot can
    /// safely fight what drops it, ranked by drop chance, spawns and distance. The ground and the monster.
    pub async fn farm_ground(&self, item: &ItemData) -> Result<Option<(Ground, String)>> {
        let Hunt {
            knowledge,
            fighter,
            from,
            max_hops,
            avoid_maps,
            avoid_mobs,
        } = *self;
        let (db, graph) = (&knowledge.db, &knowledge.graph);
        // Every monster that drops it, on the maps within reach: the best droppers often live far away (Red Herb:
        // Permeter, Goblin), and the near Lunatic was left out when only the eight best were looked at.
        let near: HashMap<String, u32> = graph
            .neighbourhood(from, max_hops)
            .await?
            .into_iter()
            .collect();
        let mut best: Option<(f32, Ground, String)> = None;
        let mut lethal = HashMap::new();
        let mut safe: HashMap<String, Option<u32>> = HashMap::new();
        for drop in item.dropped_by.iter().filter(|d| !d.mvp && d.rate > 0) {
            let Some(id) = drop.mob.id_u32() else {
                continue;
            };
            let Ok(mob) = db.mob(id).await else { continue };
            for spawn in mob
                .spawns
                .iter()
                .filter(|s| s.kind != "boss" && s.amount > 0)
            {
                let name = spawn.map.id_str();
                if avoid_maps.contains(&name)
                    || !is_hunting_map_name(&name)
                    || !near.contains_key(&name)
                {
                    continue;
                }
                let hops = match safe.get(&name) {
                    Some(hops) => *hops,
                    None => {
                        let hops = self.safe_hops(&name, &mut lethal).await?;
                        safe.insert(name.clone(), hops);
                        hops
                    }
                };
                let Some(hops) = hops else { continue };
                if hops > max_hops {
                    continue;
                }
                let Ok(map) = db.map(&name).await else {
                    continue;
                };
                let mut mobs = Vec::new();
                for s in &map.mob_spawns {
                    if let Some(id) = s.mob.id_u32()
                        && let Ok(m) = db.mob(id).await
                    {
                        mobs.push(m);
                    }
                }
                let Some(ground) = score_map(&map, &mobs, fighter, hops, avoid_mobs) else {
                    continue;
                };
                if !ground
                    .targets
                    .iter()
                    .any(|(n, _, _)| *n == mob.name_english)
                {
                    continue;
                }
                // Every warp is a walk across a map: a few items take less time on a poorer map near by
                let value = drop.rate as f32 * spawn.amount as f32 / (1.0 + hops as f32);
                if best.as_ref().is_none_or(|(v, _, _)| value > *v) {
                    best = Some((value, ground, mob.name_english.clone()));
                }
            }
        }
        Ok(best.map(|(_, g, m)| (g, m)))
    }

    /// `map` as a ground for fighting one of `wanted` (monster names): reached within `max_hops` warps without crossing a
    /// deadly map, and with one of them among its safe targets. A town quest names the map.
    pub async fn ground_on(&self, map: &str, wanted: &[String]) -> Result<Option<Ground>> {
        if self.avoid_maps.contains(map) || !is_hunting_map_name(map) {
            return Ok(None);
        }
        let db = &self.knowledge.db;
        let Some(hops) = self.safe_hops(map, &mut HashMap::new()).await? else {
            return Ok(None);
        };
        if hops > self.max_hops {
            return Ok(None);
        }
        let Ok(data) = db.map(map).await else {
            return Ok(None);
        };
        let mut mobs = Vec::new();
        for spawn in &data.mob_spawns {
            if let Some(id) = spawn.mob.id_u32()
                && let Ok(mob) = db.mob(id).await
            {
                mobs.push(mob);
            }
        }
        Ok(
            score_map(&data, &mobs, self.fighter, hops, self.avoid_mobs).filter(|g| {
                g.targets
                    .iter()
                    .any(|(n, _, _)| wanted.iter().any(|w| w.eq_ignore_ascii_case(n)))
            }),
        )
    }

    /// The best grounds within `max_hops` warps of `from`, on a way that crosses no deadly map (`safe_hops`): the score
    /// of mjo_dun01 said threat 0%, and five bots died on mjolnir_02 on the way.
    pub async fn hunting_grounds(&self, limit: usize) -> Result<Vec<Ground>> {
        let Hunt {
            knowledge,
            fighter,
            from,
            max_hops,
            avoid_maps,
            avoid_mobs,
        } = *self;
        let (db, graph) = (&knowledge.db, &knowledge.graph);
        let mut grounds = Vec::new();
        let mut lethal = HashMap::new();
        for (name, _) in graph.neighbourhood(from, max_hops).await? {
            if avoid_maps.contains(&name) || !is_hunting_map_name(&name) {
                continue;
            }
            let Ok(map) = db.map(&name).await else {
                continue;
            };
            if map.is_town()
                || map.mob_spawns.is_empty()
                || map
                    .flags
                    .iter()
                    .any(|f| f == "pvp" || f == "gvg" || f == "battleground")
            {
                continue;
            }
            let Some(hops) = self.safe_hops(&name, &mut lethal).await? else {
                continue;
            };
            if hops > max_hops {
                continue;
            }
            let mut mobs = Vec::new();
            for spawn in &map.mob_spawns {
                if let Some(id) = spawn.mob.id_u32()
                    && let Ok(mob) = db.mob(id).await
                {
                    mobs.push(mob);
                }
            }
            if let Some(g) = score_map(&map, &mobs, fighter, hops, avoid_mobs) {
                grounds.push(g);
            }
        }
        grounds.sort_by(|a, b| b.score.total_cmp(&a.score));
        grounds.truncate(limit);
        Ok(grounds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::db::{Link, Spawn};

    fn spawn(id: u32, amount: u32) -> Spawn {
        Spawn {
            amount,
            mob: Link {
                id: id.into(),
                ..Default::default()
            },
            kind: "normal".into(),
        }
    }

    fn mob(id: u32, name: &str, level: u32, hp: u32, atk: u32, exp: u32, mode: u32) -> MobData {
        MobData {
            id,
            name_english: name.into(),
            level,
            hp,
            atk1: atk,
            atk2: atk,
            atk_delay: 1500,
            exp,
            mode,
            ..Default::default()
        }
    }

    #[test]
    fn aggressive_strong_spawns_lower_the_score() {
        let me = Fighter {
            level: 5,
            max_hp: 80,
            str: 10,
            agi: 1,
            vit: 5,
            dex: 5,
            luk: 1,
            weapon_atk: 17,
            armor_def: 1,
        };
        let poring = mob(1, "Poring", 1, 50, 8, 2, 0x83);
        // Too strong to hunt, not so strong the fight is lost: that rules the map out
        let brute = mob(2, "Brute", 30, 40, 30, 500, 0x84);
        let calm = MapData {
            name: "calm".into(),
            mob_spawns: vec![spawn(1, 100)],
            ..Default::default()
        };
        let scary = MapData {
            name: "scary".into(),
            mob_spawns: vec![spawn(1, 100), spawn(2, 50)],
            ..Default::default()
        };
        let mobs = vec![poring, brute];
        let a = score_map(&calm, &mobs, &me, 1, &HashSet::new()).unwrap();
        let b = score_map(&scary, &mobs, &me, 1, &HashSet::new()).unwrap();
        assert!(a.score > b.score, "{a:?} {b:?}");
        assert_eq!(b.targets.len(), 1);
    }

    #[test]
    fn a_few_aggressive_monsters_that_kill_in_two_hits_rule_the_map_out() {
        let me = Fighter {
            level: 28,
            max_hp: 548,
            str: 30,
            vit: 12,
            weapon_atk: 40,
            ..Default::default()
        };
        let drops = mob(1, "Drops", 3, 55, 12, 4, 0x83);
        let drosera = mob(2, "Drosera", 46, 7221, 589, 2612, 0x84);
        let map = MapData {
            name: "ra_fild08".into(),
            mob_spawns: vec![spawn(1, 100), spawn(2, 5)],
            ..Default::default()
        };
        assert!(score_map(&map, &[drops.clone(), drosera], &me, 0, &HashSet::new()).is_none());
        // A passive one does not.
        let calm = mob(2, "Drosera", 46, 7221, 589, 2612, 0x81);
        assert!(score_map(&map, &[drops, calm], &me, 0, &HashSet::new()).is_some());
    }

    #[test]
    fn a_lone_aggressive_monster_the_bot_cannot_outlast_rules_the_map_out() {
        // Zykemar, killed by the Vocal of prt_fild07
        let me = Fighter {
            level: 18,
            max_hp: 264,
            str: 22,
            agi: 15,
            vit: 7,
            dex: 14,
            luk: 1,
            weapon_atk: 17,
            armor_def: 1,
        };
        let rocker = mob(1, "Rocker", 9, 198, 24, 20, 0x81);
        let vocal = mob(2, "Vocal", 18, 3016, 76, 110, 0x3695);
        assert!(!me.lethal_in_two(&vocal));
        let map = MapData {
            name: "prt_fild07".into(),
            mob_spawns: vec![spawn(1, 80), spawn(2, 1)],
            ..Default::default()
        };
        assert!(score_map(&map, &[rocker.clone(), vocal], &me, 0, &HashSet::new()).is_none());
        // Rockers alone are fine.
        let calm = MapData {
            mob_spawns: vec![spawn(1, 80)],
            ..map
        };
        assert!(score_map(&calm, &[rocker], &me, 0, &HashSet::new()).is_some());
    }

    #[test]
    fn avoided_mobs_are_not_targets() {
        let me = Fighter {
            level: 5,
            max_hp: 80,
            str: 10,
            weapon_atk: 17,
            ..Default::default()
        };
        let mobs = vec![mob(1, "Poring", 1, 50, 8, 2, 0x83)];
        let map = MapData {
            name: "m".into(),
            mob_spawns: vec![spawn(1, 10)],
            ..Default::default()
        };
        let avoid = HashSet::from(["poring".to_string()]);
        assert!(score_map(&map, &mobs, &me, 0, &avoid).is_none());
    }
}
