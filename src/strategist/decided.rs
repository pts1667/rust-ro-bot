//! A strategist step with the decision model (`--voice system-one`): one choice among the options, read against the
//! guidelines, Σ and the observation the LLM gets. The objective and the job target come from the code (`on_job`,
//! party roles) and from lessons of the forum.
//!
//! Notes: when something happened since the last look, the model sees the lessons the bot's experience suggests
//! (`Known::own_lessons`) that the notes do not hold yet, and is asked whether one would help; when it says yes, it
//! picks up to two of them, each in a call of its own. Fewer choices per call, fewer mistakes.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};

use super::Snapshot;
use super::options::Opt;
use super::prompt;
use super::skill_state::StrategicState;
use crate::voice::known::{Known, Lesson, noted};
use crate::voice::systemone::{Question, SystemOne};

/// The decision model, and the share of the strategist steps it takes (the others go to the fallback).
#[derive(Clone)]
pub struct Decider {
    pub s1: SystemOne,
    pub share: f32,
    /// What the bot had lived through at the last look for lessons (the journal's last line and the deaths), and when.
    pub looked: Arc<Mutex<Option<(String, Instant)>>>,
}

impl Decider {
    pub fn new(s1: SystemOne, share: f32) -> Self {
        Self {
            s1,
            share,
            looked: Default::default(),
        }
    }

    /// The lessons the model keeps from what happened since the last look: none when nothing did, or when it looked
    /// less than `LEARN_GAP` ago.
    pub async fn learn(&self, state: &StrategicState, snap: &Snapshot) -> Result<Vec<Lesson>> {
        let lived = format!(
            "{}|{}",
            snap.journal.last().map(String::as_str).unwrap_or(""),
            snap.deaths
        );
        {
            let mut looked = self.looked.lock().map_err(|_| anyhow!("lessons lock"))?;
            match looked.as_ref() {
                Some((seen, at)) if *seen == lived || at.elapsed() < LEARN_GAP => {
                    return Ok(Vec::new());
                }
                _ => *looked = Some((lived, Instant::now())),
            }
        }
        self.lessons(&Known::from_snapshot(snap), &state.notes)
            .await
    }

    /// The model's part of `learn`: whether to take notes at all, then up to `LESSONS_PER_LOOK` picks.
    pub async fn lessons(&self, known: &Known, notes: &[String]) -> Result<Vec<Lesson>> {
        let candidates: Vec<Lesson> = known
            .own_lessons()
            .into_iter()
            .filter(|l| !noted(notes, l))
            .collect();
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let notes = if notes.is_empty() {
            "none".to_owned()
        } else {
            notes.join("; ")
        };
        let suggested = candidates
            .iter()
            .map(|l| l.note.render())
            .collect::<Vec<_>>()
            .join("; ");
        let situation = format!(
            "{}\nLessons this suggests: {suggested}.\nNotes kept so far: {notes}",
            crate::voice::state_of(known)
        );
        let worth = self
            .s1
            .ask(
                &situation,
                &[(
                    "learn",
                    Question::noul("Would one of these lessons, not yet in the notes, help the character play better?"),
                )],
            )
            .await?;
        let yes = worth.get("learn").map(|a| a.yes()).unwrap_or(0.0);
        tracing::debug!(target: "strategist", yes, candidates = candidates.len(), %situation, "anything to learn?");
        if yes < LEARN_GATE {
            return Ok(Vec::new());
        }
        let mut kept: Vec<Lesson> = Vec::new();
        let mut left = candidates;
        while kept.len() < LESSONS_PER_LOOK && !left.is_empty() {
            let mut options: Vec<(String, String)> = left
                .iter()
                .enumerate()
                .map(|(i, l)| (format!("l{i}"), l.note.render()))
                .collect();
            options.push(("none".into(), "Nothing more worth keeping".into()));
            let so_far = if kept.is_empty() {
                situation.clone()
            } else {
                format!(
                    "{situation}\nJust kept: {}",
                    kept.iter()
                        .map(|l| l.note.render())
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            };
            let pick = self
                .s1
                .choose(
                    &so_far,
                    "Which lesson should the character keep in its notes?",
                    options,
                    TEMPERATURE,
                )
                .await?;
            let Some(i) = pick
                .strip_prefix('l')
                .and_then(|i| i.parse::<usize>().ok())
                .filter(|i| *i < left.len())
            else {
                break;
            };
            kept.push(left.remove(i));
        }
        Ok(kept)
    }
}

/// Least time between two looks for lessons, and the decision model's chance of yes above which the bot takes notes.
const LEARN_GAP: Duration = Duration::from_secs(5 * 60);
const LEARN_GATE: f64 = 0.5;
/// Lessons kept per look, each a call.
const LESSONS_PER_LOOK: usize = 2;

/// Close to the model's pick: a goal drawn by luck costs minutes.
const TEMPERATURE: f64 = 0.3;

const GUIDE: &str = "\
A program plays a Ragnarok Online character (pre-renewal): it walks, fights, loots, rests, trades and spends points \
by itself, and works on the goal chosen until the next choice.
Guidelines:
- Staying alive matters more than speed: prefer threat 0% maps; after deaths on a map, pick another.
- Keep hunting on the current map while it works; switch when a listed map has clearly better exp/s (1.5 times or more) at low threat.
- Healing is counted in HP: \"1.0x max HP\" means the healing items give back the max HP once. Buy potions below 1.0x when affordable. Sell loot when weight is above 40%, or when healing is below 1.0x and zeny is too low for potions.
- When a job change is offered, take the one matching the job target.
- Buying a better weapon or armor makes every fight easier: take it when it is not far.
- Answering someone who asked for something in chat: help party members and friends; help others when it costs little and is safe.";

/// The option the decision model picks, and why, in words for the log.
pub async fn decision_step(
    s1: &SystemOne,
    state: &StrategicState,
    snap: &Snapshot,
    options: &[Opt],
) -> Result<(Opt, String)> {
    let job_target = state.job_target.as_deref().unwrap_or("none yet");
    let notes = if state.notes.is_empty() {
        "none".to_owned()
    } else {
        state.notes.join("; ")
    };
    let situation = format!(
        "{GUIDE}\n\nObjective: {}\nJob target: {job_target}\nNotes: {notes}\n\n{}",
        state.objective,
        prompt::observation(snap).trim_end()
    );
    let choices: Vec<(String, String)> = options
        .iter()
        .map(|o| (o.id.clone(), o.line.clone()))
        .collect();
    let answers = s1
        .ask(
            &situation,
            &[(
                "goal",
                Question::choice("Which goal should the character work on next?", choices),
            )],
        )
        .await?;
    let answer = answers
        .get("goal")
        .ok_or_else(|| anyhow!("decision model: no goal"))?;
    let id = answer
        .sample(TEMPERATURE)
        .ok_or_else(|| anyhow!("decision model: no goal"))?;
    let option = options
        .iter()
        .find(|o| o.id == id)
        .cloned()
        .ok_or_else(|| anyhow!("decision model: unknown option {id}"))?;
    let p = answer.probabilities.get(&id).copied().unwrap_or(0.0);
    Ok((
        option.clone(),
        format!("decision model ({:.0}%): {}", p * 100.0, option.line),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the decision model keeps from a short life: `cargo test lessons_on -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn lessons_on_the_decision_model() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("strategist=debug")
            .with_test_writer()
            .try_init();
        let d = Decider::new(SystemOne::new("http://127.0.0.1:9931").unwrap(), 0.25);
        let known = Known {
            me: "Elara".into(),
            level: 12,
            job: "Novice".into(),
            map: "prt_fild05".into(),
            hunt_map: Some("prt_fild05".into()),
            hunting: true,
            kills: vec![("Poring".into(), 45), ("Fabre".into(), 6)],
            deaths: vec![("Poison Spore".into(), 2)],
            losing_to: vec!["Poison Spore".into()],
            journal: vec![
                "reached base level 10 on prt_fild08".into(),
                "died fighting Poison Spore on prt_fild05".into(),
                "died fighting Poison Spore on prt_fild05".into(),
            ],
            ..Default::default()
        };
        for notes in [
            vec![],
            vec!["Avoid Poison Spore while weak".to_owned()],
            known
                .own_lessons()
                .iter()
                .map(|l| l.note.render())
                .collect(),
        ] {
            let start = Instant::now();
            let kept = d.lessons(&known, &notes).await.unwrap();
            println!(
                "notes {notes:?} -> {:?} in {} ms",
                kept.iter().map(|l| l.note.render()).collect::<Vec<_>>(),
                start.elapsed().as_millis()
            );
        }
    }
}
