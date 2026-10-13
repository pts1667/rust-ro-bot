//! Hunting grounds: maps near the bot ranked by the experience it can safely make there.

use std::collections::HashSet;

use anyhow::Result;

use super::combat::{Fighter, SAFE_DANGER};
use super::db::{Db, ItemData, MapData, MobData};
use super::graph::WorldGraph;

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
        let mobs: Vec<String> = self.targets.iter().take(4).map(|(n, l, a)| format!("{n} lv{l} x{a}")).collect();
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

/// Scores one map. `None` when nothing on it is worth fighting.
pub fn score_map(map: &MapData, mobs: &[MobData], fighter: &Fighter, hops: u32, avoid_mobs: &HashSet<String>) -> Option<Ground> {
    let mut total = 0u32;
    let mut threat_amount = 0u32;
    let mut weighted = 0f32;
    let mut safe_amount = 0u32;
    let mut targets = Vec::new();
    for spawn in &map.mob_spawns {
        let Some(mob) = mobs.iter().find(|m| Some(m.id) == spawn.mob.id_u32()) else { continue };
        total += spawn.amount;
        let a = fighter.assess(mob);
        let avoided = avoid_mobs.iter().any(|n| n.eq_ignore_ascii_case(&mob.name_english));
        // However few, an aggressive monster that kills in two hits makes the map no place to hunt (Drosera lv 46 on
        // ra_fild08, among Drops and Stapo, killed a lv 28 Swordsman with 548 HP).
        if mob.aggressive() && !mob.boss() && fighter.lethal_in_two(mob) {
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
    Some(Ground { map: map.name.clone(), hops, exp_rate, threat, score, targets })
}

/// Rules out the training grounds, arenas, event and quest rooms, whose spawns are not for levelling.
pub fn is_hunting_map_name(name: &str) -> bool {
    const SKIP: [&str; 10] = ["new_", "arena", "room", "quiz", "job_", "que_", "evt", "pvp", "gld", "_in"];
    !SKIP.iter().any(|s| name.contains(s))
}

/// An aggressive monster of `map` that kills the bot in two hits, if any: crossing the map is a gamble.
pub async fn lethal_monster(db: &Db, map: &str, fighter: &Fighter) -> Option<String> {
    let data = db.map(map).await.ok()?;
    for spawn in &data.mob_spawns {
        let Some(id) = spawn.mob.id_u32() else { continue };
        let Ok(mob) = db.mob(id).await else { continue };
        if mob.aggressive() && fighter.lethal_in_two(&mob) {
            return Some(mob.name_english);
        }
    }
    None
}

/// Where to hunt for an item: the map within `max_hops` warps where the bot can safely fight what drops it,
/// ranked by drop chance, spawns and distance. The ground and the monster.
pub async fn farm_ground(
    db: &Db,
    graph: &WorldGraph,
    fighter: &Fighter,
    from: &str,
    item: &ItemData,
    max_hops: u32,
    avoid_maps: &HashSet<String>,
    avoid_mobs: &HashSet<String>,
) -> Result<Option<(Ground, String)>> {
    let mut droppers: Vec<_> = item.dropped_by.iter().filter(|d| !d.mvp && d.rate > 0).collect();
    droppers.sort_by_key(|d| std::cmp::Reverse(d.rate));
    droppers.truncate(8);
    let mut best: Option<(f32, Ground, String)> = None;
    for drop in droppers {
        let Some(id) = drop.mob.id_u32() else { continue };
        let Ok(mob) = db.mob(id).await else { continue };
        for spawn in mob.spawns.iter().filter(|s| s.kind != "boss" && s.amount > 0) {
            let name = spawn.map.id_str();
            if avoid_maps.contains(&name) || !is_hunting_map_name(&name) {
                continue;
            }
            let hops = if name == from {
                0
            } else {
                match graph.route(from, &name, avoid_maps).await? {
                    Some(route) => route.len() as u32,
                    None => continue,
                }
            };
            if hops > max_hops {
                continue;
            }
            let Ok(map) = db.map(&name).await else { continue };
            let mut mobs = Vec::new();
            for s in &map.mob_spawns {
                if let Some(id) = s.mob.id_u32()
                    && let Ok(m) = db.mob(id).await
                {
                    mobs.push(m);
                }
            }
            let Some(ground) = score_map(&map, &mobs, fighter, hops, avoid_mobs) else { continue };
            if !ground.targets.iter().any(|(n, _, _)| *n == mob.name_english) {
                continue;
            }
            let value = drop.rate as f32 * spawn.amount as f32 / (1.0 + 0.3 * hops as f32);
            if best.as_ref().is_none_or(|(v, _, _)| value > *v) {
                best = Some((value, ground, mob.name_english.clone()));
            }
        }
    }
    Ok(best.map(|(_, g, m)| (g, m)))
}

/// The best grounds within `max_hops` warps of `from`.
pub async fn hunting_grounds(
    db: &Db,
    graph: &WorldGraph,
    fighter: &Fighter,
    from: &str,
    max_hops: u32,
    avoid_maps: &HashSet<String>,
    avoid_mobs: &HashSet<String>,
    limit: usize,
) -> Result<Vec<Ground>> {
    let mut grounds = Vec::new();
    for (name, hops) in graph.neighbourhood(from, max_hops).await? {
        if avoid_maps.contains(&name) || !is_hunting_map_name(&name) {
            continue;
        }
        let Ok(map) = db.map(&name).await else { continue };
        if map.is_town() || map.mob_spawns.is_empty() || map.flags.iter().any(|f| f == "pvp" || f == "gvg" || f == "battleground") {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::db::{Link, Spawn};

    fn spawn(id: u32, amount: u32) -> Spawn {
        Spawn { amount, mob: Link { id: id.into(), ..Default::default() }, kind: "normal".into() }
    }

    fn mob(id: u32, name: &str, level: u32, hp: u32, atk: u32, exp: u32, mode: u32) -> MobData {
        MobData { id, name_english: name.into(), level, hp, atk1: atk, atk2: atk, atk_delay: 1500, exp, mode, ..Default::default() }
    }

    #[test]
    fn aggressive_strong_spawns_lower_the_score() {
        let me = Fighter { level: 5, max_hp: 80, str: 10, agi: 1, vit: 5, dex: 5, luk: 1, weapon_atk: 17, armor_def: 1 };
        let poring = mob(1, "Poring", 1, 50, 8, 2, 0x83);
        let brute = mob(2, "Brute", 30, 3000, 30, 500, 0x84);
        let calm = MapData { name: "calm".into(), mob_spawns: vec![spawn(1, 100)], ..Default::default() };
        let scary = MapData { name: "scary".into(), mob_spawns: vec![spawn(1, 100), spawn(2, 50)], ..Default::default() };
        let mobs = vec![poring, brute];
        let a = score_map(&calm, &mobs, &me, 1, &HashSet::new()).unwrap();
        let b = score_map(&scary, &mobs, &me, 1, &HashSet::new()).unwrap();
        assert!(a.score > b.score, "{a:?} {b:?}");
        assert_eq!(b.targets.len(), 1);
    }

    #[test]
    fn a_few_aggressive_monsters_that_kill_in_two_hits_rule_the_map_out() {
        let me = Fighter { level: 28, max_hp: 548, str: 30, vit: 12, weapon_atk: 40, ..Default::default() };
        let drops = mob(1, "Drops", 3, 55, 12, 4, 0x83);
        let drosera = mob(2, "Drosera", 46, 7221, 589, 2612, 0x84);
        let map = MapData { name: "ra_fild08".into(), mob_spawns: vec![spawn(1, 100), spawn(2, 5)], ..Default::default() };
        assert!(score_map(&map, &[drops.clone(), drosera], &me, 0, &HashSet::new()).is_none());
        // A passive one does not.
        let calm = mob(2, "Drosera", 46, 7221, 589, 2612, 0x81);
        assert!(score_map(&map, &[drops, calm], &me, 0, &HashSet::new()).is_some());
    }

    #[test]
    fn avoided_mobs_are_not_targets() {
        let me = Fighter { level: 5, max_hp: 80, str: 10, weapon_atk: 17, ..Default::default() };
        let mobs = vec![mob(1, "Poring", 1, 50, 8, 2, 0x83)];
        let map = MapData { name: "m".into(), mob_spawns: vec![spawn(1, 10)], ..Default::default() };
        let avoid = HashSet::from(["poring".to_string()]);
        assert!(score_map(&map, &mobs, &me, 0, &avoid).is_none());
    }
}
