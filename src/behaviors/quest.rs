//! Carrying out the stages of a quest: talking to its NPCs, fighting its tests, queueing in its waiting rooms.
//! Gathering items is not done here: the strategist sends the bot to buy or hunt them (`strategist::options`).

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;

use super::dialog::{Intent, dialog_of};
use super::travel::journey;
use crate::api::client::MapGrid;
use crate::api::{Action, BuyLine, Observation, SellLine, dist};
use crate::bot::{Bot, Outcome};
use crate::memory::ShopSpot;
use crate::quests::{
    self, Act, Check, Dance, Fight, Quest, QuestProgress, Reward, Talk, Trap, Unburden,
    assigned_set, normalize, stage_map, trapped,
};

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
/// Rounds of spending and storing when leaving everything behind: each takes what the weight limit lets the bot buy.
const UNBURDEN_ROUNDS: u32 = 12;
/// The Free Ticket for Kafra Storage, which a Kafra takes in place of the fee.
const FREE_TICKET: u32 = 7059;
/// Slots of the inventory.
const INVENTORY_SLOTS: u32 = 100;
/// Walks on to an NPC after one the server's action timeout ended: Father Rubalkabara is ~350 cells (52 s) from the
/// way into `prt_fild03`.
const WALK_ON: u32 = 4;
/// Most of one item in a slot.
const STACK: u64 = 30_000;

/// A shop's ware: its price there, its weight, whether it stacks into one slot.
#[derive(Debug, Clone, Copy)]
struct Ware {
    item_id: u32,
    price: u64,
    weight: u32,
    stacks: bool,
}

/// What to buy of a shop's wares to spend `zeny` and no more, within `room` weight and `slots` free inventory slots
/// (`held`: items carried, whose stacks take no new slot): the dearest first, so that each weight carried takes the
/// most zeny, down to the cheapest for the change.
fn spending(wares: &[Ware], zeny: u64, room: u32, slots: u32, held: &[u32]) -> Vec<(u32, u32)> {
    let mut wares: Vec<&Ware> = wares.iter().filter(|w| w.price > 0).collect();
    wares.sort_by_key(|w| std::cmp::Reverse(w.price));
    let (mut zeny, mut room, mut slots) = (zeny, room, slots);
    let mut out = Vec::new();
    for w in wares {
        let new_slot = !held.contains(&w.item_id);
        let mut n = zeny / w.price;
        if w.weight > 0 {
            n = n.min(u64::from(room / w.weight));
        }
        n = match (w.stacks, new_slot && slots == 0) {
            (true, true) => 0,
            (true, false) => n.min(STACK),
            (false, _) => n.min(u64::from(slots)),
        };
        if n == 0 {
            continue;
        }
        let n = n as u32;
        zeny -= w.price * u64::from(n);
        room -= w.weight * n;
        slots -= if w.stacks { u32::from(new_slot) } else { n };
        out.push((w.item_id, n));
    }
    out
}

/// A shop of a quest, as the commerce behaviour walks to them.
fn spot(map: &str, name: &str, x: u16, y: u16) -> ShopSpot {
    ShopSpot {
        map: map.to_owned(),
        x,
        y,
        name: name.to_owned(),
        price: 0,
    }
}

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

/// The shortest walk from `from` to `to` over walkable cells outside the traps, one cell at a time without diagonals
/// (a diagonal step could cut the corner of a trap).
fn safe_path(
    grid: &MapGrid,
    avoid: &[Trap],
    from: (u16, u16),
    to: (u16, u16),
) -> Option<Vec<(u16, u16)>> {
    let safe = |c: (u16, u16)| grid.walkable(c.0, c.1) && !trapped(avoid, c);
    if !safe(to) {
        return None;
    }
    let mut came: HashMap<(u16, u16), (u16, u16)> = HashMap::new();
    let mut queue = VecDeque::from([from]);
    came.insert(from, from);
    while let Some(c) = queue.pop_front() {
        if c == to {
            let mut path = vec![c];
            let mut at = c;
            while at != from {
                at = came[&at];
                path.push(at);
            }
            path.reverse();
            return Some(path);
        }
        let (x, y) = c;
        let next = [
            (x.wrapping_add(1), y),
            (x.wrapping_sub(1), y),
            (x, y.wrapping_add(1)),
            (x, y.wrapping_sub(1)),
        ];
        for n in next {
            if safe(n) && !came.contains_key(&n) {
                came.insert(n, c);
                queue.push_back(n);
            }
        }
    }
    None
}

/// The ends of the straight stretches of a path, each at most `LEG` cells: walked one after the other, the walk
/// keeps to the path.
fn straight_legs(path: &[(u16, u16)]) -> Vec<(u16, u16)> {
    const LEG: usize = 8;
    let dir = |a: (u16, u16), b: (u16, u16)| (b.0 as i32 - a.0 as i32, b.1 as i32 - a.1 as i32);
    let mut legs = Vec::new();
    let mut start = 0;
    for i in 1..path.len() {
        let turns = i + 1 < path.len() && dir(path[i - 1], path[i]) != dir(path[i], path[i + 1]);
        if turns || i - start >= LEG || i + 1 == path.len() {
            legs.push(path[i]);
            start = i;
        }
    }
    legs
}

/// The name an NPC shows, without the `#suffix` of its script name.
fn shown_name(name: &str) -> &str {
    name.split('#').next().unwrap_or(name).trim()
}

/// A walk to a cell, and the dialogs that may come up on the way.
struct Walk<'a> {
    map: &'a str,
    to: (u16, u16),
    /// The walk stays in this area (anywhere without one).
    area: Option<Area>,
    /// Menu entries to pick, of those that speak up on the way.
    menu: &'a [&'a str],
    /// Cells never to step on.
    avoid: &'a [Trap],
    /// Quiz answers, of those that speak up on the way.
    answers: &'a [(&'a str, &'a str)],
    /// What the walk is for: the goal of a dialog on the way.
    what: &'a str,
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
        if progress.done {
            self.memory.note(format!(
                "{}: remembered done, but the reward is not had: starting over",
                quest.title
            ));
            progress = QuestProgress::default();
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
        // A stage that gives what an NPC said to remember, with nothing remembered (memory lost): back to that NPC.
        if let Act::Talk(talk) = &stage.act
            && progress.remembered.is_none()
            && talk.steps.iter().any(|s| s.contains("{remembered}"))
            && let Some(back) = quest.stages[..i]
                .iter()
                .rposition(|s| matches!(&s.act, Act::Talk(t) if !t.remembers.is_empty()))
        {
            self.memory.note(format!(
                "{}: forgot what to tell, asking again at '{}'",
                quest.title, quest.stages[back].what
            ));
            progress.stage = back;
            self.memory.quests.insert(id.to_owned(), progress);
            return Ok(Outcome::failed("nothing remembered to tell"));
        }
        progress.stage = i;
        progress.branch = quest.branch(&progress, &obs).map(str::to_owned);
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
            Act::Talk(talk) => {
                self.quest_talk(talk, stage.what, progress.remembered.as_deref())
                    .await?
            }
            Act::Fight(fight) => self.quest_fight(fight, stage.area).await?,
            Act::Endure { map } => self.quest_endure(map).await?,
            Act::Walk {
                map,
                x,
                y,
                menu,
                answers,
                avoid,
            } => {
                let walk = Walk {
                    map,
                    to: (*x, *y),
                    area: stage.area,
                    menu,
                    answers,
                    avoid,
                    what: stage.what,
                };
                self.quest_walk(walk).await?
            }
            Act::Room { map, npc } => self.quest_room(map, npc).await?,
            Act::Dance(dance) => self.quest_dance(dance).await?,
            Act::Unburden(u) => self.quest_unburden(u, stage.what).await?,
            Act::Gather(_) => Outcome::failed("items still missing"),
        };
        let after = self.observe().await?;
        let said = self.last_dialog.clone();
        if let Act::Talk(talk) = &stage.act
            && !talk.assigns.is_empty()
        {
            let ids: Vec<u32> = talk
                .assigns
                .iter()
                .flat_map(|s| s.iter().map(|(i, _)| *i))
                .collect();
            let names = self.item_names(&ids).await;
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
        if let Act::Talk(talk) = &stage.act
            && let Some(value) = talk
                .remembers
                .iter()
                .find(|v| said.iter().any(|l| normalize(l).contains(&normalize(v))))
        {
            self.memory
                .note(format!("{}: to remember: {value}", quest.title));
            progress.remembered = Some((*value).to_owned());
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

    /// Takes a slice, not an iterator: an iterator of closures held across the awaits keeps rustc from seeing the
    /// bot's future is `Send`.
    async fn item_names(&self, ids: &[u32]) -> HashMap<u32, String> {
        let mut names = HashMap::new();
        for &id in ids {
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
                    .filter(|n| !n.invisible && dist((n.x, n.y), (talk.x, talk.y)) <= 1)
                    .min_by_key(|n| dist((n.x, n.y), (talk.x, talk.y)))
            })
            .map(|n| (n.id, n.region))
    }

    async fn quest_talk(
        &mut self,
        talk: &Talk,
        what: &str,
        remembered: Option<&str>,
    ) -> Result<Outcome> {
        let obs = self.observe().await?;
        if obs.map.name != talk.map {
            return Ok(Outcome::failed(format!("{} is on {}", talk.npc, talk.map)));
        }
        let Some((npc, region)) = Self::quest_npc(&obs, talk) else {
            return Ok(Outcome::failed(format!("no {} on {}", talk.npc, talk.map)));
        };
        let region = region.or(talk.room);
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
            strict: false,
            curious: false,
            steps: talk
                .steps
                .iter()
                .map(|s| s.replace("{remembered}", remembered.unwrap_or_default()))
                .collect(),
        };
        let mut said = Vec::new();
        for n in 0..talk.times.max(1) {
            let mut result = match self.act(Action::Use { target: npc }).await {
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
            // A long walk to the NPC ends before it gets there: the talk has not happened, whatever `use` answered.
            for _ in 0..WALK_ON {
                if journey(&result) != "timeout" || self.preempted() {
                    break;
                }
                result = self.act(Action::Use { target: npc }).await?;
            }
            if matches!(journey(&result), "timeout" | "stopped") {
                let why = result
                    .get("stop_reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("too far");
                return Ok(Outcome::failed(format!(
                    "did not reach {} ({why})",
                    talk.npc
                )));
            }
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
            && let Some(heal) = self.knowledge.heal_action(obs, &self.keep)
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

    /// Leaves everything behind (`Act::Unburden`): the zeny spent down to the storage fee at the shops of the map,
    /// then every item, worn ones too, put into the Kafra storage. As many rounds of buying and storing as the weight
    /// the bot carries asks for; what the storage refuses is sold, and its zeny spent the next round.
    async fn quest_unburden(&mut self, u: &Unburden, what: &str) -> Result<Outcome> {
        let kafra = Talk {
            map: u.map,
            npc: u.kafra.0,
            x: u.kafra.1,
            y: u.kafra.2,
            menu: &["Use Storage"],
            answers: &[],
            assigns: &[],
            times: 1,
            recite: false,
            steps: &[],
            remembers: &[],
            room: None,
        };
        let fee_of = |obs: &Observation| {
            if obs.item_count(FREE_TICKET) > 0 {
                0
            } else {
                u.fee
            }
        };
        let mut refused: Vec<u32> = Vec::new();
        for _ in 0..UNBURDEN_ROUNDS {
            let obs = self.observe().await?;
            if obs.map.name != u.map {
                return Ok(Outcome::failed(format!(
                    "the Kafra storage is on {}",
                    u.map
                )));
            }
            if Check::Bare.holds(&obs, &[]) {
                return Ok(Outcome::Done);
            }
            // What the storage refused, or everything when the fee is short, goes to the shop
            let sell_all = obs.me.zeny < fee_of(&obs);
            if (!refused.is_empty() || sell_all)
                && let Some(&(name, x, y)) = u.shops.last()
            {
                let lines: Vec<SellLine> = obs
                    .inventory
                    .iter()
                    .filter(|i| !i.equipped && (sell_all || refused.contains(&i.item_id)))
                    .map(|i| SellLine {
                        index: i.index,
                        amount: i.amount,
                    })
                    .collect();
                let shop = spot(u.map, name, x, y);
                if !lines.is_empty()
                    && self.walk_to_spot(&shop).await? == Outcome::Done
                    && self.open_shop_at(&shop, "sell").await?.is_some()
                    && let Err(error) = self.act(Action::DialogSell { items: lines }).await
                {
                    tracing::warn!(%error, "the shop bought nothing of what is left");
                }
                refused.clear();
            }
            for &(name, x, y) in u.shops {
                let obs = self.observe().await?;
                let fee = fee_of(&obs);
                if obs.me.zeny <= fee {
                    break;
                }
                let shop = spot(u.map, name, x, y);
                if self.walk_to_spot(&shop).await? != Outcome::Done {
                    continue;
                }
                let Some(dialog) = self.open_shop_at(&shop, "buy").await? else {
                    continue;
                };
                let mut wares = Vec::new();
                for line in dialog.prompt.map(|p| p.items).unwrap_or_default() {
                    let Ok(item) = self.knowledge.db.item(line.item_id).await else {
                        continue;
                    };
                    wares.push(Ware {
                        item_id: line.item_id,
                        price: line.price,
                        weight: item.weight,
                        stacks: item.location == 0,
                    });
                }
                let held: Vec<u32> = obs.inventory.iter().map(|i| i.item_id).collect();
                let room = (obs.me.max_weight * 9 / 10).saturating_sub(obs.me.weight);
                let slots = INVENTORY_SLOTS.saturating_sub(obs.inventory.len() as u32);
                let items: Vec<BuyLine> = spending(&wares, obs.me.zeny - fee, room, slots, &held)
                    .into_iter()
                    .map(|(item_id, amount)| BuyLine { item_id, amount })
                    .collect();
                if items.is_empty() {
                    self.act(Action::DialogClose).await.ok();
                    continue;
                }
                if let Err(error) = self.act(Action::DialogBuy { items }).await {
                    tracing::warn!(%error, shop = name, "buying to spend the zeny refused");
                }
            }
            let obs = self.observe().await?;
            if obs.me.zeny < fee_of(&obs) {
                continue;
            }
            match self.quest_talk(&kafra, what, None).await? {
                Outcome::Done => {}
                other => return Ok(other),
            }
            let open = self.observe().await?;
            if open.storage.is_none() {
                return Ok(Outcome::failed("the Kafra did not open the storage"));
            }
            for item in open.inventory.iter().filter(|i| i.equipped) {
                if let Err(error) = self.act(Action::Unequip { index: item.index }).await {
                    tracing::warn!(%error, item = item.name, "could not take off an item to store it");
                }
            }
            for item in self.observe().await?.inventory {
                if let Err(error) = self
                    .act(Action::StorageDeposit {
                        index: item.index,
                        amount: item.amount,
                    })
                    .await
                {
                    tracing::info!(%error, item = item.name, "the storage refused an item");
                    refused.push(item.item_id);
                }
            }
            self.act(Action::StorageClose).await.ok();
            let after = self.observe().await?;
            self.memory.note(format!(
                "left {} items in the storage, {} z and {} items still carried",
                open.inventory.len().saturating_sub(after.inventory.len()),
                after.me.zeny,
                after.inventory.len()
            ));
        }
        let obs = self.observe().await?;
        Ok(if Check::Bare.holds(&obs, &[]) {
            Outcome::Done
        } else {
            Outcome::failed(format!(
                "still carrying {} z and {} items",
                obs.me.zeny,
                obs.inventory.len()
            ))
        })
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
    async fn quest_walk(&mut self, walk: Walk<'_>) -> Result<Outcome> {
        let Walk {
            map,
            to,
            area,
            menu,
            answers,
            avoid,
            what,
        } = walk;
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
            if !avoid.is_empty() {
                if let Some(failed) = self.walk_around(&obs, to, avoid).await? {
                    return Ok(failed);
                }
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

    /// Walks the safe way towards `to`, keeping off the traps, a stretch at a time; with no safe way there (fallen
    /// into a pit), onto the nearest warp of the map, which leads back. `Some` ends the walk as failed.
    async fn walk_around(
        &mut self,
        obs: &Observation,
        to: (u16, u16),
        avoid: &[Trap],
    ) -> Result<Option<Outcome>> {
        let me = obs.pos();
        // The square around both ends, with room to go round
        let mid = ((me.0 + to.0) / 2, (me.1 + to.1) / 2);
        let half = me.0.abs_diff(to.0).max(me.1.abs_diff(to.1)) / 2;
        let grid = self
            .api
            .map_grid(&self.name, mid.0, mid.1, (half + 30).clamp(40, 250))
            .await?;
        let path = safe_path(&grid, avoid, me, to).or_else(|| {
            obs.warps
                .iter()
                .filter(|w| w.to_map == obs.map.name)
                .filter_map(|w| safe_path(&grid, avoid, me, (w.x, w.y)))
                .min_by_key(Vec::len)
        });
        let Some(path) = path else {
            return Ok(Some(Outcome::failed(format!(
                "no safe way to ({}, {}) on {}",
                to.0, to.1, obs.map.name
            ))));
        };
        for cell in straight_legs(&path) {
            if let Err(error) = self
                .act(Action::Move {
                    x: cell.0,
                    y: cell.1,
                    wait: true,
                })
                .await
            {
                tracing::debug!(%error, ?cell, "safe walk refused");
                return Ok(None);
            }
            // Sent elsewhere (a gate, a pit) or stopped by a dialog: look again.
            let now = self.observe().await?;
            if now.map.name != obs.map.name || now.pos() != cell || now.dialog.is_some() {
                return Ok(None);
            }
        }
        Ok(None)
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
        Act::Unburden(u) => (u.kafra.1, u.kafra.2),
        _ => (0, 0),
    };
    Some((map, spot))
}

/// Whether the quest may be taken up now (not paused after getting stuck).
/// A job quest remembered done counts only once the job is had: the callers ask `Quest::open_to` first, so one remembered
/// done here is of a character made anew (the server's characters were wiped), and is taken up again.
pub fn awake(quest: &Quest, progress: Option<&QuestProgress>) -> bool {
    progress.is_none_or(|p| {
        p.paused_until <= now_secs() && (!p.done || matches!(quest.reward, Reward::Job(_)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_zeny_is_spent_to_the_fee_dearest_first() {
        let ware = |item_id, price, weight, stacks| Ware {
            item_id,
            price,
            weight,
            stacks,
        };
        // Mr. King's breastplates for the bulk, then the Tool Dealer's potions and Arrows for the change
        let wares = [
            ware(1750, 1, 1, true),
            ware(2340, 89_000, 500, false),
            ware(504, 1_200, 150, true),
        ];
        let buy = spending(&wares, 1_000_000, 10_000, 50, &[]);
        assert_eq!(buy, vec![(2340, 11), (504, 17), (1750, 600)]);
        let spent: u64 = buy
            .iter()
            .map(|(id, n)| wares.iter().find(|w| w.item_id == *id).unwrap().price * u64::from(*n))
            .sum();
        assert_eq!(spent, 1_000_000);
        // Too little room: the weight limit stops it, and the rest waits for the next round
        let buy = spending(&wares, 1_000_000, 1_000, 50, &[]);
        assert_eq!(buy[0], (2340, 2));
        // No free slot: only a stack already carried grows
        assert_eq!(spending(&wares, 100, 1_000, 0, &[1750]), vec![(1750, 100)]);
        assert!(spending(&wares, 100, 1_000, 0, &[]).is_empty());
    }

    #[test]
    fn the_safe_path_goes_round_the_pits() {
        // A corridor of 5 rows; a pit across the middle row and the one above it, open at the top.
        let grid = MapGrid {
            x_min: 0,
            y_min: 0,
            y_max: 4,
            rows: vec!["..........".into(); 5],
        };
        const PITS: &[Trap] = &[(5, 1, 0, 1), (5, 3, 0, 0)];
        let path = safe_path(&grid, PITS, (0, 2), (9, 2)).unwrap();
        assert!(path.iter().all(|c| !trapped(PITS, *c)));
        assert_eq!(path.first(), Some(&(0, 2)));
        assert_eq!(path.last(), Some(&(9, 2)));
        assert!(path.contains(&(5, 4)));
        let legs = straight_legs(&path);
        assert_eq!(legs.last(), Some(&(9, 2)));
        // Each leg is straight: one coordinate stays.
        let mut from = (0, 2);
        for leg in legs {
            assert!(leg.0 == from.0 || leg.1 == from.1, "{from:?} -> {leg:?}");
            from = leg;
        }
        // A pit across the whole corridor: no way.
        assert!(safe_path(&grid, &[(5, 2, 0, 2)], (0, 2), (9, 2)).is_none());
    }

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
