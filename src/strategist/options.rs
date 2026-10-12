//! The menu of goals offered to the strategist, built by code so the small model picks rather than invents,
//! and the deterministic choice used without the LLM or when it fails.

use std::collections::HashSet;

use anyhow::Result;

use super::Snapshot;
use super::skill_state::StrategicState;
use crate::goap::Goal;
use crate::goap::state::{HEAL_STOCK_MIN, HEAL_STOCK_WANTED, next_jobs};
use crate::knowledge::Knowledge;
use crate::knowledge::grounds::{hunting_grounds, is_hunting_map_name};

/// The map of the Job Master.
pub const JOB_MAP: &str = "prontera";
const HUNT_OPTIONS: usize = 5;
const HUNT_HOPS: u32 = 4;
const DEFAULT_POTION_PRICE: u64 = 50;

#[derive(Debug, Clone, PartialEq)]
pub struct Opt {
    /// Stable id the model answers with, e.g. `hunt:prt_fild08`.
    pub id: String,
    pub goal: Goal,
    /// One line of description for the prompt.
    pub line: String,
    /// Hunting score, for the fallback.
    pub score: f32,
}

fn opt(id: impl Into<String>, goal: Goal, line: impl Into<String>) -> Opt {
    Opt { id: id.into(), goal, line: line.into(), score: 0.0 }
}

pub async fn build_options(snap: &Snapshot, knowledge: &Knowledge, state: &StrategicState) -> Result<Vec<Opt>> {
    let ws = &snap.ws;
    if ws.in_training {
        return Ok(vec![opt("leave_training", Goal::LeaveTraining, "talk to the receptionist and go to a town (one way)")]);
    }
    let failed = |g: &Goal| snap.recent_failures.iter().any(|f| f == g);
    let mut out = Vec::new();

    if ws.job_change_ready {
        for job in next_jobs(&ws.job) {
            let goal = Goal::JobChange { job: (*job).to_owned() };
            if !failed(&goal) {
                out.push(opt(format!("job_change:{job}"), goal, format!("go to the Job Master in {JOB_MAP} and become a {job}")));
            }
        }
        // A job change is always worth more than another hunt, but the model kept hunting when offered both.
        // So it only chooses which job, and only when no target is set yet.
        if !out.is_empty() {
            if let Some(target) = &state.job_target
                && let Some(i) = out.iter().position(|o| o.id.eq_ignore_ascii_case(&format!("job_change:{target}")))
            {
                return Ok(vec![out.swap_remove(i)]);
            }
            return Ok(out);
        }
    }

    let fighter = knowledge.fighter(&snap.obs).await?;
    let avoid_maps: HashSet<String> = state.avoid_maps.iter().cloned().collect();
    let avoid_mobs: HashSet<String> = state.avoid_mobs.iter().cloned().collect();
    let grounds = hunting_grounds(&knowledge.db, &knowledge.graph, &fighter, &ws.map, HUNT_HOPS, &avoid_maps, &avoid_mobs, HUNT_OPTIONS + 3).await?;
    // The model weighs facts it reads next to an option, and overlooks them elsewhere in the observation.
    let here = grounds.iter().find(|g| g.map == ws.map).map(|g| g.exp_rate).filter(|r| *r > 0.0);
    let mut hunts = 0;
    for g in &grounds {
        let goal = Goal::Hunt { map: g.map.clone() };
        if failed(&goal) || hunts >= HUNT_OPTIONS {
            continue;
        }
        hunts += 1;
        let current = if snap.goal.as_ref() == Some(&goal) { " [current]" } else { "" };
        let compare = match here {
            Some(r) if g.map != ws.map => format!(" ({:.1}x the exp/s of the current map)", g.exp_rate / r),
            _ => String::new(),
        };
        out.push(Opt { id: format!("hunt:{}", g.map), goal, line: format!("hunt: {}{current}{compare}", g.describe()), score: g.score });
    }

    if snap.loot_items > 0 && !failed(&Goal::SellLoot) {
        out.push(opt(
            "sell_loot",
            Goal::SellLoot,
            format!(
                "sell {} loot items at the shop (weight {:.0}%, healing {:.1}x max HP, zeny {})",
                snap.loot_items,
                snap.weight_frac * 100.0,
                heal_ratio(snap),
                snap.obs.me.zeny
            ),
        ));
    }
    let price = if snap.potion_price > 0 { snap.potion_price } else { DEFAULT_POTION_PRICE };
    if heal_ratio(snap) < HEAL_STOCK_WANTED && snap.obs.me.zeny >= price * 3 + 100 && !failed(&Goal::BuyPotions) {
        out.push(opt(
            "buy_potions",
            Goal::BuyPotions,
            format!("buy Red Potions at {price} z each (healing {:.1}x max HP, zeny {})", heal_ratio(snap), snap.obs.me.zeny),
        ));
    }
    if snap.obs.me.hp_frac() < 0.9 {
        out.push(opt("rest", Goal::Rest, "stand still until HP is back"));
    }

    // A couple of unvisited maps nearby, for curiosity.
    let mut explore = 0;
    for (map, hops) in knowledge.graph.neighbourhood(&ws.map, 2).await? {
        if explore >= 2 || hops == 0 || snap.visited.contains(&map) || avoid_maps.contains(&map) || !is_hunting_map_name(&map) {
            continue;
        }
        let goal = Goal::Explore { map: map.clone() };
        if failed(&goal) {
            continue;
        }
        explore += 1;
        out.push(opt(format!("explore:{map}"), goal, format!("visit {map} for the first time ({hops} warps away)")));
    }
    if out.is_empty() {
        out.push(opt("rest", Goal::Rest, "nothing else to do: stand still"));
    }
    Ok(out)
}

/// Healing carried, in multiples of the max HP.
pub fn heal_ratio(snap: &Snapshot) -> f32 {
    snap.heal_hp as f32 / snap.obs.me.max_hp.max(1) as f32
}

/// The job a build suits.
pub fn job_for_build(build: &str) -> &'static str {
    match build {
        "caster" => "Mage",
        "archer" => "Archer",
        _ => "Swordman",
    }
}

/// Deterministic choice: progress first, then upkeep, then the best hunting ground.
pub fn fallback(snap: &Snapshot, options: &[Opt], state: &StrategicState) -> Option<Opt> {
    let find = |id: &str| options.iter().find(|o| o.id == id).cloned();
    if let Some(o) = find("leave_training") {
        return Some(o);
    }
    let job = state.job_target.clone().unwrap_or_else(|| job_for_build(&state.stat_build).to_owned());
    if let Some(o) = find(&format!("job_change:{job}")) {
        return Some(o);
    }
    // Past the first job: the first offered branch.
    if let Some(o) = options.iter().find(|o| o.id.starts_with("job_change:") && !snap.ws.job.eq_ignore_ascii_case("novice")) {
        return Some(o.clone());
    }
    // Without potions and the zeny to buy them, loot is the only way to get some.
    let broke = heal_ratio(snap) < HEAL_STOCK_MIN && snap.obs.me.zeny < DEFAULT_POTION_PRICE * 5 && snap.loot_items >= 15;
    if snap.weight_frac >= 0.4 || snap.loot_items >= 60 || broke {
        if let Some(o) = find("sell_loot") {
            return Some(o);
        }
    }
    if heal_ratio(snap) < HEAL_STOCK_MIN && snap.obs.me.base_level >= 5 {
        if let Some(o) = find("buy_potions") {
            return Some(o);
        }
    }
    let best = options.iter().filter(|o| o.id.starts_with("hunt:")).max_by(|a, b| a.score.total_cmp(&b.score));
    if let Some(current) = options.iter().find(|o| o.id.starts_with("hunt:") && Some(&o.goal) == snap.goal.as_ref())
        && let Some(best) = best
        && current.score >= best.score * 0.7
    {
        return Some(current.clone());
    }
    best.cloned().or_else(|| options.iter().find(|o| o.id.starts_with("explore:")).cloned()).or_else(|| options.first().cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunt(map: &str, score: f32) -> Opt {
        Opt { id: format!("hunt:{map}"), goal: Goal::Hunt { map: map.into() }, line: String::new(), score }
    }

    #[test]
    fn fallback_keeps_a_good_current_map() {
        let snap = Snapshot { goal: Some(Goal::Hunt { map: "a".into() }), ..Default::default() };
        let options = vec![hunt("a", 0.8), hunt("b", 1.0)];
        assert_eq!(fallback(&snap, &options, &StrategicState::default()).unwrap().id, "hunt:a");
        let options = vec![hunt("a", 0.5), hunt("b", 1.0)];
        assert_eq!(fallback(&snap, &options, &StrategicState::default()).unwrap().id, "hunt:b");
    }

    #[test]
    fn fallback_sells_when_heavy() {
        let snap = Snapshot { weight_frac: 0.45, ..Default::default() };
        let options = vec![hunt("a", 1.0), opt("sell_loot", Goal::SellLoot, "")];
        assert_eq!(fallback(&snap, &options, &StrategicState::default()).unwrap().id, "sell_loot");
    }

    #[test]
    fn fallback_sells_when_broke_without_potions() {
        let mut snap = Snapshot { loot_items: 20, heal_hp: 100, ..Default::default() };
        snap.obs.me.max_hp = 200;
        let options = vec![hunt("a", 1.0), opt("sell_loot", Goal::SellLoot, "")];
        assert_eq!(fallback(&snap, &options, &StrategicState::default()).unwrap().id, "sell_loot");
    }

    #[test]
    fn fallback_takes_the_job_matching_the_build() {
        let options = vec![
            opt("job_change:Mage", Goal::JobChange { job: "Mage".into() }, ""),
            opt("job_change:Swordman", Goal::JobChange { job: "Swordman".into() }, ""),
        ];
        let state = StrategicState { stat_build: "caster".into(), ..Default::default() };
        assert_eq!(fallback(&Snapshot::default(), &options, &state).unwrap().id, "job_change:Mage");
    }
}
