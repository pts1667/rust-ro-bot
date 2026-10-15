//! A fixed set of strategist decisions with known good answers, to compare prompt layouts and models offline:
//! live runs are too noisy to tell them apart. None of them repeats a few-shot example.

use std::time::Instant;

use anyhow::Result;

use super::options::Opt;
use super::prompt::{Order, render_turn};
use super::skill_state::StrategicState;
use super::{Snapshot, Step, llm::Llm, llm_turn};
use crate::goap::Goal;
use crate::knowledge::Knowledge;

struct Scenario {
    name: &'static str,
    state: &'static str,
    observation: &'static str,
    options: &'static [(&'static str, &'static str)],
    /// Any of these is a good answer.
    good: &'static [&'static str],
}

const NOVICE: &str = r#"{"objective":"Become a Swordman, then reach base level 40","stat_build":"melee","job_target":"Swordman","avoid_maps":[],"avoid_mobs":[],"notes":[]}"#;

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "switch to a much better map",
        state: NOVICE,
        observation: "Character: Kit the Novice (base level 12, job level 8), HP 100/104 (96%), SP 22/22, zeny 400, weight 6%.
Map: prt_fild01 (340 monsters, 0 players). Healing items 9 (heal 200 HP, 1.9x max HP), loot items 12.
Current goal: hunt on prt_fild01 for 420 s. Last result: killed Lunatic.
Totals: 60 kills (Lunatic 40, Poring 20), 0 deaths ().",
        options: &[
            ("hunt:prt_fild01", "hunt: prt_fild01 (0 warps away, exp/s 0.84, threat 0%): Lunatic lv3 x160, Poring lv1 x60 [current]"),
            ("hunt:prt_fild07", "hunt: prt_fild07 (3 warps away, exp/s 1.63, threat 1%): Rocker lv9 x160, Black Mushroom lv1 x6 (1.9x the exp/s of the current map)"),
            ("hunt:prt_fild06", "hunt: prt_fild06 (2 warps away, exp/s 0.63, threat 0%): Lunatic lv3 x120, Poring lv1 x120 (0.8x the exp/s of the current map)"),
            ("sell_loot", "sell 12 loot items at the shop (weight 6%, healing 1.9x max HP, zeny 400)"),
        ],
        good: &["hunt:prt_fild07"],
    },
    Scenario {
        name: "keep the best map",
        state: NOVICE,
        observation: "Character: Kit the Novice (base level 13, job level 9), HP 90/108 (83%), SP 22/22, zeny 400, weight 8%.
Map: prt_fild07 (240 monsters, 0 players). Healing items 9 (heal 200 HP, 1.9x max HP), loot items 18.
Current goal: hunt on prt_fild07 for 300 s. Last result: killed Rocker.
Totals: 75 kills (Lunatic 40, Rocker 15, Poring 20), 0 deaths ().",
        options: &[
            ("hunt:prt_fild07", "hunt: prt_fild07 (0 warps away, exp/s 1.63, threat 1%): Rocker lv9 x160, Black Mushroom lv1 x6 [current]"),
            ("hunt:prt_fild04", "hunt: prt_fild04 (1 warps away, exp/s 1.11, threat 1%): Rocker lv9 x140, Poring lv1 x60 (0.7x the exp/s of the current map)"),
            ("hunt:prt_fild01", "hunt: prt_fild01 (2 warps away, exp/s 0.84, threat 0%): Lunatic lv3 x160, Poring lv1 x60 (0.5x the exp/s of the current map)"),
            ("explore:mjolnir_09", "visit mjolnir_09 for the first time (2 warps away)"),
        ],
        good: &["hunt:prt_fild07"],
    },
    Scenario {
        name: "sell when broke and out of healing",
        state: NOVICE,
        observation: "Character: Kit the Novice (base level 11, job level 7), HP 70/100 (70%), SP 20/20, zeny 15, weight 14%.
Map: prt_fild08 (300 monsters, 0 players). Healing items 1 (heal 20 HP, 0.2x max HP), loot items 46.
Current goal: hunt on prt_fild08 for 600 s. Last result: killed Poring.
Totals: 50 kills (Poring 30, Lunatic 20), 0 deaths ().",
        options: &[
            ("hunt:prt_fild08", "hunt: prt_fild08 (0 warps away, exp/s 0.90, threat 0%): Poring lv1 x60, Lunatic lv3 x30 [current]"),
            ("hunt:prt_fild01", "hunt: prt_fild01 (2 warps away, exp/s 0.84, threat 0%): Lunatic lv3 x160, Poring lv1 x60 (0.9x the exp/s of the current map)"),
            ("sell_loot", "sell 46 loot items at the shop (weight 14%, healing 0.2x max HP, zeny 15)"),
            ("rest", "stand still until HP is back"),
        ],
        good: &["sell_loot"],
    },
    Scenario {
        name: "buy potions with zeny and little healing",
        state: NOVICE,
        observation: "Character: Kit the Swordsman (base level 20, job level 6), HP 250/280 (89%), SP 40/40, zeny 1800, weight 10%.
Map: prt_fild07 (240 monsters, 0 players). Healing items 2 (heal 80 HP, 0.3x max HP), loot items 5.
Current goal: hunt on prt_fild07 for 500 s. Last result: killed Rocker.
Totals: 180 kills (Rocker 90, Lunatic 60), 1 deaths (Rocker 1).",
        options: &[
            ("hunt:prt_fild07", "hunt: prt_fild07 (0 warps away, exp/s 1.85, threat 1%): Rocker lv9 x160 [current]"),
            ("hunt:prt_fild04", "hunt: prt_fild04 (1 warps away, exp/s 1.20, threat 1%): Rocker lv9 x140, Poring lv1 x60 (0.6x the exp/s of the current map)"),
            ("sell_loot", "sell 5 loot items at the shop (weight 10%, healing 0.3x max HP, zeny 1800)"),
            ("buy_potions", "buy Red Potions at 50 z each (healing 0.3x max HP, zeny 1800)"),
        ],
        good: &["buy_potions"],
    },
    Scenario {
        name: "sell when heavy",
        state: NOVICE,
        observation: "Character: Kit the Swordsman (base level 22, job level 9), HP 300/300 (100%), SP 44/44, zeny 900, weight 63%.
Map: prt_fild07 (240 monsters, 0 players). Healing items 12 (heal 700 HP, 2.3x max HP), loot items 140.
Current goal: hunt on prt_fild07 for 900 s. Last result: killed Rocker.
Totals: 260 kills (Rocker 160, Lunatic 60), 0 deaths ().",
        options: &[
            ("hunt:prt_fild07", "hunt: prt_fild07 (0 warps away, exp/s 1.90, threat 1%): Rocker lv9 x160 [current]"),
            ("hunt:prt_fild04", "hunt: prt_fild04 (1 warps away, exp/s 1.25, threat 1%): Rocker lv9 x140, Poring lv1 x60 (0.7x the exp/s of the current map)"),
            ("sell_loot", "sell 140 loot items at the shop (weight 63%, healing 2.3x max HP, zeny 900)"),
            ("buy_potions", "buy Red Potions at 50 z each (healing 2.3x max HP, zeny 900)"),
        ],
        good: &["sell_loot"],
    },
    Scenario {
        name: "leave a map after deaths",
        state: NOVICE,
        observation: "Character: Kit the Swordsman (base level 24, job level 12), HP 330/330 (100%), SP 48/48, zeny 1200, weight 9%.
Map: prt_fild05 (200 monsters, 0 players). Healing items 10 (heal 600 HP, 1.8x max HP), loot items 10.
Current goal: hunt on prt_fild05 for 400 s. Last result: died fighting Savage.
Totals: 300 kills (Rocker 160, Savage 4), 3 deaths (Savage 3).
Recent events:
- died fighting Savage on prt_fild05
- died fighting Savage on prt_fild05
- died fighting Savage on prt_fild05",
        options: &[
            ("hunt:prt_fild05", "hunt: prt_fild05 (0 warps away, exp/s 2.20, threat 45%): Savage lv26 x10, Rocker lv9 x40 [current]"),
            ("hunt:prt_fild02", "hunt: prt_fild02 (1 warps away, exp/s 1.70, threat 0%): Rocker lv9 x50, Poring lv1 x30 (0.8x the exp/s of the current map)"),
            ("hunt:prt_fild07", "hunt: prt_fild07 (2 warps away, exp/s 1.95, threat 1%): Rocker lv9 x160 (0.9x the exp/s of the current map)"),
            ("buy_potions", "buy Red Potions at 50 z each (healing 1.8x max HP, zeny 1200)"),
        ],
        good: &["hunt:prt_fild02", "hunt:prt_fild07"],
    },
    Scenario {
        name: "pick the job that fits the build",
        state: r#"{"objective":"Level up safely, then change job","stat_build":"caster","job_target":null,"avoid_maps":[],"avoid_mobs":[],"notes":[]}"#,
        observation: "Character: Kit the Novice (base level 15, job level 10), HP 120/120 (100%), SP 60/60, zeny 600, weight 7%.
Stats: str 2 agi 4 vit 5 int 30 dex 20 luk 1. Unspent: 0 status points, 0 skill points.
Map: prt_fild08 (300 monsters, 0 players). Healing items 6 (heal 150 HP, 1.2x max HP), loot items 20.
Current goal: hunt on prt_fild08 for 200 s. Last result: reached job level 10.",
        options: &[
            ("job_change:Swordman", "go to the Job Master in prontera and become a Swordman"),
            ("job_change:Mage", "go to the Job Master in prontera and become a Mage"),
            ("job_change:Archer", "go to the Job Master in prontera and become a Archer"),
            ("job_change:Acolyte", "go to the Job Master in prontera and become a Acolyte"),
            ("job_change:Merchant", "go to the Job Master in prontera and become a Merchant"),
            ("job_change:Thief", "go to the Job Master in prontera and become a Thief"),
        ],
        good: &["job_change:Mage"],
    },
    Scenario {
        name: "refuse a deadly map that pays more",
        state: NOVICE,
        observation: "Character: Kit the Novice (base level 8, job level 6), HP 80/82 (98%), SP 18/18, zeny 100, weight 4%.
Map: prt_fild08 (300 monsters, 0 players). Healing items 4 (heal 80 HP, 1.0x max HP), loot items 9.
Current goal: hunt on prt_fild08 for 300 s. Last result: killed Lunatic.
Totals: 30 kills (Poring 18, Lunatic 12), 0 deaths ().",
        options: &[
            ("hunt:prt_fild08", "hunt: prt_fild08 (0 warps away, exp/s 0.90, threat 0%): Poring lv1 x60, Lunatic lv3 x30 [current]"),
            ("hunt:moc_fild13", "hunt: moc_fild13 (4 warps away, exp/s 2.40, threat 80%): Desert Wolf lv27 x60 (2.7x the exp/s of the current map)"),
            ("hunt:prt_fild06", "hunt: prt_fild06 (2 warps away, exp/s 0.63, threat 0%): Lunatic lv3 x120, Poring lv1 x120 (0.7x the exp/s of the current map)"),
            ("sell_loot", "sell 9 loot items at the shop (weight 4%, healing 1.0x max HP, zeny 100)"),
        ],
        good: &["hunt:prt_fild08"],
    },
];

fn goal_of(id: &str) -> Goal {
    match id.split_once(':') {
        Some(("hunt", map)) => Goal::Hunt { map: map.into() },
        Some(("explore", map)) => Goal::Explore { map: map.into() },
        Some(("job_change", job)) => Goal::JobChange { job: job.into() },
        _ => match id {
            "sell_loot" => Goal::SellLoot,
            "buy_potions" => Goal::BuyPotions,
            _ => Goal::Rest,
        },
    }
}

/// Runs every scenario `n` times per layout and prints the share of good answers.
pub async fn run(llm: &Llm, knowledge: &Knowledge, orders: &[Order], n: usize) -> Result<()> {
    let mut snap = Snapshot::default();
    snap.obs.me.base_level = 12;
    snap.obs.me.max_hp = 104;
    for order in orders {
        let mut llm = llm.clone();
        llm.order = *order;
        let (mut good_total, mut runs, mut bad_patches, mut millis) = (0, 0, 0, 0u128);
        println!("== {order:?} ==");
        for s in SCENARIOS {
            let options: Vec<Opt> = s
                .options
                .iter()
                .map(|(id, line)| Opt {
                    id: (*id).into(),
                    goal: goal_of(id),
                    line: (*line).into(),
                    score: 0.0,
                })
                .collect();
            let mut picks = Vec::new();
            for _ in 0..n {
                let options = super::shuffled(&llm, &options);
                let user = render_turn(
                    *order,
                    s.state,
                    s.observation,
                    &super::prompt::options_text(&options),
                );
                let start = Instant::now();
                let step = Step {
                    llm: &llm,
                    knowledge,
                    snap: &snap,
                    options: &options,
                };
                let pick = match llm_turn(step, user).await {
                    Ok(d) => {
                        let mut state: StrategicState = serde_json::from_str(s.state)?;
                        if !state.apply_patch(&d.patch).rejected.is_empty() {
                            bad_patches += 1;
                        }
                        if picks.is_empty() {
                            println!("   analysis: {}", d.reason);
                        }
                        d.option.id
                    }
                    Err(e) => format!("error: {e}"),
                };
                millis += start.elapsed().as_millis();
                runs += 1;
                if s.good.contains(&pick.as_str()) {
                    good_total += 1;
                }
                picks.push(pick);
            }
            let good = picks
                .iter()
                .filter(|p| s.good.contains(&p.as_str()))
                .count();
            println!("{good}/{n}  {}  picks: {}", s.name, picks.join(", "));
        }
        println!(
            "total {good_total}/{runs} good ({:.0}%), {bad_patches} patches with rejected keys, {} ms per step\n",
            100.0 * good_total as f32 / runs.max(1) as f32,
            millis / runs.max(1) as u128
        );
    }
    Ok(())
}
