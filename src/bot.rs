//! The bot: its API handle, knowledge and memory, and the helpers the behaviours share.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::Value;

use crate::api::{Action, BotApi, Observation};
use crate::knowledge::Knowledge;
use crate::knowledge::db::MobData;
use crate::memory::Memory;
use crate::strategist::helper::Helper;

/// How a behaviour ended, when the API itself worked.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Done,
    Failed(String),
    /// The strategist set a new goal meanwhile: not a failure.
    Preempted,
}

impl Outcome {
    pub fn failed(reason: impl Into<String>) -> Self {
        Outcome::Failed(reason.into())
    }
}

pub struct Bot {
    pub api: BotApi,
    pub name: String,
    pub knowledge: Knowledge,
    pub memory: Memory,
    /// LLM helper for dialogues and chat, when the LLM is used.
    pub helper: Option<Helper>,
    /// The last action walked the bot to the shop it uses.
    pub near_shop: bool,
    /// The map the choice of shop was last checked from.
    pub shop_checked_on: Option<String>,
    /// Skill points could not be spent at this job level, so the planner stops trying until it changes.
    pub skills_stuck_at: Option<(String, u32)>,
    /// Status points could not be spent with this many points left.
    pub stats_stuck_at: Option<u32>,
    /// Search walks in a row that found nothing to fight.
    pub empty_searches: u32,
    mobs: HashMap<u32, Option<MobData>>,
    /// Monster ids not to attack again, until the given time.
    blacklist: HashMap<u64, Instant>,
    /// Sequence number of the strategist's latest directive, and of the one the current goal comes from.
    pub latest_directive: Arc<AtomicU64>,
    pub current_directive: u64,
    /// Someone spoke to the bot since its last turn of conversation: long behaviours stop so it answers.
    pub talked_to: AtomicBool,
    /// The last message looked at for that.
    pub talk_seq: AtomicU64,
    /// Playing in a party, when the bot was given one.
    pub party: Option<crate::party::PartyPlay>,
    /// Conversations and requests, see `social`.
    pub social: crate::social::Social,
    /// Items not to sell: what the bot's quests ask for.
    pub keep: Vec<u32>,
    /// When Arrow Crafting was last tried.
    pub last_craft: Option<Instant>,
    /// The spot the current goal is about, so a trip into a map of several rooms takes the right door.
    pub travel_spot: Option<(String, (u16, u16))>,
    /// The lines of the last conversation.
    pub last_dialog: Vec<String>,
}

impl Bot {
    pub fn new(
        api: BotApi,
        name: String,
        knowledge: Knowledge,
        memory: Memory,
        helper: Option<Helper>,
    ) -> Self {
        Self {
            api,
            name,
            knowledge,
            memory,
            helper,
            near_shop: false,
            shop_checked_on: None,
            skills_stuck_at: None,
            stats_stuck_at: None,
            empty_searches: 0,
            mobs: HashMap::new(),
            blacklist: HashMap::new(),
            latest_directive: Arc::new(AtomicU64::new(0)),
            current_directive: 0,
            party: None,
            social: Default::default(),
            talked_to: AtomicBool::new(false),
            talk_seq: AtomicU64::new(0),
            keep: Vec::new(),
            last_craft: None,
            travel_spot: None,
            last_dialog: Vec::new(),
        }
    }

    /// Whether a long behaviour should stop: the strategist changed its mind, or someone waits for an answer.
    pub fn preempted(&self) -> bool {
        (self.current_directive != 0
            && self.latest_directive.load(Ordering::SeqCst) > self.current_directive)
            || self.talked_to.load(Ordering::SeqCst)
    }

    pub async fn observe(&self) -> Result<Observation> {
        let obs = self.api.observation(&self.name).await?;
        let last = self
            .talk_seq
            .fetch_max(obs.last_message_seq(), Ordering::SeqCst);
        if obs
            .messages
            .iter()
            .any(|m| m.seq > last && crate::social::calls_for_answer(m, &obs.me.name))
        {
            self.talked_to.store(true, Ordering::SeqCst);
        }
        Ok(obs)
    }

    pub async fn act(&self, action: Action) -> Result<Value> {
        self.api.act(&self.name, &action).await
    }

    /// Database entry of a monster, cached (also when missing).
    pub async fn mob_info(&mut self, mob_id: u32) -> Option<MobData> {
        if let Some(m) = self.mobs.get(&mob_id) {
            return m.clone();
        }
        let m = self.knowledge.db.mob(mob_id).await.ok();
        self.mobs.insert(mob_id, m.clone());
        m
    }

    pub fn blacklist(&mut self, id: u64, secs: u64) {
        self.blacklist
            .insert(id, Instant::now() + Duration::from_secs(secs));
    }

    pub fn is_blacklisted(&mut self, id: u64) -> bool {
        let now = Instant::now();
        self.blacklist.retain(|_, until| *until > now);
        self.blacklist.contains_key(&id)
    }

    pub async fn pause(ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}
