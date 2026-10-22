//! Town quests: the mini-quests the townsfolk of the server's `alive_towns` plugin give players walking by. With the
//! plugin on (it lists its `offers` action under `plugins` of the observation), the bot asks in each town what the
//! townsfolk offer it this hour, keeps those it can do (a hunt of a monster it beats, or loot it can farm, on a ground
//! it reaches safely), and walks up to their givers: the offer shows in a bubble over the townsperson's head, and the
//! bot presses Accept as a player does. The quests it holds are under `plugins.alive_towns.data.quests`: the strategist
//! offers a hunt on each one's ground (`town_quest:<n>`), the hunt prefers its monster, its loot is kept, and once one
//! is done a trip to a town of the plugin, which pays the reward on arrival (`town_quest:reward`).

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::api::{Action, Observation};
use crate::bot::Bot;
use crate::knowledge::Knowledge;
use crate::knowledge::grounds::{Ground, Hunt};

pub const PLUGIN: &str = "alive_towns";
const OFFERS: &str = "offers";
const OFFER_WINDOW: &str = "offer";
/// Warps the bot goes for a town quest.
pub const QUEST_HOPS: u32 = 6;
/// Time between two looks at the offers of a town, at the same level.
const OFFERS_EVERY: Duration = Duration::from_secs(10 * 60);
/// How long the bot looks for the giver of an offer.
const SEEK: Duration = Duration::from_secs(90);
/// A quest with less time left than this is not set out on.
pub const MIN_SECS_LEFT: i64 = 5 * 60;
/// How much more a monster a town quest asks for is worth fighting.
pub const WANTED_BONUS: f32 = 3.0;

/// What a quest asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Task {
    /// Kill a number of one monster.
    Hunt { mob: u32 },
    /// Bring a number of one item.
    Gather { item: u32 },
}

/// A quest the bot holds, as the plugin shows it.
#[derive(Debug, Clone, Deserialize)]
pub struct TownQuest {
    #[serde(default)]
    pub title: String,
    pub task: Task,
    /// Where its monster lives.
    #[serde(default)]
    pub map: String,
    #[serde(default)]
    pub have: u32,
    #[serde(default)]
    pub count: u32,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub seconds_left: i64,
}

/// An offer of the townsfolk of the bot's town, from the `offers` action.
#[derive(Debug, Clone, Deserialize)]
pub struct Offer {
    /// The townsperson, above whom the bubble shows.
    pub npc_id: u64,
    #[serde(default)]
    pub giver: String,
    /// Where they stand; absent while they are inside a building.
    pub x: Option<u16>,
    pub y: Option<u16>,
    #[serde(default)]
    pub title: String,
    pub task: Task,
    #[serde(default)]
    pub map: String,
    #[serde(default)]
    pub map_name: String,
    #[serde(default)]
    pub base_exp: u32,
    #[serde(default)]
    pub job_exp: u32,
    #[serde(default)]
    pub zeny: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct Offers {
    town: String,
    offers: Vec<Value>,
    /// Cells (|dx| + |dy|) from a townsperson within which its bubble shows.
    range: u32,
    free_slots: u32,
    error: Option<String>,
}

/// What the bot remembers of town quests: the towns of the plugin, and the quests it took with their rewards (the
/// plugin shows neither once they are taken).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TownQuestBook {
    pub towns: BTreeSet<String>,
    pub taken: Vec<Taken>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Taken {
    pub title: String,
    pub map: String,
    pub giver: String,
    pub town: String,
    pub base_exp: u32,
    pub job_exp: u32,
    pub zeny: u32,
    /// Done when last seen.
    pub done: bool,
}

impl TownQuestBook {
    /// The quest taken that the plugin shows as `quest`.
    pub fn taken(&self, quest: &TownQuest) -> Option<&Taken> {
        self.taken
            .iter()
            .find(|t| t.title == quest.title && t.map == quest.map)
    }
}

/// The server runs the plugin with its quests.
pub fn enabled(obs: &Observation) -> bool {
    obs.plugins
        .get(PLUGIN)
        .is_some_and(|p| p.offers_action(OFFERS))
}

/// The quests the bot holds.
pub fn held(obs: &Observation) -> Vec<TownQuest> {
    let Some(Value::Array(quests)) = obs.plugins.get(PLUGIN).map(|p| &p.data["quests"]) else {
        return Vec::new();
    };
    quests
        .iter()
        .filter_map(|q| serde_json::from_value(q.clone()).ok())
        .collect()
}

/// The items the quests held ask for: not to be sold. The plugin takes them when it pays the reward.
pub fn kept_items(obs: &Observation) -> Vec<u32> {
    held(obs)
        .iter()
        .filter_map(|q| match q.task {
            Task::Gather { item } => Some(item),
            Task::Hunt { .. } => None,
        })
        .collect()
}

/// Where to do a quest: its map when the bot can fight what it asks for there, else (for loot) the best ground for it
/// within reach. The ground and the monster to fight.
pub async fn quest_ground(
    hunt: &Hunt<'_>,
    knowledge: &Knowledge,
    task: Task,
    map: &str,
) -> Result<Option<(Ground, String)>> {
    match task {
        Task::Hunt { mob } => {
            let Ok(data) = knowledge.db.mob(mob).await else {
                return Ok(None);
            };
            let name = data.name_english;
            Ok(hunt
                .ground_on(map, std::slice::from_ref(&name))
                .await?
                .map(|g| (g, name)))
        }
        Task::Gather { item } => {
            let Ok(data) = knowledge.db.item(item).await else {
                return Ok(None);
            };
            let droppers = dropper_names(knowledge, item).await;
            if let Some(g) = hunt.ground_on(map, &droppers).await? {
                let name = g
                    .targets
                    .iter()
                    .map(|(n, _, _)| n)
                    .find(|n| droppers.contains(n))
                    .cloned()
                    .unwrap_or_default();
                return Ok(Some((g, name)));
            }
            hunt.farm_ground(&data).await
        }
    }
}

async fn droppers(knowledge: &Knowledge, item: u32) -> Vec<u32> {
    let Ok(data) = knowledge.db.item(item).await else {
        return Vec::new();
    };
    data.dropped_by
        .iter()
        .filter(|d| !d.mvp && d.rate > 0)
        .filter_map(|d| d.mob.id_u32())
        .collect()
}

async fn dropper_names(knowledge: &Knowledge, item: u32) -> Vec<String> {
    let mut names = Vec::new();
    for id in droppers(knowledge, item).await {
        if let Ok(mob) = knowledge.db.mob(id).await {
            names.push(mob.name_english);
        }
    }
    names
}

/// The monsters the quests held still ask for: the one to hunt, or those that drop the loot.
pub async fn wanted_mobs(knowledge: &Knowledge, quests: &[TownQuest]) -> Vec<u32> {
    let mut mobs = Vec::new();
    for q in quests.iter().filter(|q| !q.done) {
        match q.task {
            Task::Hunt { mob } => mobs.push(mob),
            Task::Gather { item } => mobs.extend(droppers(knowledge, item).await),
        }
    }
    mobs.sort_unstable();
    mobs.dedup();
    mobs
}

/// |dx| + |dy|, as the plugin measures the way to a townsperson.
fn steps(a: (u16, u16), b: (u16, u16)) -> u32 {
    u32::from(a.0.abs_diff(b.0)) + u32::from(a.1.abs_diff(b.1))
}

/// The offer key of an Accept or Decline button (`accept:<town>:<npc>:<slot>:<level>`): the verb.
fn verb(widget: &str) -> &str {
    widget.split(':').next().unwrap_or_default()
}

impl Bot {
    /// In a town of the plugin, now and then: asks what its townsfolk offer, and takes what is worth taking. True when
    /// it acted (asked or walked), so the turn is over.
    pub async fn take_town_quests(&mut self, obs: &Observation) -> Result<bool> {
        let town = obs.map.name.clone();
        let level = obs.me.base_level;
        if !enabled(obs)
            || self.no_quest_towns.contains(&town)
            || !self.is_town(&town).await
            || self
                .offers_asked
                .as_ref()
                .is_some_and(|(t, l, at)| *t == town && *l == level && at.elapsed() < OFFERS_EVERY)
        {
            return Ok(false);
        }
        self.offers_asked = Some((town.clone(), level, Instant::now()));
        let reply = match self
            .act(Action::Plugin {
                plugin: PLUGIN.into(),
                name: OFFERS.into(),
                args: json!({}),
            })
            .await
        {
            Ok(reply) => reply,
            Err(error) => {
                tracing::debug!(%error, "town quest offers refused");
                return Ok(false);
            }
        };
        let offers: Offers = serde_json::from_value(reply["reply"].clone()).unwrap_or_default();
        if offers.error.is_some() || offers.town.is_empty() {
            tracing::debug!(%town, error = ?offers.error, "no town quests here");
            self.no_quest_towns.insert(town);
            return Ok(false);
        }
        self.memory.town_quests.towns.insert(offers.town.clone());
        let listed: Vec<Offer> = offers
            .offers
            .iter()
            .filter_map(|o| serde_json::from_value(o.clone()).ok())
            .collect();
        tracing::info!(%town, offers = listed.len(), free = offers.free_slots, "town quests offered");
        if offers.free_slots == 0 || listed.is_empty() {
            return Ok(true);
        }
        let mut picks = self.worth_taking(obs, listed).await?;
        picks.truncate(offers.free_slots as usize);
        while !picks.is_empty() && !self.preempted() {
            match self.accept_offer(&picks, offers.range.max(1)).await? {
                Some(i) => {
                    let o = picks.remove(i);
                    self.memory.note(format!(
                        "took a town quest from {} in {town}: {} on {} for {} base exp, {} job exp, {} zeny",
                        o.giver, o.title, o.map_name, o.base_exp, o.job_exp, o.zeny
                    ));
                    self.memory.town_quests.taken.push(Taken {
                        title: o.title,
                        map: o.map,
                        giver: o.giver,
                        town: town.clone(),
                        base_exp: o.base_exp,
                        job_exp: o.job_exp,
                        zeny: o.zeny,
                        done: false,
                    });
                }
                None => break,
            }
        }
        Ok(true)
    }

    /// The offers the bot can do, best first: on a ground within `QUEST_HOPS` warps where it beats what they ask for,
    /// worth the more experience the nearer.
    async fn worth_taking(&mut self, obs: &Observation, offers: Vec<Offer>) -> Result<Vec<Offer>> {
        let fighter = self.knowledge.fighter(obs).await?;
        let level = obs.me.base_level;
        let avoid_maps = self
            .memory
            .strategy
            .avoid_maps
            .iter()
            .cloned()
            .chain(self.recent_deadly_ways())
            .collect();
        let avoid_mobs = self
            .memory
            .strategy
            .avoided_mobs(level)
            .into_iter()
            .chain(self.memory.losing_to(level))
            .collect();
        let hunt = Hunt::new(&self.knowledge, &fighter, &obs.map.name, QUEST_HOPS)
            .avoiding_maps(&avoid_maps)
            .avoiding_mobs(&avoid_mobs);
        let mut worth = Vec::new();
        for offer in offers
            .into_iter()
            .filter(|o| o.x.is_some() && o.y.is_some())
        {
            let Some((ground, mob)) =
                quest_ground(&hunt, &self.knowledge, offer.task, &offer.map).await?
            else {
                tracing::debug!(title = %offer.title, map = %offer.map, "town quest out of reach");
                continue;
            };
            tracing::debug!(title = %offer.title, ground = %ground.map, %mob, "town quest worth taking");
            let value =
                (offer.base_exp as f32 + offer.job_exp as f32 * 0.5) / (1.0 + ground.hops as f32);
            worth.push((value, offer));
        }
        worth.sort_by(|a, b| b.0.total_cmp(&a.0));
        Ok(worth.into_iter().map(|(_, o)| o).collect())
    }

    /// Walks up to the giver of the first offer until a bubble shows, and accepts it when it is one of `picks` (the
    /// nearest townsperson with an offer speaks up first), else declines it. The index of the offer taken.
    async fn accept_offer(&mut self, picks: &[Offer], range: u32) -> Result<Option<usize>> {
        let target = &picks[0];
        let Some(mut at) = target.x.zip(target.y) else {
            return Ok(None);
        };
        let started = Instant::now();
        let mut waited = 0;
        let mut stepped_away = false;
        while started.elapsed() < SEEK && !self.preempted() {
            let obs = self.observe().await?;
            if obs.me.dead || obs.dialog.is_some() {
                return Ok(None);
            }
            let bubble = obs
                .plugins
                .get(PLUGIN)
                .and_then(|p| p.window(OFFER_WINDOW))
                .cloned();
            if let Some(window) = bubble {
                let buttons = window.buttons();
                let taken = window
                    .anchored_to()
                    .and_then(|unit| picks.iter().position(|o| o.npc_id == unit));
                let answer = if taken.is_some() { "accept" } else { "decline" };
                let Some(widget) = buttons.into_iter().find(|b| verb(b) == answer) else {
                    return Ok(None);
                };
                tracing::info!(giver = %window.title, %answer, "answering a town quest offer");
                self.act(Action::PluginUi {
                    plugin: PLUGIN.into(),
                    window: OFFER_WINDOW.into(),
                    widget,
                    kind: "click",
                })
                .await?;
                if taken.is_some() {
                    return Ok(taken);
                }
                // No other offer shows until the bot walks away from that townsperson
                stepped_away = false;
                waited = 0;
            }
            // The townsfolk walk their rounds: follow them while in sight
            if let Some(npc) = obs.npcs.iter().find(|n| n.id == target.npc_id) {
                at = (npc.x, npc.y);
            }
            let me = obs.pos();
            if steps(me, at) + 2 <= range {
                // The townsfolk's controller looks once a second
                waited += 1;
                if waited <= 4 {
                    Bot::pause(1000).await;
                    continue;
                }
                if stepped_away {
                    tracing::info!(giver = %target.giver, "the townsperson offers nothing after all");
                    return Ok(None);
                }
                // Another townsperson's answered offer holds the next one back until the bot walks off
                stepped_away = true;
                waited = 0;
                let side: i16 = if me.0 >= at.0 { 1 } else { -1 };
                let away = (me.0.saturating_add_signed(side * (range as i16 + 6)), me.1);
                self.act(Action::Move {
                    x: away.0,
                    y: away.1,
                    wait: true,
                })
                .await
                .ok();
                continue;
            }
            if self
                .act(Action::Move {
                    x: at.0,
                    y: at.1,
                    wait: true,
                })
                .await
                .is_err()
            {
                // The cell of a townsperson can be a door step: one next to it
                let near = (at.0.saturating_add(1), at.1);
                if self
                    .act(Action::Move {
                        x: near.0,
                        y: near.1,
                        wait: true,
                    })
                    .await
                    .is_err()
                {
                    return Ok(None);
                }
            }
        }
        Ok(None)
    }

    /// Keeps up with the quests held: the monsters the hunt prefers, and what came of the quests taken.
    pub async fn follow_town_quests(&mut self, obs: &Observation) {
        if !enabled(obs) {
            return;
        }
        let quests = held(obs);
        let tasks: Vec<(Task, bool)> = quests.iter().map(|q| (q.task, q.done)).collect();
        if tasks != self.town_quest_tasks {
            self.wanted_mobs = wanted_mobs(&self.knowledge, &quests).await;
            self.town_quest_tasks = tasks;
        }
        let book = &mut self.memory.town_quests;
        let mut lines = Vec::new();
        book.taken.retain_mut(|t| {
            match quests.iter().find(|q| q.title == t.title && q.map == t.map) {
                Some(q) => {
                    if q.done && !t.done {
                        lines.push(format!(
                            "town quest done: {}, the reward waits in a town of the townsfolk",
                            t.title
                        ));
                    }
                    t.done = q.done;
                    true
                }
                None => {
                    lines.push(if t.done {
                        format!(
                            "got the reward of the town quest {} from {}: {} base exp, {} job exp, {} zeny",
                            t.title, t.giver, t.base_exp, t.job_exp, t.zeny
                        )
                    } else {
                        format!("the town quest {} ran out of time", t.title)
                    });
                    false
                }
            }
        });
        for line in lines {
            self.memory.note(line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(plugins: Value) -> Observation {
        serde_json::from_value(json!({ "plugins": plugins })).unwrap()
    }

    #[test]
    fn the_quests_held_and_the_bubble_are_read() {
        let obs = observation(json!({
            "alive_towns": {
                "windows": [{
                    "id": "offer", "title": "Dalia", "width": 230, "closable": false,
                    "placement": {"type": "anchored", "unit_id": 110000123, "offset_y": 4},
                    "node": {"type": "column", "children": [
                        {"type": "text", "text": "Could you clear 12 of them?"},
                        {"type": "row", "children": [
                            {"type": "button", "id": "accept:0:123:500000:30", "label": "Accept"},
                            {"type": "button", "id": "decline:0:123:500000:30", "label": "Decline", "enabled": true}
                        ]}
                    ]}
                }],
                "data": {"quests": [
                    {"title": "Hunt Porings", "task": {"kind": "hunt", "mob": 1002}, "map": "prt_fild08",
                     "have": 4, "count": 12, "done": false, "seconds_left": 6000},
                    {"title": "Bring Jellopy", "task": {"kind": "gather", "item": 909}, "map": "prt_fild08",
                     "have": 5, "count": 5, "done": true, "seconds_left": 3000},
                    {"title": "Something new", "task": {"kind": "escort", "who": 7}}
                ]},
                "actions": [{"name": "offers", "description": "..."}]
            }
        }));
        assert!(enabled(&obs));
        let quests = held(&obs);
        assert_eq!(quests.len(), 2, "a task the bot does not know is left out");
        assert_eq!(quests[0].task, Task::Hunt { mob: 1002 });
        assert!(quests[1].done);
        assert_eq!(kept_items(&obs), vec![909]);
        let bubble = obs.plugins[PLUGIN].window(OFFER_WINDOW).unwrap();
        assert_eq!(bubble.anchored_to(), Some(110000123));
        let buttons = bubble.buttons();
        assert_eq!(buttons.len(), 2);
        assert_eq!(verb(&buttons[0]), "accept");
        assert_eq!(verb(&buttons[1]), "decline");
    }

    #[test]
    fn without_the_plugin_there_is_nothing() {
        let obs = observation(json!({"ui_shop": {"actions": [{"name": "buy"}]}}));
        assert!(!enabled(&obs));
        assert!(held(&obs).is_empty());
        assert!(!enabled(&Observation::default()));
    }

    #[test]
    fn offers_parse_with_a_giver_inside() {
        let offers: Offers = serde_json::from_value(json!({
            "town": "prontera", "range": 14, "free_slots": 3, "renews_in_secs": 1200,
            "offers": [{"npc_id": 110000005, "giver": "Bran", "title": "Hunt Lunatics",
                        "task": {"kind": "hunt", "mob": 1063}, "target": "Lunatic", "count": 15, "map": "prt_fild08",
                        "map_name": "Prontera Field", "minutes": 150, "base_exp": 300, "job_exp": 200, "zeny": 900}]
        }))
        .unwrap();
        let offer: Offer = serde_json::from_value(offers.offers[0].clone()).unwrap();
        assert_eq!(offer.x, None, "inside a building");
        assert_eq!((offers.range, offers.free_slots), (14, 3));
        let refused: Offers = serde_json::from_value(
            json!({"offers": [], "error": "Quests are offered in the towns of alive_towns only"}),
        )
        .unwrap();
        assert!(refused.error.is_some() && refused.town.is_empty());
    }
}
