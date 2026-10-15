//! Curiosity: the bot talks to NPCs it does not know, to see what they do, as a player new to the world would. On the
//! way somewhere it stops for one near it now and then (`TALK_CHANCE`); entering a town, it may go to one anywhere in
//! it (`TOWN_CHANCE`); on a map it went to explore, it talks to a few. Where any will do, the decision model (or the
//! LLM) picks the one most worth meeting first. Menus get the entries that ask to hear more, never those that cost or
//! commit something (`curious_option`). What came of it is kept in `npcs_met` and goes to the journal, where the forum
//! finds the news worth telling.

use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;

use super::dialog::{DialogEnd, Intent, dialog_of};
use super::travel::GO_TOWNS;
use crate::api::observation::Npc;
use crate::api::{Action, Observation, dist};
use crate::bot::Bot;
use crate::memory::NpcNote;
use crate::quests::{self, Act};

/// Chance that the bot stops to talk to an unknown NPC it walks past, once per NPC and run.
const TALK_CHANCE: f64 = 0.15;
/// Chance that the bot, entering a town, goes to talk to one NPC there it does not know, wherever it stands.
pub const TOWN_CHANCE: f64 = 0.1;
/// NPCs whose name tells what they do, the bot knows already: Kafras and the warpers (`transports`).
const KNOWN_KINDS: [&str; 2] = ["kafra", "warper"];
/// Unknown NPCs the model ranks at most, the nearest ones.
const RANKED: usize = 10;
/// An NPC this near is one the bot walks past.
const NEAR: u32 = 12;
/// Least time between two stops on the way.
const TALK_GAP: Duration = Duration::from_secs(90);
/// NPCs talked to on a map the bot went to explore.
pub const EXPLORE_TALKS: usize = 3;
/// Lines of what an NPC said kept, and their length.
const SAID_KEPT: usize = 3;
const LINE_MAX: usize = 120;
/// Least HP share to stop for a talk.
const HP_TO_TALK: f32 = 0.5;

/// Why the bot may talk to an unknown NPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Curiosity {
    /// On the way: one near by, by chance.
    Near,
    /// Just entered a town: one anywhere in it.
    Town,
    /// On the map the goal explores.
    Explore,
}

/// The NPC's name as shown: the part before `#`.
fn shown_name(name: &str) -> &str {
    name.split('#').next().unwrap_or(name).trim()
}

/// The key of an NPC in `npcs_met`.
pub fn npc_key(map: &str, name: &str) -> String {
    format!("{map}:{}", shown_name(name))
}

/// The NPCs the quests talk to, as `map:name` (lowercase): the bot knows them already.
fn quest_npcs() -> &'static HashSet<String> {
    static NPCS: OnceLock<HashSet<String>> = OnceLock::new();
    NPCS.get_or_init(|| {
        let mut set = HashSet::new();
        for quest in quests::all() {
            for stage in &quest.stages {
                match &stage.act {
                    Act::Talk(t) => set.insert(npc_key(t.map, t.npc).to_lowercase()),
                    Act::Room { map, npc, .. } => set.insert(npc_key(map, npc).to_lowercase()),
                    _ => false,
                };
            }
        }
        set
    })
}

/// Text without the colour codes of NPC dialogues (`^FF0000`).
fn uncoloured(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find('^') {
        out.push_str(&rest[..at]);
        let code = rest
            .get(at + 1..at + 7)
            .is_some_and(|h| h.chars().all(|c| c.is_ascii_hexdigit()));
        rest = if code {
            &rest[at + 7..]
        } else {
            &rest[at + 1..]
        };
    }
    out.push_str(rest);
    out.trim().to_owned()
}

/// The first lines an NPC said, without the `[Name]` headers.
fn first_lines(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|l| uncoloured(l))
        .filter(|l| !l.is_empty() && !(l.starts_with('[') && l.ends_with(']')))
        .take(SAID_KEPT)
        .map(|l| l.chars().take(LINE_MAX).collect())
        .collect()
}

/// What the bot holds, to tell what a talk changed.
struct Holdings {
    map: String,
    job: String,
    zeny: u64,
    items: Vec<(u32, String, u32)>,
    quests: Vec<(u32, String)>,
}

impl Holdings {
    fn of(obs: &Observation) -> Self {
        let mut items: Vec<(u32, String, u32)> = Vec::new();
        for i in &obs.inventory {
            match items.iter_mut().find(|(id, _, _)| *id == i.item_id) {
                Some(e) => e.2 += i.amount,
                None => items.push((i.item_id, i.name.clone(), i.amount)),
            }
        }
        Self {
            map: obs.map.name.clone(),
            job: obs.me.job.clone(),
            zeny: obs.me.zeny,
            items,
            quests: obs.quests.iter().map(|q| (q.id, q.title.clone())).collect(),
        }
    }

    fn count(&self, id: u32) -> u32 {
        self.items
            .iter()
            .find(|(i, _, _)| *i == id)
            .map_or(0, |e| e.2)
    }
}

/// What a talk did, in a few words, from what the bot held before and after it.
fn what_it_did(before: &Holdings, after: &Holdings, end: &DialogEnd, menu: &[String]) -> String {
    let mut did: Vec<String> = Vec::new();
    if matches!(end, DialogEnd::Shop) {
        did.push("sells things".into());
    }
    if after.map != before.map {
        did.push(format!("sent me to {}", after.map));
    }
    if after.job != before.job {
        did.push(format!("made me a {}", after.job));
    }
    for (id, title) in &after.quests {
        if !before.quests.iter().any(|(b, _)| b == id) {
            let title = if title.is_empty() {
                format!("#{id}")
            } else {
                title.clone()
            };
            did.push(format!("started the quest {title}"));
        }
    }
    for (id, name, n) in &after.items {
        let had = before.count(*id);
        if *n > had {
            did.push(format!("gave me {} {name}", n - had));
        }
    }
    for (id, name, n) in &before.items {
        let has = after.count(*id);
        if *n > has {
            did.push(format!("took {} {name}", n - has));
        }
    }
    if after.zeny > before.zeny {
        did.push(format!("gave me {} zeny", after.zeny - before.zeny));
    } else if after.zeny < before.zeny {
        did.push(format!("took {} zeny", before.zeny - after.zeny));
    }
    if did.is_empty() && !menu.is_empty() {
        did.push(format!("offers {}", menu.join(", ")));
    }
    if did.is_empty() {
        did.push("talks".into());
    }
    did.join("; ")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl Bot {
    /// The NPCs of the map the bot does not know, nearest first: not met yet, not a shop, Kafra or warper, no quest's, in the
    /// bot's part of the map.
    async fn unknown_npcs(&self, obs: &Observation) -> Vec<Npc> {
        let map = &obs.map.name;
        let shops: Vec<(u16, u16)> = match self.knowledge.db.map(map).await {
            Ok(data) => data
                .npcs
                .iter()
                .filter(|n| n.shop)
                .map(|n| (n.x, n.y))
                .collect(),
            Err(_) => Vec::new(),
        };
        let me = obs.pos();
        let mut npcs: Vec<Npc> = obs
            .npcs
            .iter()
            .filter(|n| {
                let name = shown_name(&n.name);
                let key = npc_key(map, &n.name);
                !name.is_empty()
                    && !KNOWN_KINDS.iter().any(|k| name.to_lowercase().contains(k))
                    && !self.memory.npcs_met.contains_key(&key)
                    && !self.npcs_passed.contains(&key)
                    && !quest_npcs().contains(&key.to_lowercase())
                    && !shops.iter().any(|s| dist(*s, (n.x, n.y)) <= 1)
                    && n.waiting_room.is_none()
                    && !matches!((n.region, obs.me.region), (Some(a), Some(b)) if a != b)
            })
            .cloned()
            .collect();
        npcs.sort_by_key(|n| dist((n.x, n.y), me));
        // NPCs of one name (four Guards) are one to meet: the nearest
        let mut seen = HashSet::new();
        npcs.retain(|n| seen.insert(shown_name(&n.name).to_owned()));
        npcs
    }

    /// Whether `map` is a town.
    pub async fn is_town(&self, map: &str) -> bool {
        GO_TOWNS.contains(&map) || self.knowledge.db.map(map).await.is_ok_and(|m| m.is_town())
    }

    /// Maybe talks to an unknown NPC, `why` telling which. True when it talked (it may stand on another map since).
    pub async fn curious(&mut self, obs: &Observation, why: Curiosity) -> Result<bool> {
        if self.following
            || self.preempted()
            || obs.map.name.starts_with("new_")
            || obs.me.dead
            || obs.me.hp_frac() < HP_TO_TALK
            || obs.dialog.is_some()
            || (why != Curiosity::Explore
                && self.last_curious.is_some_and(|t| t.elapsed() < TALK_GAP))
        {
            return Ok(false);
        }
        let me = obs.pos();
        let unknown = self.unknown_npcs(obs).await;
        let npc = match why {
            Curiosity::Near => {
                let mut picked = None;
                for npc in unknown
                    .into_iter()
                    .take_while(|n| dist((n.x, n.y), me) <= NEAR)
                {
                    self.npcs_passed.insert(npc_key(&obs.map.name, &npc.name));
                    if rand::random::<f64>() < TALK_CHANCE {
                        picked = Some(npc);
                        break;
                    }
                }
                picked
            }
            Curiosity::Town | Curiosity::Explore => self.npc_to_meet(obs, unknown).await,
        };
        let Some(npc) = npc else {
            return Ok(false);
        };
        tracing::info!(?why, name = %shown_name(&npc.name), "curious: going to an NPC");
        self.talk_curiously(obs, &npc).await?;
        Ok(true)
    }

    /// The unknown NPC most worth meeting first among the nearest ones: the decision model's (or the LLM's) pick, else
    /// one with a quest icon, else the nearest.
    async fn npc_to_meet(&self, obs: &Observation, mut npcs: Vec<Npc>) -> Option<Npc> {
        npcs.truncate(RANKED);
        if npcs.len() > 1
            && let Some(helper) = &self.helper
        {
            let me = obs.pos();
            let lines: Vec<String> = npcs
                .iter()
                .map(|n| {
                    let icon = if n.quest_icon {
                        ", a quest icon above it"
                    } else {
                        ""
                    };
                    format!(
                        "{} ({} cells away{icon})",
                        shown_name(&n.name),
                        dist((n.x, n.y), me)
                    )
                })
                .collect();
            let m = &obs.me;
            let situation = format!(
                "A level {}/{} {} in Ragnarok Online is on {} and may talk to one NPC there it has never met, to \
discover a quest, or news to tell other players.",
                m.base_level, m.job_level, m.job, obs.map.name
            );
            match helper.npc_to_meet(&situation, &lines).await {
                Ok(i) if i < npcs.len() => {
                    tracing::info!(pick = %lines[i], among = ?lines, "curious: the model picked an NPC");
                    return Some(npcs.swap_remove(i));
                }
                Ok(i) => tracing::warn!(i, "curious: the model picked no NPC of the list"),
                Err(error) => tracing::warn!(
                    error = format!("{error:#}"),
                    "curious: could not rank the NPCs"
                ),
            }
        }
        let icon = npcs.iter().position(|n| n.quest_icon).unwrap_or(0);
        (!npcs.is_empty()).then(|| npcs.swap_remove(icon))
    }

    /// Talks to `npc` to see what it does, and remembers it.
    async fn talk_curiously(&mut self, obs: &Observation, npc: &Npc) -> Result<()> {
        self.last_curious = Some(Instant::now());
        let before = Holdings::of(obs);
        let (map, name) = (obs.map.name.clone(), shown_name(&npc.name).to_owned());
        tracing::info!(%name, %map, x = npc.x, y = npc.y, "curious: talking to an unknown NPC");
        let mut note = NpcNote {
            map: map.clone(),
            name: name.clone(),
            x: npc.x,
            y: npc.y,
            at: now(),
            ..Default::default()
        };
        let result = match self.act(Action::Use { target: npc.id }).await {
            Ok(r) => r,
            // Out of reach from here, maybe not from elsewhere: left alone for this run
            Err(error) => {
                tracing::info!(%name, %error, "curious: could not talk to it");
                self.npcs_passed.insert(npc_key(&map, &name));
                return Ok(());
            }
        };
        let intent = Intent {
            goal: format!("Hear what {name} has to say, without paying or giving anything"),
            curious: true,
            ..Default::default()
        };
        let end = self.run_dialog(dialog_of(&result), &intent).await?;
        if matches!(end, DialogEnd::Shop) {
            self.act(Action::DialogClose).await.ok();
        }
        // A script that warps or rewards the bot does it a moment after the dialogue closes.
        Bot::pause(800).await;
        let now_obs = self.observe().await?;
        if now_obs.dialog.is_some() {
            self.act(Action::DialogClose).await.ok();
        }
        note.does = what_it_did(&before, &Holdings::of(&now_obs), &end, &self.first_menu);
        note.said = first_lines(&self.last_dialog);
        note.menu = self.first_menu.clone();
        tracing::info!(%name, %map, does = %note.does, said = ?note.said, ?end, "curious: met an NPC");
        self.memory
            .note(format!("met {name} on {map}: {}", note.does));
        if now_obs.map.name != map {
            self.memory.visited_maps.insert(now_obs.map.name.clone());
        }
        self.memory.npcs_met.insert(npc_key(&map, &name), note);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holdings(
        map: &str,
        zeny: u64,
        items: &[(u32, &str, u32)],
        quests: &[(u32, &str)],
    ) -> Holdings {
        Holdings {
            map: map.into(),
            job: "Novice".into(),
            zeny,
            items: items
                .iter()
                .map(|(i, n, a)| (*i, (*n).to_owned(), *a))
                .collect(),
            quests: quests.iter().map(|(i, t)| (*i, (*t).to_owned())).collect(),
        }
    }

    #[test]
    fn a_talk_is_told_by_what_it_changed() {
        let before = holdings("prontera", 100, &[(501, "Red Potion", 3)], &[]);
        let after = holdings(
            "prontera",
            60,
            &[(501, "Red Potion", 1), (909, "Jellopy", 2)],
            &[(1000, "The Lost Child")],
        );
        assert_eq!(
            what_it_did(&before, &after, &DialogEnd::Closed, &[]),
            "started the quest The Lost Child; gave me 2 Jellopy; took 2 Red Potion; took 40 zeny"
        );
        let moved = holdings("izlude", 100, &[(501, "Red Potion", 3)], &[]);
        assert_eq!(
            what_it_did(&before, &moved, &DialogEnd::Closed, &[]),
            "sent me to izlude"
        );
        assert_eq!(
            what_it_did(
                &before,
                &before,
                &DialogEnd::Closed,
                &["Who are you?".into(), "Bye".into()]
            ),
            "offers Who are you?, Bye"
        );
        assert_eq!(
            what_it_did(&before, &before, &DialogEnd::Shop, &[]),
            "sells things"
        );
        assert_eq!(
            what_it_did(&before, &before, &DialogEnd::Closed, &[]),
            "talks"
        );
    }

    #[test]
    fn what_an_npc_said_is_kept_without_colours_and_headers() {
        let lines: Vec<String> = [
            "[Old Man]",
            "Have you seen ^0000FFmy grandson^000000?",
            "",
            "He went west.",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            first_lines(&lines),
            vec!["Have you seen my grandson?", "He went west."]
        );
    }

    #[test]
    fn quest_npcs_are_known() {
        assert!(!quest_npcs().is_empty());
        assert_eq!(npc_key("prontera", "Guard#prt1"), "prontera:Guard");
    }
}
