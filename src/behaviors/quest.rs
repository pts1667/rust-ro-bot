//! Carrying out the stages of a quest: talking to its NPCs, fighting its tests, queueing in its waiting rooms.
//! Gathering items is not done here: the strategist sends the bot to buy or hunt them (`strategist::options`).

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;

use super::dialog::{Intent, dialog_of};
use crate::api::{Action, Observation, dist};
use crate::bot::{Bot, Outcome};
use crate::quests::{self, Act, Dance, Fight, Quest, QuestProgress, Talk, assigned_set, stage_map};

/// Tries of one stage before the quest is left alone for a while.
const MAX_ATTEMPTS: u32 = 4;
/// How long a stuck quest is left alone.
const PAUSE_SECS: u64 = 1800;
/// Longest stay on a test map.
const TEST_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Longest wait in a waiting room.
const ROOM_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// HP share under which a potion is taken during a test.
const TEST_HEAL_AT: f32 = 0.5;

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

type Area = (u16, u16, u16, u16);

/// The cell is in the area (anywhere without one).
fn within(area: Option<Area>, (x, y): (u16, u16)) -> bool {
    area.is_none_or(|(x0, y0, x1, y1)| (x0..=x1).contains(&x) && (y0..=y1).contains(&y))
}

/// At most `LEG` cells from `from` straight towards `to`.
fn leg(from: (u16, u16), to: (u16, u16)) -> (u16, u16) {
    const LEG: i32 = 8;
    let step = |a: u16, b: u16| (a as i32 + (b as i32 - a as i32).clamp(-LEG, LEG)) as u16;
    (step(from.0, to.0), step(from.1, to.1))
}

/// The name an NPC shows, without the `#suffix` of its script name.
fn shown_name(name: &str) -> &str {
    name.split('#').next().unwrap_or(name).trim()
}

impl Bot {
    /// Does the next stage of a quest. The bot stands on the stage's map (the plan travelled there).
    pub async fn quest_step(&mut self, id: &str) -> Result<Outcome> {
        let Some(quest) = quests::find(id) else {
            return Ok(Outcome::failed(format!("no quest {id}")));
        };
        let obs = self.observe().await?;
        let mut progress = self.memory.quests.get(id).cloned().unwrap_or_default();
        if quest.finished(&obs) {
            progress.done = true;
            self.memory.quests.insert(id.to_owned(), progress);
            return Ok(Outcome::Done);
        }
        let i = quest.actionable_stage(&progress, &obs);
        let Some(stage) = quest.stages.get(i) else {
            // Every stage went through and the reward did not come: start over, the NPCs will say where it stands.
            self.memory.note(format!(
                "{}: all stages done but no reward yet, starting over",
                quest.title
            ));
            progress.stage = 0;
            self.memory.quests.insert(id.to_owned(), progress);
            return Ok(Outcome::failed("quest went through without its reward"));
        };
        progress.stage = i;
        if progress.branch.is_none() {
            progress.branch = quest.branch(&progress, &obs).map(str::to_owned);
        }
        if let Some(item) = stage.wear {
            self.wear(&obs, item).await;
        }
        tracing::info!(
            quest = quest.title,
            stage = i,
            what = stage.what,
            "quest stage"
        );
        self.last_dialog.clear();
        let outcome = match &stage.act {
            Act::Talk(talk) => self.quest_talk(talk, stage.what).await?,
            Act::Fight(fight) => self.quest_fight(fight, stage.area).await?,
            Act::Endure { map } => self.quest_endure(map).await?,
            Act::Walk {
                map,
                x,
                y,
                menu,
                answers,
            } => {
                self.quest_walk(map, (*x, *y), stage.area, menu, answers, stage.what)
                    .await?
            }
            Act::Room { map, npc } => self.quest_room(map, npc).await?,
            Act::Dance(dance) => self.quest_dance(dance).await?,
            Act::Gather(_) => Outcome::failed("items still missing"),
        };
        let after = self.observe().await?;
        let said = self.last_dialog.clone();
        if let Act::Talk(talk) = &stage.act
            && !talk.assigns.is_empty()
        {
            let names = self
                .item_names(talk.assigns.iter().flat_map(|s| s.iter().map(|(i, _)| *i)))
                .await;
            if let Some(set) = assigned_set(talk.assigns, &said, |i| names.get(&i).cloned()) {
                let list: Vec<String> = set
                    .iter()
                    .map(|(i, n)| format!("{n} {}", names.get(i).cloned().unwrap_or_default()))
                    .collect();
                self.memory.note(format!(
                    "{}: asked to bring {}",
                    quest.title,
                    list.join(", ")
                ));
                progress.assigned = set;
            }
        }
        let done = match (&stage.act, stage.done.is_empty()) {
            (_, true) => outcome == Outcome::Done,
            (Act::Talk(_), false) => stage.done.iter().any(|c| c.holds(&after, &said)),
            // A test is passed by coming out of it alive, not by dying or running out of time.
            (_, false) => {
                outcome == Outcome::Done && stage.done.iter().any(|c| c.holds(&after, &said))
            }
        };
        let back = stage
            .back
            .iter()
            .find(|(text, _)| quests::Check::Said(text).holds(&after, &said))
            .map(|(_, to)| *to);
        if let Some(to) = back {
            self.memory.note(format!(
                "{}: '{}' sends the quest back to '{}'",
                quest.title, stage.what, quest.stages[to].what
            ));
            progress.stage = to;
            progress.attempts = 0;
        } else if done {
            self.memory
                .note(format!("{}: {} - done", quest.title, stage.what));
            progress.stage = i + 1;
            progress.attempts = 0;
        } else {
            progress.attempts += 1;
            let last = said
                .iter()
                .rev()
                .find(|l| !l.starts_with('['))
                .cloned()
                .unwrap_or_default();
            tracing::info!(quest = quest.title, stage = stage.what, attempts = progress.attempts, %last, ?outcome, "quest stage not done");
            if progress.attempts >= MAX_ATTEMPTS {
                progress.attempts = 0;
                progress.paused_until = now_secs() + PAUSE_SECS;
                self.memory.note(format!(
                    "{}: stuck at '{}' (last said: {last}), leaving it for a while",
                    quest.title, stage.what
                ));
            }
        }
        if quest.finished(&after) {
            progress.done = true;
            self.memory.note(format!("{} finished", quest.title));
        }
        self.memory.quests.insert(id.to_owned(), progress);
        Ok(if done || back.is_some() {
            Outcome::Done
        } else {
            match outcome {
                Outcome::Failed(reason) => Outcome::Failed(reason),
                _ => Outcome::failed(format!("'{}' not done yet", stage.what)),
            }
        })
    }

    async fn item_names(&self, ids: impl Iterator<Item = u32>) -> HashMap<u32, String> {
        let mut names = HashMap::new();
        for id in ids {
            if let Ok(item) = self.knowledge.db.item(id).await {
                names.insert(id, item.name_english);
            }
        }
        names
    }

    /// Puts on an item the stage asks to wear, when the bot has it and does not wear it.
    async fn wear(&mut self, obs: &Observation, item: u32) {
        if let Some(it) = obs
            .inventory
            .iter()
            .find(|i| i.item_id == item && !i.equipped)
        {
            if obs
                .inventory
                .iter()
                .any(|i| i.item_id == item && i.equipped)
            {
                return;
            }
            let index = it.index;
            if let Err(error) = self.act(Action::Equip { index }).await {
                tracing::warn!(%error, item, "could not wear the item the quest asks for");
            }
        }
    }

    /// The NPC of a talk on the current map: by its shown name, nearest to where the guide says it stands.
    fn quest_npc(obs: &Observation, talk: &Talk) -> Option<(u64, Option<u32>)> {
        let wanted = talk.npc.to_lowercase();
        obs.npcs
            .iter()
            .filter(|n| {
                shown_name(&n.name).to_lowercase() == wanted || n.name.to_lowercase() == wanted
            })
            .min_by_key(|n| dist((n.x, n.y), (talk.x, talk.y)))
            .or_else(|| {
                obs.npcs
                    .iter()
                    .filter(|n| dist((n.x, n.y), (talk.x, talk.y)) <= 1)
                    .min_by_key(|n| dist((n.x, n.y), (talk.x, talk.y)))
            })
            .map(|n| (n.id, n.region))
    }

    async fn quest_talk(&mut self, talk: &Talk, what: &str) -> Result<Outcome> {
        let obs = self.observe().await?;
        if obs.map.name != talk.map {
            return Ok(Outcome::failed(format!("{} is on {}", talk.npc, talk.map)));
        }
        let Some((npc, region)) = Self::quest_npc(&obs, talk) else {
            return Ok(Outcome::failed(format!("no {} on {}", talk.npc, talk.map)));
        };
        // In another room of the map: through the warps between rooms first.
        if let (Some(to), Some(at)) = (region, obs.me.region)
            && to != at
            && !self.reach_region(to).await?
            && self.travel_into(talk.map, to).await? != Outcome::Done
        {
            return Ok(Outcome::failed(format!(
                "cannot reach the room of {} on {}",
                talk.npc, talk.map
            )));
        }
        let intent = Intent {
            goal: format!("{what}. Say yes to the quest and to its tests."),
            keywords: talk.menu.iter().map(|s| (*s).to_owned()).collect(),
            text_answer: None,
            answers: talk
                .answers
                .iter()
                .map(|(q, a)| ((*q).to_owned(), (*a).to_owned()))
                .collect(),
            recite: talk.recite,
            curious: false,
        };
        let mut said = Vec::new();
        for n in 0..talk.times.max(1) {
            let result = match self.act(Action::Use { target: npc }).await {
                Ok(r) => r,
                Err(error) if n > 0 => {
                    tracing::debug!(%error, "talking again refused");
                    break;
                }
                Err(error) => {
                    return Ok(Outcome::failed(format!(
                        "cannot talk to {}: {error}",
                        talk.npc
                    )));
                }
            };
            let end = self.run_dialog(dialog_of(&result), &intent).await?;
            tracing::debug!(?end, "quest dialogue ended");
            said.extend(self.last_dialog.drain(..));
            if talk.times > 1 {
                Bot::pause(400).await;
            }
        }
        self.last_dialog = said;
        // A script that warps the bot does it a moment after the dialogue closes.
        Bot::pause(800).await;
        Ok(Outcome::Done)
    }

    /// Takes a potion when HP runs low; true when it did.
    async fn heal_in_test(&mut self, obs: &Observation) -> bool {
        if obs.me.hp_frac() < TEST_HEAL_AT
            && let Some(heal) = self.knowledge.heal_action(obs)
        {
            return self.act(heal.action).await.is_ok();
        }
        false
    }

    /// Fights the monsters of a test map (of its `area` when the map holds several tests) until the script sends
    /// the bot away.
    async fn quest_fight(&mut self, fight: &Fight, area: Option<Area>) -> Result<Outcome> {
        let start = Instant::now();
        let mut idle = 0;
        while start.elapsed() < TEST_TIMEOUT {
            let obs = self.observe().await?;
            if obs.me.dead {
                self.memory
                    .note(format!("died in the test on {}", fight.map));
                return Ok(Outcome::failed("died in the test"));
            }
            if obs.map.name != fight.map {
                return Ok(Outcome::Done);
            }
            if fight.until.iter().any(|c| c.holds(&obs, &[])) {
                return Ok(Outcome::Done);
            }
            if !within(area, obs.pos()) {
                // On to the next hall, or sent back when the time ran out: the stage's check tells which.
                return Ok(Outcome::Done);
            }
            if obs.dialog.is_some() {
                // A guide of the hall says its piece when walked past.
                self.act(Action::DialogClose).await.ok();
            }
            if self.heal_in_test(&obs).await {
                continue;
            }
            let me = obs.pos();
            let target = obs
                .mobs
                .iter()
                .filter(|m| within(area, (m.x, m.y)))
                .filter(|m| {
                    fight
                        .only
                        .is_none_or(|only| m.name.eq_ignore_ascii_case(only))
                })
                .filter(|m| !fight.spare.iter().any(|s| m.name.eq_ignore_ascii_case(s)))
                .filter(|m| !self.is_blacklisted(m.id))
                // What attacks the bot first, then the nearest.
                .min_by_key(|m| (m.target != Some(obs.me.char_id), dist((m.x, m.y), me)))
                .map(|m| (m.id, m.name.clone()));
            match target {
                Some((id, name)) => {
                    idle = 0;
                    if let Outcome::Failed(reason) = self.fight(id, &name).await?
                        && reason.contains("died")
                    {
                        return Ok(Outcome::Failed(reason));
                    }
                }
                None => {
                    idle += 1;
                    if let Some(to) = fight.ahead {
                        // On along the corridor: walking on brings the next monsters out; at its end the fight is over.
                        if dist(me, to) <= 1 {
                            return Ok(Outcome::Done);
                        }
                        let step = leg(me, to);
                        if self
                            .act(Action::Move {
                                x: step.0,
                                y: step.1,
                                wait: true,
                            })
                            .await
                            .is_err()
                        {
                            self.act(Action::Move {
                                x: to.0,
                                y: to.1,
                                wait: true,
                            })
                            .await
                            .ok();
                        }
                        continue;
                    } else if idle % 10 == 0 {
                        // Monsters of the next round come after a pause; a long wait means they are out of sight.
                        self.search_walk(&obs).await;
                    }
                    Bot::pause(1000).await;
                }
            }
        }
        Ok(Outcome::failed(format!(
            "the test on {} did not end",
            fight.map
        )))
    }

    /// A step towards the far side of the map, to find what the test spawned there.
    async fn search_walk(&mut self, obs: &Observation) {
        let (w, h) = (obs.map.width.max(2) as u16, obs.map.height.max(2) as u16);
        let (x, y) = obs.pos();
        let to = (
            if x < w / 2 { x + (w - x) / 2 } else { x / 2 },
            if y < h / 2 { y + (h - y) / 2 } else { y / 2 },
        );
        self.act(Action::Move {
            x: to.0,
            y: to.1,
            wait: true,
        })
        .await
        .ok();
    }

    /// Stands on a test map without attacking until the script sends the bot away.
    async fn quest_endure(&mut self, map: &str) -> Result<Outcome> {
        self.act(Action::Stop).await.ok();
        let start = Instant::now();
        while start.elapsed() < TEST_TIMEOUT {
            let obs = self.observe().await?;
            if obs.me.dead {
                return Ok(Outcome::failed("died in the test"));
            }
            if obs.map.name != map {
                return Ok(Outcome::Done);
            }
            self.heal_in_test(&obs).await;
            Bot::pause(1500).await;
        }
        Ok(Outcome::failed(format!("the test on {map} did not end")))
    }

    /// Walks to a cell without fighting, a short way at a time so potions keep the bot alive among monsters.
    async fn quest_walk(
        &mut self,
        map: &str,
        to: (u16, u16),
        area: Option<Area>,
        menu: &[&str],
        answers: &[(&str, &str)],
        what: &str,
    ) -> Result<Outcome> {
        let start = Instant::now();
        while start.elapsed() < TEST_TIMEOUT {
            let obs = self.observe().await?;
            if obs.me.dead {
                return Ok(Outcome::failed("died on the way"));
            }
            if obs.map.name != map || !within(area, obs.pos()) {
                return Ok(Outcome::Done);
            }
            if obs.dialog.is_some() {
                // Something on the way, or at the end of it, speaks up when walked past.
                let intent = Intent {
                    goal: format!("{what}. Refuse temptations."),
                    keywords: menu.iter().map(|s| (*s).to_owned()).collect(),
                    answers: answers
                        .iter()
                        .map(|(q, a)| ((*q).to_owned(), (*a).to_owned()))
                        .collect(),
                    ..Default::default()
                };
                self.run_dialog(obs.dialog.clone(), &intent).await?;
                continue;
            }
            if dist(obs.pos(), to) <= 1 {
                // What stands there may speak up, send the bot on, or send it back to walk the way again (a maze guard).
                Bot::pause(1500).await;
                let after = self.observe().await?;
                let sent_back = area.is_some()
                    && after.map.name == map
                    && within(area, after.pos())
                    && dist(after.pos(), to) > 1;
                if after.dialog.is_some() || sent_back {
                    continue;
                }
                return Ok(Outcome::Done);
            }
            if self.heal_in_test(&obs).await {
                continue;
            }
            // A straight short leg, or the whole way when a wall stands in the straight line (a maze).
            let step = leg(obs.pos(), to);
            if self
                .act(Action::Move {
                    x: step.0,
                    y: step.1,
                    wait: true,
                })
                .await
                .is_err()
                && let Err(error) = self
                    .act(Action::Move {
                        x: to.0,
                        y: to.1,
                        wait: true,
                    })
                    .await
            {
                return Ok(Outcome::failed(format!(
                    "cannot walk to ({}, {}) on {map}: {error}",
                    to.0, to.1
                )));
            }
        }
        Ok(Outcome::failed(format!(
            "did not reach ({}, {}) on {map}",
            to.0, to.1
        )))
    }

    /// Joins the waiting room of an NPC and waits for the script to send the bot on.
    async fn quest_room(&mut self, map: &str, npc: &str) -> Result<Outcome> {
        let obs = self.observe().await?;
        let wanted = npc.to_lowercase();
        let Some(host) = obs
            .npcs
            .iter()
            .filter(|n| n.waiting_room.is_some())
            .find(|n| {
                shown_name(&n.name).to_lowercase() == wanted
                    || n.name.to_lowercase().contains(&wanted)
            })
            .or_else(|| obs.npcs.iter().find(|n| n.waiting_room.is_some()))
            .map(|n| (n.id, n.x, n.y))
        else {
            return Ok(Outcome::failed(format!(
                "no waiting room of {npc} on {map}"
            )));
        };
        if obs.dialog.is_some() {
            // A guide by the room talks to whoever walks past.
            let intent = Intent {
                goal: format!("wait for the test of {npc}"),
                keywords: vec!["keep trying".into(), "continue".into(), "yes".into()],
                ..Default::default()
            };
            self.run_dialog(obs.dialog.clone(), &intent).await?;
        }
        if obs.chat_room.is_none() {
            if dist(obs.pos(), (host.1, host.2)) > 8 {
                self.act(Action::Move {
                    x: host.1,
                    y: host.2.saturating_sub(2),
                    wait: true,
                })
                .await
                .ok();
            }
            if let Err(error) = self.act(Action::RoomJoinNpc { npc: host.0 }).await {
                return Ok(Outcome::failed(format!(
                    "cannot join the waiting room of {npc}: {error}"
                )));
            }
            tracing::info!(%npc, "waiting in the room");
        }
        let start = Instant::now();
        let waiting_at = self.observe().await?.pos();
        while start.elapsed() < ROOM_TIMEOUT {
            Bot::pause(2000).await;
            let obs = self.observe().await?;
            if obs.map.name != map {
                return Ok(Outcome::Done);
            }
            if obs.chat_room.is_none() {
                // Sent on to the test, on the same map (the Knight's arena).
                if dist(obs.pos(), waiting_at) > 5 {
                    return Ok(Outcome::Done);
                }
                // Taken out of the room without being sent on: join again.
                if self.act(Action::RoomJoinNpc { npc: host.0 }).await.is_err() {
                    return Ok(Outcome::failed(format!("left the waiting room of {npc}")));
                }
            }
        }
        self.act(Action::RoomLeave).await.ok();
        Ok(Outcome::failed(format!(
            "waited too long in the room of {npc}"
        )))
    }
}

impl Bot {
    /// Dances to the calls announced on the map until sent off the floor (passed or failed: the stage's check tells).
    async fn quest_dance(&mut self, dance: &Dance) -> Result<Outcome> {
        let obs = self.observe().await?;
        // The calls from the start of this test on; none of an earlier one.
        let calls = |o: &Observation| -> Vec<(u64, String)> {
            let mut v: Vec<(u64, String)> = o
                .messages
                .iter()
                .filter(|m| m.channel == "announce")
                .map(|m| (m.seq, m.text.to_lowercase()))
                .collect();
            v.sort_by_key(|(seq, _)| *seq);
            v
        };
        let mut heard = calls(&obs)
            .iter()
            .rev()
            .find(|(_, t)| t.contains(dance.start))
            .map(|(seq, _)| *seq)
            .unwrap_or_else(|| obs.last_message_seq());
        let mut finished = false;
        let mut poked = Instant::now();
        let start = Instant::now();
        while start.elapsed() < TEST_TIMEOUT {
            let obs = self.observe().await?;
            if obs.map.name != dance.map {
                return Ok(Outcome::Done);
            }
            if obs.me.dead {
                return Ok(Outcome::failed("died on the dance floor"));
            }
            let new: Vec<(u64, String)> = calls(&obs)
                .into_iter()
                .filter(|(seq, _)| *seq > heard)
                .collect();
            for (seq, text) in new {
                heard = seq;
                finished |= text.contains(dance.finish);
                if let Some(to) = called_cell(&text, dance.cells) {
                    tracing::info!(call = %text.trim(), ?to, "dance step");
                    self.act(Action::Move {
                        x: to.0,
                        y: to.1,
                        wait: true,
                    })
                    .await
                    .ok();
                }
            }
            if finished && poked.elapsed() > Duration::from_secs(3) {
                // The way off opens on the end cell a little after the last call: step off it and back on.
                poked = Instant::now();
                self.act(Action::Move {
                    x: dance.end.0 + 4,
                    y: dance.end.1,
                    wait: true,
                })
                .await
                .ok();
                self.act(Action::Move {
                    x: dance.end.0,
                    y: dance.end.1,
                    wait: true,
                })
                .await
                .ok();
            }
            Bot::pause(150).await;
        }
        Ok(Outcome::failed("the dance did not end"))
    }
}

/// The cell of the move a dance call names last ("Left, then Right~!" ends on the right).
fn called_cell(call: &str, cells: &[(&str, (u16, u16))]) -> Option<(u16, u16)> {
    let call = call.to_lowercase();
    cells
        .iter()
        .filter_map(|(word, cell)| call.rfind(word).map(|at| (at, *cell)))
        .max_by_key(|(at, _)| *at)
        .map(|(_, cell)| cell)
}

/// The map of the stage a quest is at, and the spot (an NPC) there.
pub fn stage_place(quest: &Quest, i: usize) -> Option<(&'static str, (u16, u16))> {
    let stage = quest.stages.get(i)?;
    let map = stage_map(stage)?;
    let spot = match &stage.act {
        Act::Talk(t) => (t.x, t.y),
        Act::Walk { x, y, .. } => (*x, *y),
        Act::Dance(d) => d.end,
        _ => (0, 0),
    };
    Some((map, spot))
}

/// Whether the quest may be taken up now (not paused after getting stuck).
pub fn awake(progress: Option<&QuestProgress>) -> bool {
    progress.is_none_or(|p| p.paused_until <= now_secs() && !p.done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dance_call_ends_on_the_move_it_names_last() {
        const CELLS: &[(&str, (u16, u16))] = &[
            ("up", (69, 113)),
            ("down", (69, 107)),
            ("left", (66, 110)),
            ("right", (72, 110)),
            ("center", (69, 110)),
        ];
        assert_eq!(called_cell(" Up!", CELLS), Some((69, 113)));
        assert_eq!(called_cell(" Left, then Right~!", CELLS), Some((72, 110)));
        assert_eq!(
            called_cell(" Left, Center, Down, Up~! ", CELLS),
            Some((69, 113))
        );
        assert_eq!(
            called_cell(" Back to the Center~ !", CELLS),
            Some((69, 110))
        );
        assert_eq!(called_cell(" Hold in place... ", CELLS), None);
    }
}
