//! Worked examples for the strategist, as compact text at the end of the system message. Small models follow a
//! shown pattern better than a described one, but sent as earlier chat turns the examples read as the model's own
//! history: it copied their answers (the same job, the same notes) instead of their reasoning. As short marked
//! text they show the shape of a good answer without looking like turns to continue.
//!
//! The cases cover each guideline once, with made-up characters, and none is a case of the offline eval.

use serde_json::{Value, json};

use super::skill_state::StrategicState;

struct Example {
    /// The situation in one or two lines.
    situation: &'static str,
    /// The options, `id (facts)`.
    options: &'static str,
    /// A lookup made before answering, with its result.
    lookup: Option<(&'static str, &'static str)>,
    commit: fn() -> Value,
}

const EXAMPLES: &[Example] = &[
    Example {
        situation: "Novice, base level 4, in prontera, no healing items. Objective is the default one.",
        options: "hunt:prt_fild08 (1 warp, exp/s 0.90, threat 0%) | hunt:moc_fild13 (4 warps, exp/s 1.10, threat 85%, Desert Wolf lv27) | explore:prt_fild05",
        lookup: Some((
            "monster_info({\"name\":\"Desert Wolf\"})",
            "level 27, attacks on sight; a fight would cost about 400% of your HP",
        )),
        commit: || {
            json!({
                "analysis": "moc_fild13 pays a little more but its threat is 85% and the wolves would kill me; prt_fild08 is safe and close.",
                "option": "hunt:prt_fild08",
                "state_patch": {"objective": "Level up safely to job level 10, then change job", "avoid_mobs": ["Desert Wolf"]}
            })
        },
    },
    Example {
        situation: "Thief, base level 18, hunting on pay_fild01 for 5 minutes; HP 90%, healing 1.5x, weight 12%.",
        options: "hunt:pay_fild01 [current] (exp/s 1.20, threat 0%) | hunt:pay_fild02 (1 warp, exp/s 2.10, threat 2%, 1.8x the exp/s of the current map) | hunt:pay_fild03 (2 warps, exp/s 1.10, threat 0%, 0.9x the exp/s of the current map) | sell_loot (weight 12%, healing 1.5x, zeny 300)",
        lookup: None,
        commit: || {
            json!({
                "analysis": "pay_fild02 gives 2.10 exp/s, almost twice the 1.20 here, at a threat of only 2%: worth the one warp.",
                "option": "hunt:pay_fild02",
                "state_patch": {}
            })
        },
    },
    Example {
        situation: "Archer, base level 25, hunting on gef_fild07; healing 1.6x, weight 18%, zeny 700.",
        options: "hunt:gef_fild07 [current] (exp/s 2.00, threat 1%) | hunt:gef_fild06 (1 warp, exp/s 1.80, threat 0%, 0.9x the exp/s of the current map) | sell_loot (weight 18%, healing 1.6x, zeny 700) | buy_potions (healing 1.6x, zeny 700)",
        lookup: None,
        commit: || {
            json!({
                "analysis": "The current map is the best one listed, healing is above 1.0x and weight is low: nothing to change.",
                "option": "hunt:gef_fild07",
                "state_patch": {}
            })
        },
    },
    Example {
        situation: "Acolyte, base level 16; healing 0.1x, zeny 20, 52 loot items, weight 20%.",
        options: "hunt:prt_fild03 [current] (exp/s 1.10, threat 0%) | sell_loot (weight 20%, healing 0.1x, zeny 20) | rest",
        lookup: None,
        commit: || {
            json!({
                "analysis": "Healing is below 1.0x and 20 zeny cannot buy potions, so the loot must be sold first.",
                "option": "sell_loot",
                "state_patch": {}
            })
        },
    },
    Example {
        situation: "Merchant, base level 30; weight 58%, 160 loot items, healing 2.0x, zeny 3000.",
        options: "hunt:mjolnir_02 [current] (exp/s 2.50, threat 3%) | sell_loot (weight 58%, healing 2.0x, zeny 3000) | buy_potions (healing 2.0x, zeny 3000)",
        lookup: None,
        commit: || {
            json!({
                "analysis": "Weight is above 40%, close to the 50% where HP stops regenerating: sell before hunting on.",
                "option": "sell_loot",
                "state_patch": {}
            })
        },
    },
    Example {
        situation: "Novice, job level 10, stat_build archer, job_target not set; high DEX.",
        options: "job_change:Swordman | job_change:Mage | job_change:Archer | job_change:Acolyte | job_change:Merchant | job_change:Thief",
        lookup: Some((
            "quest_info({\"name\":\"Job Change to Archer\"})",
            "Archer guild in payon; needs job level 10",
        )),
        commit: || {
            json!({
                "analysis": "The build is archer and DEX is high, so Archer fits best.",
                "option": "job_change:Archer",
                "state_patch": {"job_target": "Archer", "objective": "Become an Archer, then reach job level 40"}
            })
        },
    },
    Example {
        situation: "Mage, base level 28, died twice to Orc Warrior on gef_fild10; HP 30%, healing 0.4x, zeny 1500.",
        options: "hunt:gef_fild10 [current] (exp/s 2.60, threat 40%) | hunt:gef_fild01 (2 warps, exp/s 1.90, threat 2%, 0.7x the exp/s of the current map) | buy_potions (healing 0.4x, zeny 1500) | rest",
        lookup: None,
        commit: || {
            json!({
                "analysis": "Two deaths on gef_fild10 mean its 40% threat is real; healing is low and zeny is enough, so buy potions, then hunt somewhere safer.",
                "option": "buy_potions",
                "state_patch": {"avoid_maps": ["gef_fild10"], "notes": ["Orc Warriors outfight a Mage at close range"]}
            })
        },
    },
];

/// The examples as one block for the end of the system message.
pub fn text() -> String {
    let mut out = String::from(
        "\n\nWORKED EXAMPLES (made-up characters, situations shortened; they show how to reason, not what to choose):\n",
    );
    for (i, e) in EXAMPLES.iter().enumerate() {
        out.push_str(&format!("{}. {}\n   Options: {}\n", i + 1, e.situation, e.options));
        if let Some((call, result)) = e.lookup {
            out.push_str(&format!("   Lookup: {call} -> {result}\n"));
        }
        out.push_str(&format!("   commit {}\n", (e.commit)()));
    }
    out
}

/// List entries the example commits add. The model copies them into its own state ("Orc Warriors outfight…" with no
/// Orc ever met), so they are taken out of real patches.
fn example_entries() -> Vec<String> {
    let mut out = Vec::new();
    for example in EXAMPLES {
        let patch = &(example.commit)()["state_patch"];
        for key in LIST_KEYS {
            if let Some(items) = patch[key].as_array() {
                out.extend(items.iter().filter_map(Value::as_str).filter(|e| !e.starts_with('-')).map(str::to_lowercase));
            }
        }
    }
    out
}

const LIST_KEYS: [&str; 3] = ["notes", "avoid_maps", "avoid_mobs"];

/// Drops the example entries from a patch's lists.
pub fn scrub_patch(mut patch: Value) -> Value {
    let examples = example_entries();
    for key in LIST_KEYS {
        if let Some(items) = patch.get_mut(key).and_then(Value::as_array_mut) {
            items.retain(|e| e.as_str().is_none_or(|s| !examples.contains(&s.trim().to_lowercase())));
        }
    }
    patch
}

/// Drops example entries a state already holds (older versions had other examples; their copies stay until the
/// model removes them).
pub fn scrub_state(state: &mut StrategicState) {
    let examples = example_entries();
    for list in [&mut state.notes, &mut state.avoid_maps, &mut state.avoid_mobs] {
        list.retain(|e| !examples.contains(&e.trim().to_lowercase()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_example_entries_are_scrubbed() {
        let patch = json!({"notes": ["Orc Warriors outfight a Mage at close range", "Rockers hit hard"], "avoid_maps": ["gef_fild10"]});
        assert_eq!(scrub_patch(patch), json!({"notes": ["Rockers hit hard"], "avoid_maps": []}));
    }

    #[test]
    fn examples_commit_offered_options_with_valid_patches() {
        for example in EXAMPLES {
            let commit = (example.commit)();
            let report = StrategicState::default().apply_patch(&commit["state_patch"]);
            assert!(report.rejected.is_empty(), "{report:?}");
            let option = commit["option"].as_str().unwrap();
            assert!(example.options.contains(option), "{option} is not offered");
        }
    }

    #[test]
    fn analysis_comes_before_the_option() {
        let line = text();
        let first = line.lines().find(|l| l.contains("commit {")).unwrap();
        assert!(first.find("\"analysis\"").unwrap() < first.find("\"option\"").unwrap());
    }
}
