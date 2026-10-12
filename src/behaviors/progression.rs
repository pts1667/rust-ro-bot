//! Spending status and skill points, and changing job.

use anyhow::Result;
use serde_json::Value;

use super::dialog::{Intent, dialog_of};
use crate::api::{Action, ApiError, Observation};
use crate::bot::{Bot, Outcome};
use crate::goap::state::stat_cost;

const STATS: [&str; 6] = ["str", "agi", "vit", "int", "dex", "luk"];

/// Relative weight of each stat for a build, in the order of [`STATS`].
pub fn build_weights(build: &str) -> [u32; 6] {
    match build {
        "tank" => [2, 0, 3, 0, 1, 0],
        "caster" => [0, 0, 1, 3, 2, 0],
        "archer" => [1, 2, 0, 0, 3, 0],
        "balanced" => [2, 1, 2, 1, 2, 0],
        _ => [3, 2, 1, 0, 2, 0], // melee
    }
}

/// The stat to raise next: the furthest behind its weight among those the points cover.
pub fn next_stat(stats: [u32; 6], points: u32, build: &str) -> Option<&'static str> {
    let weights = build_weights(build);
    (0..6)
        .filter(|i| weights[*i] > 0 && stats[*i] < 99 && stat_cost(stats[*i]) <= points)
        .min_by(|a, b| {
            let ra = stats[*a] as f32 / weights[*a] as f32;
            let rb = stats[*b] as f32 / weights[*b] as f32;
            ra.total_cmp(&rb)
        })
        .map(|i| STATS[i])
}

/// Skills learned in this order, per job, before anything else the server offers.
fn preferred_skills(job: &str) -> &'static [&'static str] {
    match job.to_lowercase().as_str() {
        "novice" => &["NV_BASIC"],
        "swordman" | "swordsman" => &["SM_SWORD", "SM_BASH", "SM_RECOVERY", "SM_PROVOKE", "SM_MAGNUM", "SM_ENDURE"],
        "mage" | "magician" => &["MG_FIREBOLT", "MG_SRECOVERY", "MG_COLDBOLT", "MG_LIGHTNINGBOLT", "MG_NAPALMBEAT", "MG_SOULSTRIKE"],
        "archer" => &["AC_OWL", "AC_VULTURE", "AC_DOUBLE", "AC_CONCENTRATION"],
        "acolyte" => &["AL_HEAL", "AL_BLESSING", "AL_INCAGI", "AL_DP", "AL_DEMONBANE"],
        "merchant" => &["MC_INCCARRY", "MC_DISCOUNT", "MC_OVERCHARGE", "MC_PUSHCART", "MC_MAMMONITE"],
        "thief" => &["TF_DOUBLE", "TF_MISS", "TF_STEAL", "TF_HIDING", "TF_POISON"],
        _ => &[],
    }
}

/// Skill names (`AB_CDEF`) mentioned in a refusal, which lists what can be learned now.
pub fn skill_names_in(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit()))
        .filter(|w| w.len() > 3 && w.contains('_') && w.chars().next().is_some_and(|c| c.is_ascii_uppercase()))
        .map(str::to_owned)
        .collect()
}

fn stats_of(obs: &Observation) -> [u32; 6] {
    let s = &obs.me.stats;
    [s.str, s.agi, s.vit, s.int, s.dex, s.luk]
}

/// Other names the Job Master may use for a job.
fn job_keywords(job: &str) -> Vec<String> {
    let mut k = vec![job.to_owned()];
    match job.to_lowercase().as_str() {
        "swordman" => k.push("Swordsman".into()),
        "mage" => k.push("Magician".into()),
        _ => {}
    }
    k
}

impl Bot {
    pub async fn raise_stats(&mut self) -> Result<Outcome> {
        let mut raised = Vec::new();
        for _ in 0..150 {
            let obs = self.observe().await?;
            let Some(stat) = next_stat(stats_of(&obs), obs.me.status_points, &self.memory.strategy.stat_build) else { break };
            let before = obs.me.stats.get(stat);
            let result = self.act(Action::RaiseStat { stat, amount: 1 }).await;
            let value = match &result {
                Ok(v) => v.get("value").and_then(Value::as_u64).map(|v| v as u32),
                Err(_) => None,
            };
            let value = match value {
                Some(v) => v,
                None => self.observe().await?.me.stats.get(stat),
            };
            if value <= before {
                tracing::warn!(stat, ?result, "raise_stat did not raise the stat");
                self.stats_stuck_at = Some(obs.me.status_points);
                break;
            }
            raised.push(stat);
        }
        if raised.is_empty() {
            // Points left that no stat of the build can take (each costs more): wait for the next level's points.
            self.stats_stuck_at = Some(self.observe().await?.me.status_points);
            return Ok(Outcome::failed("no stat could be raised"));
        }
        let obs = self.observe().await?;
        let s = &obs.me.stats;
        self.memory.note(format!(
            "raised {} stat points ({} build): str {} agi {} vit {} int {} dex {} luk {}",
            raised.len(),
            self.memory.strategy.stat_build,
            s.str,
            s.agi,
            s.vit,
            s.int,
            s.dex,
            s.luk
        ));
        Ok(Outcome::Done)
    }

    pub async fn learn_skills(&mut self) -> Result<Outcome> {
        let mut learned = Vec::new();
        let mut offered: Vec<String> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        for _ in 0..60 {
            let obs = self.observe().await?;
            if obs.me.skill_points == 0 {
                break;
            }
            let job = obs.me.job.clone();
            let candidates: Vec<String> = preferred_skills(&job).iter().map(|s| (*s).to_owned()).chain(offered.iter().cloned()).collect();
            let Some(skill) = candidates.into_iter().find(|s| !failed.contains(s)) else { break };
            match self.act(Action::LearnSkill { skill: skill.clone() }).await {
                Ok(_) => learned.push(skill),
                Err(error) => {
                    tracing::debug!(%skill, %error, "learn_skill refused");
                    if ApiError::status_of(&error).is_some() {
                        for s in skill_names_in(&format!("{error:#}")) {
                            if !offered.contains(&s) {
                                offered.push(s);
                            }
                        }
                    }
                    failed.push(skill);
                }
            }
        }
        let obs = self.observe().await?;
        if learned.is_empty() {
            self.skills_stuck_at = Some((obs.me.job.clone(), obs.me.job_level));
            return Ok(Outcome::failed("no skill could be learned"));
        }
        learned.dedup();
        self.memory.note(format!("learned skills: {}", learned.join(", ")));
        Ok(Outcome::Done)
    }

    /// Asks the Job Master (on the current map) for a job change.
    pub async fn change_job(&mut self, job: &str) -> Result<Outcome> {
        let obs = self.observe().await?;
        let Some(npc) = obs.npcs.iter().find(|n| n.name.to_lowercase().contains("job master")).map(|n| n.id) else {
            return Ok(Outcome::failed(format!("no Job Master on {}", obs.map.name)));
        };
        let result = self.act(Action::Use { target: npc }).await?;
        let intent = Intent {
            goal: format!("Change job from Novice to {job}. Accept the job change."),
            keywords: job_keywords(job).into_iter().chain(["yes".to_owned(), "change".to_owned()]).collect(),
            text_answer: None,
        };
        let end = self.run_dialog(dialog_of(&result), &intent).await?;
        let after = self.observe().await?;
        if after.me.job.eq_ignore_ascii_case(&obs.me.job) {
            return Ok(Outcome::failed(format!("still {} after the Job Master ({end:?})", after.me.job)));
        }
        self.memory.note(format!("changed job: {} -> {}", obs.me.job, after.me.job));
        Ok(Outcome::Done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn melee_build_raises_str_first() {
        assert_eq!(next_stat([1, 1, 1, 1, 1, 1], 48, "melee"), Some("str"));
        // STR 9 is far ahead of its share, so AGI or DEX come next.
        assert!(matches!(next_stat([9, 1, 1, 1, 1, 1], 30, "melee"), Some("agi") | Some("dex") | Some("vit")));
    }

    #[test]
    fn no_stat_when_points_too_few() {
        // Every weighted stat at 11+ costs 3 points.
        assert_eq!(next_stat([11, 11, 11, 1, 11, 1], 2, "melee"), None);
    }

    #[test]
    fn caster_never_raises_str() {
        let mut stats = [1, 1, 1, 1, 1, 1];
        for _ in 0..30 {
            let s = next_stat(stats, 100, "caster").unwrap();
            assert_ne!(s, "str");
            stats[STATS.iter().position(|x| *x == s).unwrap()] += 1;
        }
    }

    #[test]
    fn skill_names_are_found_in_refusals() {
        let text = "400 Bad Request: Basic Skill must be level 9 first; can learn now: NV_BASIC, NV_FIRSTAID";
        assert_eq!(skill_names_in(text), vec!["NV_BASIC", "NV_FIRSTAID"]);
    }
}
