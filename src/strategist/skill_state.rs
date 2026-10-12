//! The strategist's execution state Σ of SKILL.state (arXiv 2608.26263): a small, fixed-schema record that
//! replaces conversation history. The model proposes a patch ΔΣ each step, and the runtime validates and
//! merges it (Σ ← Σ ⊕ ΔΣ). Validation lives here, so a malformed patch can never corrupt Σ.
//!
//! Small models tend to drop existing entries when they rewrite a list (the paper's main failure mode), so
//! list keys are merged, not replaced: `"avoid_mobs": ["Pupa"]` adds Pupa, `"-Pupa"` removes it.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const FIRST_JOBS: [&str; 6] = ["Swordman", "Mage", "Archer", "Acolyte", "Merchant", "Thief"];
/// Every job `job_target` can name: the first jobs, then the second jobs.
pub const JOBS: [&str; 19] = [
    "Swordman", "Mage", "Archer", "Acolyte", "Merchant", "Thief", "Knight", "Crusader", "Wizard", "Sage", "Hunter", "Bard",
    "Dancer", "Priest", "Monk", "Blacksmith", "Alchemist", "Assassin", "Rogue",
];
pub const STAT_BUILDS: [&str; 5] = ["melee", "tank", "caster", "archer", "balanced"];

const OBJECTIVE_LEN: usize = 120;
const NOTE_LEN: usize = 100;
const MAX_NOTES: usize = 6;
const MAX_AVOID: usize = 10;
const NAME_LEN: usize = 40;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct StrategicState {
    /// Long-term objective in the model's words.
    pub objective: String,
    /// How status points are spent, one of [`STAT_BUILDS`].
    pub stat_build: String,
    /// The next job to change to, one of [`JOBS`].
    pub job_target: Option<String>,
    pub avoid_maps: Vec<String>,
    pub avoid_mobs: Vec<String>,
    /// Short facts the model wants to keep.
    pub notes: Vec<String>,
    /// The jobs a party role leads to, first then second: the model can not change `job_target` then.
    #[serde(skip)]
    pub pinned_jobs: Vec<String>,
    /// The stat build of a party role: the model can not change `stat_build` then.
    #[serde(skip)]
    pub pinned_build: Option<String>,
}

impl Default for StrategicState {
    fn default() -> Self {
        Self {
            objective: "Grow strong: level up safely, then change job.".into(),
            stat_build: "melee".into(),
            job_target: None,
            avoid_maps: Vec::new(),
            avoid_mobs: Vec::new(),
            notes: Vec::new(),
            pinned_jobs: Vec::new(),
            pinned_build: None,
        }
    }
}

/// What a patch did, for logs and the invalid-patch rate.
#[derive(Debug, Default, PartialEq)]
pub struct PatchReport {
    pub applied: Vec<String>,
    pub rejected: Vec<String>,
}

impl StrategicState {
    /// Fixes values an older version let through, after loading Σ from disk.
    pub fn repair(&mut self) {
        if self.objective.trim().is_empty() || looks_like_option_id(&self.objective) {
            self.objective = Self::default().objective;
        }
        self.notes.retain(|n| !n.chars().any(|c| c.is_ascii_digit()));
        if !STAT_BUILDS.contains(&self.stat_build.as_str()) {
            self.stat_build = Self::default().stat_build;
        }
    }

    /// Fixes the job path and the build to those of a party role, for a character that is `job` now.
    pub fn pin_role(&mut self, jobs: &[&str], build: Option<&str>, job: &str) {
        self.pinned_jobs = jobs.iter().map(|j| (*j).to_owned()).collect();
        self.pinned_build = build.map(str::to_owned);
        if let Some(build) = build {
            self.stat_build = build.to_owned();
        }
        self.repin_job(job);
    }

    /// The next job of the pinned path after `job`, as the job target.
    fn repin_job(&mut self, job: &str) {
        if self.pinned_jobs.is_empty() {
            return;
        }
        let at = self.pinned_jobs.iter().position(|j| same_job(j, job));
        let next = match at {
            Some(index) => self.pinned_jobs.get(index + 1),
            None => self.pinned_jobs.first(),
        };
        if let Some(next) = next {
            self.job_target = Some(next.clone());
            self.objective = truncate(&format!("Play {} for the party: become a {next}.", self.pinned_jobs.join(", then ")), OBJECTIVE_LEN);
        }
    }

    /// Once the character has become its `job_target`, the target and the objective about it are done: clears the
    /// target and sets the next step as the objective. The model never rewrote them by itself.
    pub fn on_job(&mut self, job: &str) {
        let Some(target) = &self.job_target else { return };
        let same = same_job;
        if same(target, job) {
            self.job_target = None;
            self.objective = if FIRST_JOBS.iter().any(|j| same(j, job)) {
                truncate(&format!("Grow strong as a {job}: reach job level 40, then pick a second job."), OBJECTIVE_LEN)
            } else {
                truncate(&format!("Grow strong as a {job}: level up safely and gear up."), OBJECTIVE_LEN)
            };
            self.repin_job(job);
        }
    }

    /// Compact JSON for the prompt.
    pub fn to_prompt_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Merges a patch. Unknown keys and invalid values are rejected one by one; the rest still applies.
    pub fn apply_patch(&mut self, patch: &Value) -> PatchReport {
        let mut report = PatchReport::default();
        let Some(obj) = patch.as_object() else {
            if !patch.is_null() {
                report.rejected.push("patch is not an object".into());
            }
            return report;
        };
        for (key, value) in obj {
            let ok = match key.as_str() {
                "objective" => match value {
                    Value::Null => {
                        self.objective = Self::default().objective;
                        true
                    }
                    // An option id ("hunt:prt_fild08") is a goal, not an objective.
                    Value::String(s) if !s.trim().is_empty() && !looks_like_option_id(s) => {
                        self.objective = truncate(s.trim(), OBJECTIVE_LEN);
                        true
                    }
                    _ => false,
                },
                "stat_build" if self.pinned_build.is_some() => false,
                "job_target" if !self.pinned_jobs.is_empty() => false,
                "stat_build" => match value.as_str().map(str::to_lowercase) {
                    Some(b) if STAT_BUILDS.contains(&b.as_str()) => {
                        self.stat_build = b;
                        true
                    }
                    _ => value.is_null() && {
                        self.stat_build = Self::default().stat_build;
                        true
                    },
                },
                "job_target" => match value {
                    Value::Null => {
                        self.job_target = None;
                        true
                    }
                    Value::String(s) => match JOBS.iter().find(|j| j.eq_ignore_ascii_case(s.trim())) {
                        Some(job) => {
                            self.job_target = Some((*job).to_owned());
                            true
                        }
                        None => false,
                    },
                    _ => false,
                },
                "avoid_maps" => merge_list(&mut self.avoid_maps, value, MAX_AVOID, NAME_LEN),
                "avoid_mobs" => merge_list(&mut self.avoid_mobs, value, MAX_AVOID, NAME_LEN),
                // Numbers in notes are copies of the observation, which the model then echoes every turn.
                "notes" => !has_digits(value) && merge_list(&mut self.notes, value, MAX_NOTES, NOTE_LEN),
                _ => false,
            };
            if ok { report.applied.push(key.clone()) } else { report.rejected.push(key.clone()) }
        }
        report
    }

    /// JSON schema of a patch, for the `commit` tool. Every key is optional.
    pub fn patch_schema() -> Value {
        let list = |desc: &str| json!({"type": "array", "items": {"type": "string", "maxLength": NOTE_LEN}, "maxItems": 4, "description": desc});
        json!({
            "type": "object",
            "properties": {
                "objective": {"type": "string", "maxLength": OBJECTIVE_LEN, "description": "Long-term objective, replaces the old one"},
                "stat_build": {"type": "string", "enum": STAT_BUILDS},
                "job_target": {"type": "string", "enum": JOBS},
                "avoid_maps": list("Maps to add to the avoid list; prefix with - to remove"),
                "avoid_mobs": list("Monsters to add to the avoid list; prefix with - to remove"),
                "notes": list("Short facts to remember; prefix with - to forget one")
            }
        })
    }
}

/// Adds the entries of `value` (a string or a list of strings) to `list`, `-x` removing `x`. Old entries go first
/// when the list is full. `null` clears the list.
fn merge_list(list: &mut Vec<String>, value: &Value, max: usize, max_len: usize) -> bool {
    let entries: Vec<&str> = match value {
        Value::Null => {
            list.clear();
            return true;
        }
        Value::String(s) => vec![s.as_str()],
        Value::Array(items) => match items.iter().map(Value::as_str).collect::<Option<Vec<_>>>() {
            Some(v) => v,
            None => return false,
        },
        _ => return false,
    };
    for entry in entries {
        let entry = entry.trim();
        if let Some(gone) = entry.strip_prefix('-') {
            let gone = gone.trim();
            list.retain(|e| !e.eq_ignore_ascii_case(gone));
        } else if !entry.is_empty() && !list.iter().any(|e| e.eq_ignore_ascii_case(entry)) {
            list.push(truncate(entry, max_len));
        }
    }
    if list.len() > max {
        let extra = list.len() - max;
        list.drain(..extra);
    }
    true
}

fn has_digits(value: &Value) -> bool {
    let added = |s: &str| !s.trim_start().starts_with('-') && s.chars().any(|c| c.is_ascii_digit());
    match value {
        Value::String(s) => added(s),
        Value::Array(items) => items.iter().filter_map(Value::as_str).any(added),
        _ => false,
    }
}

fn looks_like_option_id(s: &str) -> bool {
    let s = s.trim();
    !s.contains(' ') && (s.contains(':') || s.contains('_'))
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Whether two job names are the same job (`Swordman`, `Swordsman`).
fn same_job(a: &str, b: &str) -> bool {
    a.len() >= 3 && b.len() >= 3 && a[..3].eq_ignore_ascii_case(&b[..3])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_are_merged_not_replaced() {
        let mut s = StrategicState { avoid_mobs: vec!["Pupa".into()], ..Default::default() };
        let r = s.apply_patch(&json!({"avoid_mobs": ["Wolf"]}));
        assert_eq!(r.applied, vec!["avoid_mobs"]);
        assert_eq!(s.avoid_mobs, vec!["Pupa", "Wolf"]);
        s.apply_patch(&json!({"avoid_mobs": ["-pupa"]}));
        assert_eq!(s.avoid_mobs, vec!["Wolf"]);
    }

    #[test]
    fn unknown_keys_and_bad_values_are_rejected_alone() {
        let mut s = StrategicState::default();
        let r = s.apply_patch(&json!({"level": 99, "job_target": "Ninja", "stat_build": "TANK", "notes": "sell at prt"}));
        assert_eq!(r.applied, vec!["notes", "stat_build"]);
        assert_eq!(r.rejected, vec!["job_target", "level"]);
        assert_eq!(s.stat_build, "tank");
        assert_eq!(s.notes, vec!["sell at prt"]);
        assert_eq!(s.job_target, None);
    }

    #[test]
    fn null_deletes_and_caps_hold() {
        let mut s = StrategicState { job_target: Some("Mage".into()), ..Default::default() };
        s.apply_patch(&json!({"job_target": null, "objective": "x".repeat(500)}));
        assert_eq!(s.job_target, None);
        assert_eq!(s.objective.len(), OBJECTIVE_LEN);
        for c in 'a'..='t' {
            s.apply_patch(&json!({"notes": [format!("note {c}")]}));
        }
        assert_eq!(s.notes.len(), MAX_NOTES);
        assert_eq!(s.notes.last().unwrap(), "note t");
    }

    #[test]
    fn option_ids_are_not_objectives() {
        let mut s = StrategicState::default();
        let r = s.apply_patch(&json!({"objective": "hunt:prt_fild08"}));
        assert_eq!(r.rejected, vec!["objective"]);
        assert_eq!(s.objective, StrategicState::default().objective);
    }

    #[test]
    fn notes_with_numbers_are_rejected() {
        let mut s = StrategicState { notes: vec!["HP 74/89".into()], ..Default::default() };
        assert_eq!(s.apply_patch(&json!({"notes": ["Lunatics flee", "22 kills"]})).rejected, vec!["notes"]);
        assert_eq!(s.apply_patch(&json!({"notes": ["-HP 74/89", "Lunatics flee"]})).applied, vec!["notes"]);
        assert_eq!(s.notes, vec!["Lunatics flee"]);
    }

    #[test]
    fn a_reached_job_target_is_cleared() {
        let mut s = StrategicState { job_target: Some("Swordman".into()), ..Default::default() };
        s.on_job("Novice");
        assert_eq!(s.job_target.as_deref(), Some("Swordman"));
        s.on_job("Swordsman");
        assert_eq!(s.job_target, None);
        assert!(s.objective.contains("second job"));
        assert_eq!(s.apply_patch(&json!({"job_target": "knight"})).applied, vec!["job_target"]);
        assert_eq!(s.job_target.as_deref(), Some("Knight"));
    }

    #[test]
    fn job_names_are_normalised() {
        let mut s = StrategicState::default();
        s.apply_patch(&json!({"job_target": "swordman"}));
        assert_eq!(s.job_target.as_deref(), Some("Swordman"));
    }

    #[test]
    fn non_object_patch_is_rejected() {
        let mut s = StrategicState::default();
        let before = s.clone();
        let r = s.apply_patch(&json!(["nope"]));
        assert_eq!(r.rejected.len(), 1);
        assert_eq!(s, before);
    }

    #[test]
    fn a_party_role_pins_the_job_path_and_the_build() {
        let mut s = StrategicState::default();
        s.pin_role(&["Acolyte", "Priest"], Some("caster"), "Novice");
        assert_eq!(s.job_target.as_deref(), Some("Acolyte"));
        assert_eq!(s.stat_build, "caster");
        let r = s.apply_patch(&json!({"job_target": "Mage", "stat_build": "melee", "notes": "stay with the party"}));
        assert_eq!(r.rejected, vec!["job_target", "stat_build"]);
        assert_eq!(s.job_target.as_deref(), Some("Acolyte"));
        s.on_job("Acolyte");
        assert_eq!(s.job_target.as_deref(), Some("Priest"));
    }
}
