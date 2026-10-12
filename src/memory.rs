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
    pub potion_shop: Option<ShopSpot>,
    /// Shops the bot could not reach (an NPC behind a counter), as `map:x,y`.
    pub bad_shops: Vec<String>,
    /// Warps the bot could not take (a guessed way back, a quest entrance), as `from:x,y>to`.
    pub bad_warps: Vec<String>,
    /// Recent happenings for the strategist, newest last, a few dozen at most.
    pub journal: Vec<String>,
    #[serde(skip)]
    path: PathBuf,
}

const JOURNAL_LEN: usize = 12;

impl Memory {
    pub fn load(dir: &Path, name: &str) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join(format!("{}.json", name.to_lowercase()));
        let mut memory: Memory = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?,
            Err(_) => Memory::default(),
        };
        memory.path = path;
        memory.strategy.repair();
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
