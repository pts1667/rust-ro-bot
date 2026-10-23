//! Quests the bot can follow, written down the way a player follows a guide: where to go, whom to talk to, what
//! to answer and what to bring. The first- and second-job quests of the guilds and the skill quests are here.
//!
//! A quest is a list of stages. The bot remembers the stage it is at (`QuestProgress` in its memory), since the
//! server keeps its own progress in script variables nobody sees. Stages whose result shows in the observation (a
//! skill learned, an item held, a job) are skipped when already done.

pub mod first_jobs;
pub mod jobs;
pub mod rebirth;
pub mod skills;
pub mod skills2;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::api::Observation;
use crate::party::same_job;

/// What finishing the quest gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reward {
    Job(&'static str),
    Skill(&'static str),
    /// A new life as a High Novice.
    Rebirth,
}

/// Items and amounts.
pub type Items = &'static [(u32, u32)];

#[derive(Debug, Clone)]
pub struct Quest {
    /// Stable id: `job:Knight`, `skill:NV_FIRSTAID`.
    pub id: &'static str,
    pub title: &'static str,
    pub reward: Reward,
    /// Jobs that may take the quest.
    pub jobs: &'static [&'static str],
    pub job_level: u32,
    pub base_level: u32,
    /// `male` or `female` when only one may take it.
    pub sex: Option<&'static str>,
    /// Only for a reborn character (`Some(true)`) or one that is not (`Some(false)`): a High Novice is no Novice to
    /// the guilds, it changes job at its own NPCs.
    pub reborn: Option<bool>,
    /// The second jobs whose reborn line the quest belongs to (a High Swordman for a reborn Knight or Crusader);
    /// empty for any.
    pub past: &'static [&'static str],
    /// Only without a cart, which nothing the bot can do takes off.
    pub no_cart: bool,
    /// Skills (name, level) the quest asks for before it starts.
    pub skills: &'static [(&'static str, u32)],
    pub stages: Vec<Stage>,
}

#[derive(Debug, Clone)]
pub struct Stage {
    /// What the stage is about, for the journal and the strategist.
    pub what: &'static str,
    pub act: Act,
    /// The stage is done when any of these holds after acting (none: when acting went through).
    pub done: Vec<Check>,
    /// Gear to wear for this stage (a Rosary for the Crusader quest).
    pub wear: Option<u32>,
    /// Items the quest needs later that no check of this stage looks at (the Mage test's Empty Test Tube): not to sell.
    pub keep: &'static [u32],
    /// It happens on a map only the stage before leads to (a test room): away from it, that stage is done again.
    pub inside: bool,
    /// When the dialogue of this stage says the text, the quest goes back to that stage (a test to take again).
    pub back: Vec<(&'static str, usize)>,
    /// Quest window entries shown while this stage is the one to do: the window tells where the quest stands.
    pub log: &'static [u32],
    /// The part of its map a test happens in (x0, y0, x1, y1), when several tests share a map: being there means
    /// being at this stage.
    pub area: Option<(u16, u16, u16, u16)>,
    /// The way of the quest this stage belongs to, when an NPC sends each player one way of several (the Sage's
    /// professors): the stages of the other ways are skipped.
    pub branch: Option<&'static str>,
}

impl Stage {
    /// The bot is in the place of this test.
    pub fn holds_bot(&self, obs: &Observation) -> bool {
        let (x, y) = (obs.me.x, obs.me.y);
        self.inside
            && stage_map(self) == Some(obs.map.name.as_str())
            && self
                .area
                .is_none_or(|(x0, y0, x1, y1)| (x0..=x1).contains(&x) && (y0..=y1).contains(&y))
    }
}

#[derive(Debug, Clone)]
pub enum Act {
    /// Walk to an NPC and talk to it.
    Talk(Talk),
    /// Have these items (and zeny): bought or hunted for.
    Gather(Need),
    /// A fight on a test map, until the script sends the bot elsewhere.
    Fight(Fight),
    /// Stand on a test map without attacking until the script sends the bot elsewhere.
    Endure { map: &'static str },
    /// Walk to a cell (through a maze, onto a warp).
    Walk {
        map: &'static str,
        x: u16,
        y: u16,
        menu: &'static [&'static str],
        answers: &'static [(&'static str, &'static str)],
        /// Cells never to step on (pitfalls), as the script places them: `(x, y, x_span, y_span)`, the area being the
        /// cells within the spans of `x`, `y`.
        avoid: &'static [Trap],
    },
    /// Leave everything behind: spend the zeny down to the storage fee at a shop, then put every item, worn ones
    /// too, into the Kafra storage.
    Unburden(Unburden),
    /// Join the waiting room of an NPC and wait to be sent to the test.
    Room {
        map: &'static str,
        npc: &'static str,
    },
    /// Dance to the calls of the map's announcements: on each, step onto the cell of the move it names last (`cells`),
    /// from the call holding `start` on. After the call holding `finish`, step onto `end` until sent off the floor.
    Dance(Dance),
}

/// Where a character leaves its zeny and its items: the shops to spend on (the last one buys what the storage
/// refuses), and a Kafra with its storage fee.
#[derive(Debug, Clone)]
pub struct Unburden {
    pub map: &'static str,
    pub shops: &'static [(&'static str, u16, u16)],
    pub kafra: (&'static str, u16, u16),
    pub fee: u64,
}

/// A touch area of a script, `(x, y, x_span, y_span)`.
pub type Trap = (u16, u16, u16, u16);

/// Whether a cell is in one of the areas.
pub fn trapped(avoid: &[Trap], (x, y): (u16, u16)) -> bool {
    avoid
        .iter()
        .any(|&(tx, ty, xs, ys)| x.abs_diff(tx) <= xs && y.abs_diff(ty) <= ys)
}

#[derive(Debug, Clone)]
pub struct Dance {
    pub map: &'static str,
    pub cells: &'static [(&'static str, (u16, u16))],
    pub start: &'static str,
    pub finish: &'static str,
    pub end: (u16, u16),
}

#[derive(Debug, Clone)]
pub struct Talk {
    pub map: &'static str,
    /// The NPC's name as shown (the part before `#` of the script name is enough).
    pub npc: &'static str,
    pub x: u16,
    pub y: u16,
    /// Menu entries to pick, by text, in priority order.
    pub menu: &'static [&'static str],
    /// Answers: when a recent line contains the first text, the menu entry containing the second is picked, or the
    /// second is typed into a text or number prompt. `{name}` is the bot's name.
    pub answers: &'static [(&'static str, &'static str)],
    /// Say back, menu by menu, what the NPC said first.
    pub recite: bool,
    /// The NPC hands out one of these item lists at random: the one named in the dialogue is remembered.
    pub assigns: &'static [Items],
    /// Times to talk in a row (some NPCs only answer after several clicks).
    pub times: u8,
    /// What to answer, in this order, to the menus and prompts of the talk (a machine to work), before the other
    /// answers: a menu entry containing the step, or the step typed in. `{remembered}` is what the quest remembered.
    pub steps: &'static [&'static str],
    /// The NPC says one of these (a serial number to give elsewhere): the one it said is remembered.
    pub remembers: &'static [&'static str],
    /// The part (region) of the map the NPC is talked to from, when the reference places it in none: the Heart of
    /// Ymir stands on a wall of its room.
    pub room: Option<u32>,
}

#[derive(Debug, Clone)]
pub enum Need {
    Items(Items),
    /// The list an NPC handed out (`Talk::assigns`).
    Assigned,
    Zeny(u64),
    ItemsAndZeny(Items, u64),
    /// Items worth this many points together, each its points (the Archer's trunks). The first one is hunted.
    Points(Items, u32),
}

#[derive(Debug, Clone)]
pub struct Fight {
    pub map: &'static str,
    /// Only monsters whose name contains this are fought.
    pub only: Option<&'static str>,
    /// Monsters never to attack.
    pub spare: &'static [&'static str],
    /// Where to walk on when nothing is left to fight.
    pub ahead: Option<(u16, u16)>,
    /// The fight is over once this holds (enough of what the test asks to gather).
    pub until: Vec<Check>,
}

/// A condition on what the bot saw or holds.
#[derive(Debug, Clone, PartialEq)]
pub enum Check {
    /// The dialogue of the stage contained the text.
    Said(&'static str),
    /// The quest window shows the quest.
    Quest(u32),
    /// The quest window does not show it (any more).
    #[allow(dead_code)]
    NoQuest(u32),
    Item(u32),
    NoItem(u32),
    Job(&'static str),
    Skill(&'static str),
    OnMap(&'static str),
    OffMap(&'static str),
    /// The bot stands in this part (x0, y0, x1, y1) of the map: the next test of several on one map.
    At(&'static str, (u16, u16, u16, u16)),
    /// At least this many of the item.
    Count(u32, u32),
    /// The items held are worth at least this many points, each item its points (the grades of a guild test).
    Points(Items, u32),
    /// No item carried or worn, and no zeny.
    Bare,
}

impl Check {
    /// Whether the check holds; `said` is the dialogue of the stage, when it talked.
    pub fn holds(&self, obs: &Observation, said: &[String]) -> bool {
        match self {
            Check::Said(text) => {
                let text = normalize(text);
                said.iter().any(|l| normalize(l).contains(&text))
            }
            Check::Quest(id) => obs.quests.iter().any(|q| q.id == *id),
            Check::NoQuest(id) => !obs.quests.iter().any(|q| q.id == *id),
            Check::Item(id) => obs.item_count(*id) > 0,
            Check::NoItem(id) => obs.item_count(*id) == 0,
            Check::Job(job) => same_job(&obs.me.job, job),
            Check::Skill(skill) => obs.skill_level(skill) > 0,
            Check::OnMap(map) => obs.map.name == *map,
            Check::OffMap(map) => obs.map.name != *map,
            Check::Count(id, n) => obs.item_count(*id) >= *n,
            Check::Points(items, n) => points(items, obs) >= *n,
            Check::Bare => obs.inventory.is_empty() && obs.me.zeny == 0,
            Check::At(map, (x0, y0, x1, y1)) => {
                obs.map.name == *map
                    && (*x0..=*x1).contains(&obs.me.x)
                    && (*y0..=*y1).contains(&obs.me.y)
            }
        }
    }

    /// Checks that only look at the observation, so they tell a stage is done before acting.
    fn by_state(&self) -> bool {
        !matches!(self, Check::Said(_) | Check::OnMap(_) | Check::OffMap(_))
    }
}

/// What the items held are worth, each item its points.
pub fn points(items: Items, obs: &Observation) -> u32 {
    items
        .iter()
        .map(|(id, worth)| obs.item_count(*id) * worth)
        .sum()
}

/// Lower case, colour codes (`^0000FF`) and extra spaces gone.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^'
            && chars
                .clone()
                .take(6)
                .filter(|c| c.is_ascii_hexdigit())
                .count()
                == 6
        {
            for _ in 0..6 {
                chars.next();
            }
            continue;
        }
        out.extend(c.to_lowercase());
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What the bot remembers of a quest.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct QuestProgress {
    /// Index of the stage to do next.
    pub stage: usize,
    /// Tries of the current stage that did not finish it.
    pub attempts: u32,
    /// The item list an NPC handed out.
    pub assigned: Vec<(u32, u32)>,
    /// Unix seconds before which the quest is left alone, after it got stuck.
    pub paused_until: u64,
    pub done: bool,
    /// The way of the quest the bot was sent, once the quest window showed it.
    pub branch: Option<String>,
    /// What an NPC said to remember (`Talk::remembers`).
    pub remembered: Option<String>,
}

pub type QuestBook = BTreeMap<String, QuestProgress>;

static ALL: std::sync::LazyLock<Vec<Quest>> = std::sync::LazyLock::new(|| {
    let mut v = first_jobs::quests();
    v.extend(jobs::quests());
    v.extend(rebirth::quests());
    v.extend(skills::quests());
    v
});

pub fn all() -> &'static [Quest] {
    &ALL
}

pub fn find(id: &str) -> Option<&'static Quest> {
    all().iter().find(|q| q.id == id)
}

impl Quest {
    pub fn finished(&self, obs: &Observation) -> bool {
        match self.reward {
            // The job of a reborn line is only reached reborn: a Knight is no Lord Knight
            Reward::Job(job) => {
                same_job(&obs.me.job, job) && (self.reborn != Some(true) || obs.me.reborn)
            }
            Reward::Skill(skill) => obs.skill_level(skill) > 0,
            Reward::Rebirth => obs.me.reborn,
        }
    }

    /// Whether the character may take the quest now.
    pub fn open_to(&self, obs: &Observation) -> bool {
        let me = &obs.me;
        self.jobs.iter().any(|j| same_job(&me.job, j))
            && me.job_level >= self.job_level
            && me.base_level >= self.base_level
            && self.sex.is_none_or(|s| me.sex.eq_ignore_ascii_case(s))
            && self.skills.iter().all(|(s, l)| obs.skill_level(s) >= *l)
            && self.reborn.is_none_or(|r| r == me.reborn)
            && (self.past.is_empty()
                || me
                    .past_job
                    .as_deref()
                    .is_some_and(|p| self.past.iter().any(|j| same_job(p, j))))
            && !(self.no_cart && obs.cart.is_some())
            && !self.finished(obs)
    }

    /// The stage to do next: the remembered one (or the one the quest window shows, when further), past those the
    /// observation shows done already; on a test map, the stage on it.
    pub fn current_stage(&self, progress: &QuestProgress, obs: &Observation) -> usize {
        let mut i = progress.stage.min(self.stages.len());
        let logged = |s: &Stage| {
            s.log
                .iter()
                .any(|id| obs.quests.iter().any(|q| q.id == *id))
        };
        let branch = self.branch(progress, obs);
        let off = |s: &Stage| matches!((s.branch, branch), (Some(a), Some(b)) if a != b);
        if let Some(l) = self.stages.iter().position(|s| !off(s) && logged(s)) {
            // The window is the truth when the memory is behind it, lost, or ahead past a stage whose entry is not shown.
            let between = self
                .stages
                .get(l + 1..(i + 1).min(self.stages.len()))
                .unwrap_or(&[]);
            if i < l
                || between
                    .iter()
                    .any(|s| !off(s) && !s.log.is_empty() && !logged(s))
            {
                i = l;
            }
        }
        // In the place of a test, the test is the stage (the remembered one when several happen there, else the
        // next); on a test map without places, the next test on it.
        let here: Vec<usize> = (0..self.stages.len())
            .filter(|&t| {
                !off(&self.stages[t])
                    && self.stages[t].area.is_some()
                    && self.stages[t].holds_bot(obs)
            })
            .collect();
        if let Some(&first) = here.first() {
            return here.iter().copied().find(|&t| t >= i).unwrap_or(first);
        }
        if let Some(t) = (i..self.stages.len()).find(|&t| {
            !off(&self.stages[t]) && self.stages[t].area.is_none() && self.stages[t].holds_bot(obs)
        }) {
            i = t;
        }
        // Once the window shows the quest, a stage of other entries is not the one at hand (another order of items, or
        // another of the tests the guild hands out).
        let window = self.stages.iter().any(|s| !off(s) && logged(s));
        while i < self.stages.len() {
            let stage = &self.stages[i];
            if off(stage) || window && !stage.log.is_empty() && !logged(stage) {
                i += 1;
                continue;
            }
            let done = match &stage.act {
                Act::Gather(need) => {
                    missing(need, progress, obs).is_empty() && zeny_short(need, obs) == 0
                }
                _ => stage.done.iter().any(|c| c.by_state() && c.holds(obs, &[])),
            };
            if !done {
                break;
            }
            i += 1;
        }
        i
    }

    /// The way of the quest the bot was sent: remembered, or shown by the quest window.
    pub fn branch<'a>(&'a self, progress: &'a QuestProgress, obs: &Observation) -> Option<&'a str> {
        progress.branch.as_deref().or_else(|| {
            self.stages
                .iter()
                .find(|s| {
                    s.branch.is_some()
                        && s.log
                            .iter()
                            .any(|id| obs.quests.iter().any(|q| q.id == *id))
                })
                .and_then(|s| s.branch)
        })
    }

    /// The stage to act on: the current one, or the one leading into its test room when the bot is not in it. A
    /// talk in a room of a map the bot is on (a waiting room) is away too when the bot stands outside the room.
    pub fn actionable_stage(&self, progress: &QuestProgress, obs: &Observation) -> usize {
        let mut i = self.current_stage(progress, obs);
        while i > 0 && i < self.stages.len() && self.stages[i].inside {
            let stage = &self.stages[i];
            let away = stage_map(stage) != Some(obs.map.name.as_str())
                || matches!(stage.act, Act::Talk(_))
                    && stage.area.is_some()
                    && !stage.holds_bot(obs);
            if !away {
                break;
            }
            i -= 1;
        }
        i
    }

    /// Every item the quest may ask for or hand out along the way (a package, a voucher), not to be sold.
    pub fn items(&self) -> Vec<u32> {
        let mut v = Vec::new();
        for stage in &self.stages {
            let until = match &stage.act {
                Act::Fight(f) => f.until.as_slice(),
                _ => &[],
            };
            for check in stage.done.iter().chain(until) {
                match check {
                    Check::Item(id) | Check::Count(id, _) => v.push(*id),
                    Check::Points(items, _) => v.extend(items.iter().map(|(id, _)| *id)),
                    _ => {}
                }
            }
            match &stage.act {
                Act::Gather(
                    Need::Items(items) | Need::ItemsAndZeny(items, _) | Need::Points(items, _),
                ) => v.extend(items.iter().map(|(i, _)| *i)),
                Act::Talk(t) => v.extend(t.assigns.iter().flat_map(|s| s.iter().map(|(i, _)| *i))),
                _ => {}
            }
            v.extend(stage.wear);
            v.extend(stage.keep);
        }
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// The stage each quest taken up and not finished is at.
pub fn stages(book: &QuestBook, obs: &Observation) -> BTreeMap<String, usize> {
    book.iter()
        .filter(|(_, p)| !p.done)
        .filter_map(|(id, p)| find(id).map(|q| (id.clone(), q.actionable_stage(p, obs))))
        .collect()
}

/// The items the quests taken up and not finished ask for: not to be sold.
pub fn kept_items(book: &QuestBook) -> Vec<u32> {
    let mut v: Vec<u32> = book
        .iter()
        .filter(|(_, p)| !p.done)
        .filter_map(|(id, p)| find(id).map(|q| (q, p)))
        .flat_map(|(q, p)| {
            q.items()
                .into_iter()
                .chain(p.assigned.iter().map(|(i, _)| *i))
        })
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Items (id, amount still needed) a gathering stage lacks.
pub fn missing(need: &Need, progress: &QuestProgress, obs: &Observation) -> Vec<(u32, u32)> {
    let items: Vec<(u32, u32)> = match need {
        Need::Items(items) | Need::ItemsAndZeny(items, _) => items.to_vec(),
        Need::Assigned => progress.assigned.clone(),
        Need::Zeny(_) => Vec::new(),
        Need::Points(items, wanted) => {
            let short = wanted.saturating_sub(points(items, obs));
            return match items.first() {
                Some(&(id, worth)) if short > 0 => vec![(id, short.div_ceil(worth.max(1)))],
                _ => Vec::new(),
            };
        }
    };
    items
        .into_iter()
        .filter_map(|(id, amount)| {
            let have = obs.item_count(id);
            (have < amount).then(|| (id, amount - have))
        })
        .collect()
}

/// Zeny a gathering stage still lacks.
pub fn zeny_short(need: &Need, obs: &Observation) -> u64 {
    match need {
        Need::Zeny(z) | Need::ItemsAndZeny(_, z) => z.saturating_sub(obs.me.zeny),
        _ => 0,
    }
}

/// The list among `sets` the dialogue names: the one with the most of its item names in the text.
pub fn assigned_set(
    sets: &[Items],
    said: &[String],
    name_of: impl Fn(u32) -> Option<String>,
) -> Option<Vec<(u32, u32)>> {
    let text = normalize(&said.join(" "));
    let score = |set: &Items| {
        set.iter()
            .filter(|(id, _)| {
                name_of(*id).is_some_and(|name| {
                    let name = normalize(&name);
                    // "Bug Leg" is also said "Bug Legs".
                    text.contains(&name) || name.len() > 4 && text.contains(&name[..name.len() - 1])
                })
            })
            .count()
    };
    let best = sets.iter().max_by_key(|s| score(s))?;
    (score(best) > 0).then(|| best.to_vec())
}

/// The map a stage happens on.
pub fn stage_map(stage: &Stage) -> Option<&'static str> {
    match &stage.act {
        Act::Talk(t) => Some(t.map),
        Act::Fight(f) => Some(f.map),
        Act::Endure { map } | Act::Walk { map, .. } | Act::Room { map, .. } => Some(map),
        Act::Dance(d) => Some(d.map),
        Act::Unburden(u) => Some(u.map),
        Act::Gather(_) => None,
    }
}

/// Builders, so the quest tables read like a guide.
pub mod build {
    use super::*;

    fn stage(what: &'static str, act: Act, done: Vec<Check>, inside: bool) -> Stage {
        Stage {
            what,
            act,
            done,
            wear: None,
            keep: &[],
            inside,
            back: Vec::new(),
            log: &[],
            area: None,
            branch: None,
        }
    }

    pub fn talk(what: &'static str, map: &'static str, npc: &'static str, x: u16, y: u16) -> Stage {
        stage(
            what,
            Act::Talk(Talk {
                map,
                npc,
                x,
                y,
                menu: &[],
                answers: &[],
                assigns: &[],
                times: 1,
                recite: false,
                steps: &[],
                remembers: &[],
                room: None,
            }),
            Vec::new(),
            false,
        )
    }

    pub fn gather(what: &'static str, items: Items) -> Stage {
        stage(what, Act::Gather(Need::Items(items)), Vec::new(), false)
    }

    /// Items worth `wanted` points together, see `Need::Points`.
    pub fn gather_points(what: &'static str, items: Items, wanted: u32) -> Stage {
        stage(what, Act::Gather(Need::Points(items, wanted)), Vec::new(), false)
    }

    pub fn gather_assigned(what: &'static str) -> Stage {
        stage(what, Act::Gather(Need::Assigned), Vec::new(), false)
    }

    pub fn gather_zeny(what: &'static str, items: Items, zeny: u64) -> Stage {
        let need = if items.is_empty() {
            Need::Zeny(zeny)
        } else {
            Need::ItemsAndZeny(items, zeny)
        };
        stage(what, Act::Gather(need), Vec::new(), false)
    }

    pub fn fight(what: &'static str, map: &'static str) -> Stage {
        stage(
            what,
            Act::Fight(Fight {
                map,
                only: None,
                spare: &[],
                ahead: None,
                until: Vec::new(),
            }),
            vec![Check::OffMap(map)],
            true,
        )
    }

    pub fn dance(what: &'static str, dance: Dance) -> Stage {
        stage(what, Act::Dance(dance), Vec::new(), true)
    }

    pub fn endure(what: &'static str, map: &'static str) -> Stage {
        stage(what, Act::Endure { map }, vec![Check::OffMap(map)], true)
    }

    pub fn walk(what: &'static str, map: &'static str, x: u16, y: u16) -> Stage {
        stage(
            what,
            Act::Walk {
                map,
                x,
                y,
                menu: &[],
                answers: &[],
                avoid: &[],
            },
            Vec::new(),
            false,
        )
    }

    pub fn unburden(what: &'static str, at: Unburden) -> Stage {
        stage(what, Act::Unburden(at), vec![Check::Bare], false)
    }

    pub fn room(what: &'static str, map: &'static str, npc: &'static str) -> Stage {
        // Sent on is passed, whether to another map or to an arena on the same one.
        stage(what, Act::Room { map, npc }, Vec::new(), false)
    }

    impl Stage {
        fn talk_mut(&mut self) -> &mut Talk {
            match &mut self.act {
                Act::Talk(t) => t,
                _ => panic!("{} is not a talk", self.what),
            }
        }

        /// Menu entries to pick, in this order: of the NPC talked to, or of those that speak up on the way.
        pub fn menu(mut self, menu: &'static [&'static str]) -> Self {
            match &mut self.act {
                Act::Walk { menu: m, .. } => *m = menu,
                _ => self.talk_mut().menu = menu,
            }
            self
        }

        /// Quiz answers, of the NPC talked to or of one that speaks up on the way.
        pub fn answers(mut self, answers: &'static [(&'static str, &'static str)]) -> Self {
            match &mut self.act {
                Act::Walk { answers: a, .. } => *a = answers,
                _ => self.talk_mut().answers = answers,
            }
            self
        }

        pub fn assigns(mut self, sets: &'static [Items]) -> Self {
            self.talk_mut().assigns = sets;
            self
        }

        /// Menus ask to say back what the NPC said before them.
        pub fn recite(mut self) -> Self {
            self.talk_mut().recite = true;
            self
        }

        /// A fight that ends once the check holds.
        pub fn until(mut self, check: Check) -> Self {
            if let Act::Fight(f) = &mut self.act {
                f.until.push(check);
            }
            self
        }

        /// The answers, in order, to the menus and prompts of a talk.
        pub fn steps(mut self, steps: &'static [&'static str]) -> Self {
            self.talk_mut().steps = steps;
            self
        }

        /// The NPC says one of these to remember for later (`{remembered}` in the steps).
        pub fn remembers(mut self, values: &'static [&'static str]) -> Self {
            self.talk_mut().remembers = values;
            self
        }

        /// Cells a walk never steps on.
        pub fn avoid(mut self, traps: &'static [Trap]) -> Self {
            if let Act::Walk { avoid, .. } = &mut self.act {
                *avoid = traps;
            }
            self
        }

        /// The room the NPC is talked to from (`Talk::room`).
        pub fn room(mut self, region: u32) -> Self {
            self.talk_mut().room = Some(region);
            self
        }

        pub fn times(mut self, times: u8) -> Self {
            self.talk_mut().times = times;
            self
        }

        pub fn done(mut self, check: Check) -> Self {
            self.done.push(check);
            self
        }

        /// A test is passed only when this holds once it is over (leaving its map is not enough: failing does too).
        pub fn passed(mut self, check: Check) -> Self {
            self.done = vec![check];
            self
        }

        pub fn said(self, text: &'static str) -> Self {
            self.done(Check::Said(text))
        }

        /// The stage happens on a map only the stage before leads to.
        pub fn inside(mut self) -> Self {
            self.inside = true;
            self
        }

        /// The stage happens on a map anyone can walk to.
        #[allow(dead_code)]
        pub fn outside(mut self) -> Self {
            self.inside = false;
            self
        }

        /// When the NPC says `text`, the quest goes back to stage `stage` (a failed test to take again).
        pub fn back_if(mut self, text: &'static str, stage: usize) -> Self {
            self.back.push((text, stage));
            self
        }

        /// Quest window entries that show while this stage is the one to do.
        pub fn log(mut self, ids: &'static [u32]) -> Self {
            self.log = ids;
            self
        }

        /// The stage belongs to one way of the quest of several.
        pub fn branch(mut self, branch: &'static str) -> Self {
            self.branch = Some(branch);
            self
        }

        /// The part of the map the test happens in, when tests share a map.
        pub fn area(mut self, x0: u16, y0: u16, x1: u16, y1: u16) -> Self {
            self.area = Some((x0, y0, x1, y1));
            self
        }

        /// In a fight, where to walk on when no monster is left in sight (the far end of a corridor).
        pub fn ahead(mut self, x: u16, y: u16) -> Self {
            if let Act::Fight(f) = &mut self.act {
                f.ahead = Some((x, y));
            }
            self
        }

        pub fn wear(mut self, item: u32) -> Self {
            self.wear = Some(item);
            self
        }

        /// Items not to sell while the quest is taken up, see `Stage::keep`.
        pub fn keep(mut self, items: &'static [u32]) -> Self {
            self.keep = items;
            self
        }

        pub fn only(mut self, name: &'static str) -> Self {
            if let Act::Fight(f) = &mut self.act {
                f.only = Some(name);
            }
            self
        }

        pub fn spare(mut self, names: &'static [&'static str]) -> Self {
            if let Act::Fight(f) = &mut self.act {
                f.spare = names;
            }
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::observation::{InventoryItem, Skill};

    #[test]
    fn colour_codes_and_case_are_ignored() {
        assert_eq!(
            normalize("^3355FF' 3 Red Herb '^000000  now"),
            "' 3 red herb ' now"
        );
    }

    #[test]
    fn the_named_item_list_is_found() {
        const A: Items = &[(507, 3), (705, 3)];
        const B: Items = &[(909, 5), (938, 5)];
        let name = |id| {
            Some(
                match id {
                    507 => "Red Herb",
                    705 => "Clover",
                    909 => "Jellopy",
                    _ => "Sticky Mucus",
                }
                .to_owned(),
            )
        };
        let said = vec![
            "Bring me 5 ^0000FFJellopys^000000".to_owned(),
            "and 5 Sticky Mucus".to_owned(),
        ];
        assert_eq!(assigned_set(&[A, B], &said, name), Some(B.to_vec()));
        assert_eq!(assigned_set(&[A, B], &["Hello".to_owned()], name), None);
    }

    #[test]
    fn every_quest_is_well_formed() {
        let quests = all();
        let mut ids: Vec<&str> = quests.iter().map(|q| q.id).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate quest ids");
        for q in quests {
            assert!(!q.stages.is_empty(), "{} has no stages", q.id);
            for s in &q.stages {
                if matches!(s.act, Act::Gather(Need::Assigned)) {
                    assert!(
                        q.stages
                            .iter()
                            .any(|t| matches!(&t.act, Act::Talk(t) if !t.assigns.is_empty())),
                        "{}: '{}' gathers an assigned list nobody assigns",
                        q.id,
                        s.what
                    );
                }
            }
        }
    }

    #[test]
    fn stages_shown_done_are_skipped() {
        use build::*;
        let q = Quest {
            id: "t",
            title: "t",
            reward: Reward::Skill("NV_FIRSTAID"),
            jobs: &["Novice"],
            job_level: 1,
            base_level: 1,
            sex: None,
            reborn: None,
            past: &[],
            no_cart: false,
            skills: &[],
            stages: vec![
                talk("a", "prt_in", "Nami", 1, 1).said("awaiting your return"),
                gather("herbs", &[(507, 3)]),
                talk("b", "prt_in", "Nami", 1, 1).done(Check::Skill("NV_FIRSTAID")),
            ],
        };
        let mut obs = Observation::default();
        let p = QuestProgress {
            stage: 1,
            ..Default::default()
        };
        assert_eq!(q.current_stage(&p, &obs), 1);
        obs.inventory.push(InventoryItem {
            item_id: 507,
            amount: 3,
            ..Default::default()
        });
        assert_eq!(q.current_stage(&p, &obs), 2);
        obs.skills.push(Skill {
            name: "NV_FIRSTAID".into(),
            level: 1,
            ..Default::default()
        });
        assert_eq!(q.current_stage(&p, &obs), 3);
        // A said-check never skips a stage.
        assert_eq!(
            q.current_stage(&QuestProgress::default(), &Observation::default()),
            0
        );
    }

    #[test]
    fn the_quest_window_and_the_test_areas_tell_the_stage() {
        use crate::api::observation::QuestEntry;
        let q = find("job:Crusader").unwrap();
        let at = |what: &str| {
            q.stages
                .iter()
                .position(|s| s.what.starts_with(what))
                .unwrap()
        };
        let mut obs = Observation::default();
        obs.map.name = "prt_castle".into();
        let fresh = QuestProgress::default();
        // A Rosary in the bag: the first stage is done by itself.
        obs.inventory.push(InventoryItem {
            item_id: 2608,
            amount: 1,
            ..Default::default()
        });
        assert_eq!(q.current_stage(&fresh, &obs), at("ask the Senior Crusader"));
        // The window says the endurance test was passed although the memory was lost.
        obs.quests.push(QuestEntry {
            id: 3011,
            ..Default::default()
        });
        assert_eq!(q.current_stage(&fresh, &obs), at("answer Gabriel"));
        // On the test map, the place tells which test: the waiting room, then the corridor.
        obs.quests = vec![QuestEntry {
            id: 3014,
            ..Default::default()
        }];
        obs.map.name = "job_cru".into();
        (obs.me.x, obs.me.y) = (24, 169);
        assert_eq!(
            q.actionable_stage(&fresh, &obs),
            at("wait for the battle test")
        );
        (obs.me.x, obs.me.y) = (168, 60);
        assert_eq!(q.actionable_stage(&fresh, &obs), at("defeat the undead"));
        (obs.me.x, obs.me.y) = (98, 40);
        obs.quests = vec![QuestEntry {
            id: 3010,
            ..Default::default()
        }];
        assert_eq!(
            q.actionable_stage(&fresh, &obs),
            at("walk past the monsters")
        );
        // Back in the castle after the battle: the Senior Crusader.
        obs.map.name = "prt_castle".into();
        obs.quests = vec![QuestEntry {
            id: 3015,
            ..Default::default()
        }];
        assert_eq!(
            q.actionable_stage(
                &QuestProgress {
                    stage: at("defeat the undead"),
                    ..Default::default()
                },
                &obs
            ),
            at("become a Crusader")
        );
    }

    fn novice_with(quest: u32, map: &str, (x, y): (u16, u16)) -> Observation {
        use crate::api::observation::QuestEntry;
        let mut obs = Observation::default();
        obs.me.job = "Novice".into();
        obs.map.name = map.into();
        (obs.me.x, obs.me.y) = (x, y);
        obs.quests.push(QuestEntry {
            id: quest,
            ..Default::default()
        });
        obs
    }

    #[test]
    fn the_swordman_course_follows_the_bot_through_its_halls() {
        let q = find("job:Swordman").unwrap();
        let at = |what: &str| {
            q.stages
                .iter()
                .position(|s| s.what.starts_with(what))
                .unwrap()
        };
        let fresh = QuestProgress::default();
        let stage = |map: &str, cell| q.actionable_stage(&fresh, &novice_with(1014, map, cell));
        assert_eq!(stage("job_sword1", (10, 245)), at("cross the first hall"));
        assert_eq!(
            stage("job_sword1", (215, 244)),
            at("step onto the way to the second")
        );
        assert_eq!(stage("job_sword1", (11, 207)), at("cross the second hall"));
        assert_eq!(stage("job_sword1", (11, 169)), at("cross the third hall"));
        assert_eq!(stage("job_sword1", (215, 167)), at("tell Mae"));
        assert_eq!(stage("job_sword1", (65, 56)), at("climb out of the pit"));
        assert_eq!(stage("izlude_in", (39, 170)), at("ask the Test Hall Staff"));
        // Sent back to the hall in the middle of the course: through the door again.
        let midway = QuestProgress {
            stage: at("cross the second hall"),
            ..Default::default()
        };
        assert_eq!(
            q.actionable_stage(&midway, &novice_with(1014, "izlude_in", (65, 165))),
            at("go into the test hall")
        );
        // Sent out by Mae once the course is done: to the Swordman of the guild, whom the door also sends to.
        let passed = QuestProgress {
            stage: at("tell Mae") + 1,
            ..Default::default()
        };
        assert_eq!(
            q.actionable_stage(&passed, &novice_with(1014, "izlude_in", (65, 165))),
            at("become a Swordman")
        );
        assert_eq!(
            q.stages[at("go into the test hall")]
                .back
                .first()
                .map(|b| b.1),
            Some(at("become a Swordman"))
        );
    }

    #[test]
    fn the_course_keeps_off_every_pit() {
        let q = find("job:Swordman").unwrap();
        for s in &q.stages {
            if let Act::Walk { x, y, avoid, .. } = &s.act {
                assert!(!trapped(avoid, (*x, *y)), "'{}' ends on a pit", s.what);
            }
        }
        assert!(trapped(
            q.stages
                .iter()
                .find_map(|s| match &s.act {
                    Act::Walk { avoid, .. } => Some(*avoid),
                    _ => None,
                })
                .unwrap(),
            (62, 250)
        ));
    }

    #[test]
    fn the_mage_quest_goes_the_way_of_its_solution() {
        let q = find("job:Mage").unwrap();
        let at = |what: &str| {
            q.stages
                .iter()
                .position(|s| s.what.starts_with(what))
                .unwrap()
        };
        let fresh = QuestProgress::default();
        let mut obs = novice_with(1007, "geffen_in", (164, 120));
        assert_eq!(q.actionable_stage(&fresh, &obs), at("gather 6 Jellopy"));
        for (item_id, amount) in [(909, 6), (914, 1)] {
            obs.inventory.push(InventoryItem {
                item_id,
                amount,
                ..Default::default()
            });
        }
        let stage = q.actionable_stage(&fresh, &obs);
        assert_eq!(q.stages[stage].branch, Some("3"));
        assert!(q.stages[stage].what.starts_with("buy Payon Solution"));
        obs.inventory.push(InventoryItem {
            item_id: 1089,
            amount: 1,
            ..Default::default()
        });
        let stage = q.actionable_stage(&fresh, &obs);
        assert!(q.stages[stage].what.starts_with("mix the solution"));
        assert_eq!(q.stages[stage].branch, Some("3"));
        // The test tube and the solvent are not for sale.
        assert!(q.items().contains(&1092) && q.items().contains(&1089));
    }

    #[test]
    fn mushrooms_count_by_their_grade() {
        let mut obs = Observation::default();
        let check = Check::Points(&[(1069, 3), (1070, 1)], 25);
        obs.inventory.push(InventoryItem {
            item_id: 1069,
            amount: 8,
            ..Default::default()
        });
        assert!(!check.holds(&obs, &[]));
        obs.inventory.push(InventoryItem {
            item_id: 1070,
            amount: 1,
            ..Default::default()
        });
        assert!(check.holds(&obs, &[]));
    }

    #[test]
    fn archer_trunks_are_gathered_by_their_grade() {
        let need = Need::Points(&[(1068, 2), (1067, 3), (1066, 5), (1019, 1)], 25);
        let fresh = QuestProgress::default();
        let mut obs = Observation::default();
        // Nothing yet: Barren Trunks, the commonest, for all 25 points.
        assert_eq!(missing(&need, &fresh, &obs), vec![(1068, 13)]);
        for (item_id, amount) in [(1066, 3), (1067, 2), (1019, 1)] {
            obs.inventory.push(InventoryItem {
                item_id,
                amount,
                ..Default::default()
            });
        }
        // 15 + 6 + 1 = 22 points: 3 short, two Barren Trunks.
        assert_eq!(missing(&need, &fresh, &obs), vec![(1068, 2)]);
        obs.inventory.push(InventoryItem {
            item_id: 1068,
            amount: 2,
            ..Default::default()
        });
        assert!(missing(&need, &fresh, &obs).is_empty());
        // Every grade is kept from the shops.
        let archer = find("job:Archer").unwrap();
        assert!([1066, 1067, 1068, 1019].iter().all(|i| archer.items().contains(i)));
    }
}
