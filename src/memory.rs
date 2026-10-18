//! What the bot remembers across runs, saved as `state/<name>.json`: the strategist's state (Σ), counters,
//! and facts learned while playing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::strategist::skill_state::StrategicState;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Counters {
    pub kills: u64,
    pub deaths: u64,
    pub fights_lost: u64,
    pub loot_picked: u64,
    pub zeny_earned: u64,
    pub llm_calls: u64,
    pub llm_invalid: u64,
    pub llm_prompt_chars: u64,
}

/// A shop the bot uses: map, NPC position, and the item it buys there.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ShopSpot {
    pub map: String,
    pub x: u16,
    pub y: u16,
    pub name: String,
    pub price: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Memory {
    /// The strategist's state Σ: only the strategist changes it, through validated patches.
    pub strategy: StrategicState,
    pub counters: Counters,
    pub visited_maps: BTreeSet<String>,
    /// Kills per monster name.
    pub kills_by_mob: BTreeMap<String, u64>,
    /// Deaths per monster name that was being fought, or per map.
    pub deaths_by_cause: BTreeMap<String, u64>,
    /// Fights won and lost per monster since a base level, to stop picking the ones the fight model gets wrong.
    pub fights: BTreeMap<String, FightRecord>,
    pub potion_shop: Option<ShopSpot>,
    /// Shops the bot could not reach (an NPC behind a counter), as `map:x,y`.
    pub bad_shops: Vec<String>,
    /// Warps the bot could not take (a guessed way back, a quest entrance), as `from:x,y>to`.
    pub bad_warps: Vec<String>,
    /// Recent happenings for the strategist, newest last, a few dozen at most.
    pub journal: Vec<String>,
    /// Where the bot stands in each quest it took up.
    pub quests: crate::quests::QuestBook,
    /// The NPCs the bot talked to out of curiosity, as `map:name`, and what came of it.
    pub npcs_met: BTreeMap<String, NpcNote>,
    /// The second job the character is, or was before its rebirth: the line its reborn classes follow, which the
    /// server keeps in a script variable nobody sees.
    pub past_job: Option<String>,
    #[serde(skip)]
    path: PathBuf,
}

const JOURNAL_LEN: usize = 12;

/// What talking to an NPC out of curiosity showed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct NpcNote {
    pub map: String,
    pub name: String,
    pub x: u16,
    pub y: u16,
    /// What it turned out to do, in a few words: "sells things", "started the quest ...", "sent me to ...", "talks".
    pub does: String,
    /// The first things it said, colour codes removed.
    pub said: Vec<String>,
    /// The entries of the first menu it showed.
    pub menu: Vec<String>,
    /// Unix seconds of the talk.
    pub at: u64,
}

/// Fights with one monster since the bot was at `level`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FightRecord {
    pub level: u32,
    pub kills: u32,
    pub deaths: u32,
}

/// Levels after which a record no longer says how the bot fares: it starts over.
const RECORD_LEVELS: u32 = 5;
/// Deaths to a monster before its record counts, and the kills each of them must be worth.
const DEATHS_TO_AVOID: u32 = 3;
const KILLS_PER_DEATH: u32 = 5;

impl Memory {
    pub fn load(dir: &Path, name: &str) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join(format!("{}.json", name.to_lowercase()));
        let mut memory: Memory = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("parsing {}", path.display()))?,
            Err(_) => Memory::default(),
        };
        memory.path = path;
        memory.strategy.repair();
        memory.bad_shops.sort();
        memory.bad_shops.dedup();
        Ok(memory)
    }

    pub fn save(&self) -> Result<()> {
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Counts a fight with `mob` at base level `level`.
    pub fn record_fight(&mut self, mob: &str, level: u32, won: bool) {
        let record = self.fights.entry(mob.to_owned()).or_default();
        if record.level + RECORD_LEVELS <= level {
            *record = FightRecord {
                level,
                ..Default::default()
            };
        }
        if won {
            record.kills += 1;
        } else {
            record.deaths += 1;
        }
    }

    /// The monsters that keep killing the bot at about its level `level`, whatever the fight model says.
    pub fn losing_to(&self, level: u32) -> Vec<String> {
        self.fights
            .iter()
            .filter(|(_, r)| {
                r.level + RECORD_LEVELS > level
                    && r.deaths >= DEATHS_TO_AVOID
                    && r.kills < KILLS_PER_DEATH * r.deaths
            })
            .map(|(mob, _)| mob.clone())
            .collect()
    }

    /// Records a shop the bot cannot buy from (`map:x,y`), once.
    pub fn mark_bad_shop(&mut self, key: String) {
        if !self.bad_shops.contains(&key) {
            self.bad_shops.push(key);
        }
    }

    /// Adds a line to the journal the strategist reads.
    pub fn note(&mut self, line: impl Into<String>) {
        let line = line.into();
        tracing::info!(target: "journal", "{line}");
        self.journal.push(line);
        if self.journal.len() > JOURNAL_LEN {
            let extra = self.journal.len() - JOURNAL_LEN;
            self.journal.drain(..extra);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_monster_that_keeps_winning_is_left_alone_for_a_few_levels() {
        let mut m = Memory::default();
        for _ in 0..2 {
            m.record_fight("Spore", 20, true);
        }
        for _ in 0..2 {
            m.record_fight("Spore", 20, false);
        }
        assert!(m.losing_to(20).is_empty(), "two deaths are bad luck");
        m.record_fight("Spore", 21, false);
        assert_eq!(m.losing_to(21), vec!["Spore"]);
        // Enough kills for each death: the model was right
        for _ in 0..13 {
            m.record_fight("Spore", 21, true);
        }
        assert!(m.losing_to(21).is_empty());
        // Five levels on, it gets another chance
        for _ in 0..3 {
            m.record_fight("Thief Bug Female", 20, false);
        }
        assert_eq!(m.losing_to(24), vec!["Thief Bug Female"]);
        assert!(m.losing_to(25).is_empty());
    }
}
