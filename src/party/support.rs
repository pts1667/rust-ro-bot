//! Helping the other members: the healer heals and buffs them, the tank draws the monsters that attack them, and
//! anyone with potions to spare hands some over, through the trade window, to a member who says it has none left.

use std::time::{Duration, Instant};

use anyhow::Result;

use super::chat::Said;
use super::{Role, SIGHT, member_ids};
use crate::api::{Action, Observation, dist};
use crate::bot::{Bot, Outcome};

/// HP share under which the healer heals a member.
const HEAL_BELOW: f32 = 0.6;
/// Range of Heal, Blessing, Increase AGI and Provoke, in cells.
const SKILL_RANGE: u32 = 9;
/// SP Heal takes at the levels a young Acolyte has.
const HEAL_SP: u32 = 13;
const PROVOKE_SP: u32 = 5;
/// The buffs the healer keeps up, and how often it casts them again (they last a minute or more).
const BUFFS: [&str; 2] = ["AL_BLESSING", "AL_INCAGI"];
const BUFF_EVERY: Duration = Duration::from_secs(60);
/// Buffs only while this share of SP is left, Heal comes first.
const BUFF_SP_SHARE: f32 = 0.5;
/// Healing items a bot keeps for itself: it gives only what it has over this.
const SPARE_ABOVE: u32 = 8;
const GIVE_AT_MOST: u32 = 5;
/// An open trade window is cancelled after this long.
const TRADE_TIMEOUT: Duration = Duration::from_secs(60);
const TRADE_STEP: Duration = Duration::from_secs(15);
const POLL_MS: u64 = 500;

/// How long a member `cells` away waits before it answers a call for potions: the nearest goes first, and the name
/// sets apart two at the same distance. The one that goes says so, the others hear it in time.
fn give_after(cells: u32, me: &str) -> Duration {
    let tie = me.bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64)) % 700;
    Duration::from_millis(cells as u64 * 400 + tie)
}

fn sp_frac(obs: &Observation) -> f32 {
    if obs.me.max_sp == 0 { 0.0 } else { obs.me.sp as f32 / obs.me.max_sp as f32 }
}

fn done_locking(phase: &str) -> bool {
    phase == "locked" || phase == "confirmed"
}

impl Bot {
    /// The role's part, one action at most. Returns whether it acted.
    pub(super) async fn support(&mut self, obs: &Observation) -> Result<bool> {
        let Some(play) = &self.party else { return Ok(false) };
        match play.role(&obs.me.job) {
            Role::Healer => self.heal_members(obs).await,
            Role::Tank => self.provoke(obs).await,
            Role::Fighter => Ok(false),
        }
    }

    /// Heals the member in sight with the lowest HP bar under `HEAL_BELOW` (or who asked), then keeps buffs up.
    async fn heal_members(&mut self, obs: &Observation) -> Result<bool> {
        let Some(party) = &obs.party else { return Ok(false) };
        let play = self.party.as_ref().expect("party play");
        if obs.skill_level("AL_HEAL") > 0 && obs.me.sp >= HEAL_SP {
            let worst = party
                .members
                .iter()
                .filter(|m| !m.name.eq_ignore_ascii_case(&obs.me.name) && m.map == obs.map.name)
                .filter_map(|m| {
                    let frac = m.hp_frac()?;
                    (frac < HEAL_BELOW || (play.wants_heal(&m.name) && frac < 1.0)).then_some((m.name.clone(), frac))
                })
                .filter_map(|(name, frac)| obs.player_near(&name, SKILL_RANGE).filter(|p| !p.dead).map(|p| (p.id, name, frac)))
                .min_by(|a, b| a.2.total_cmp(&b.2));
            if let Some((id, name, frac)) = worst {
                tracing::info!(%name, hp = format!("{:.0}%", frac * 100.0), "healing a member");
                self.act(Action::Skill { skill: "AL_HEAL".into(), target: Some(id) }).await.ok();
                return Ok(true);
            }
        }
        if sp_frac(obs) < BUFF_SP_SHARE || obs.me.action == "attacking" {
            return Ok(false);
        }
        let mut people: Vec<(u64, String)> =
            obs.players.iter().filter(|p| p.party && !p.dead && dist((p.x, p.y), obs.pos()) <= SKILL_RANGE).map(|p| (p.id, p.name.clone())).collect();
        people.push((obs.me.char_id, obs.me.name.clone()));
        for skill in BUFFS.into_iter().filter(|s| obs.skill_level(s) > 0) {
            for (id, name) in &people {
                let play = self.party.as_mut().expect("party play");
                let key = (name.clone(), skill);
                if play.buffs.get(&key).is_some_and(|at| at.elapsed() < BUFF_EVERY) {
                    continue;
                }
                play.buffs.insert(key, Instant::now());
                tracing::info!(%name, %skill, "buffing");
                self.act(Action::Skill { skill: skill.to_owned(), target: Some(*id) }).await.ok();
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Draws to the tank a monster that attacks another member.
    async fn provoke(&mut self, obs: &Observation) -> Result<bool> {
        if obs.skill_level("SM_PROVOKE") == 0 || obs.me.sp < PROVOKE_SP {
            return Ok(false);
        }
        let members = member_ids(obs);
        let Some(mob) = obs.mobs.iter().find(|m| {
            m.hp > 0
                && m.target.is_some_and(|t| t != obs.me.char_id && members.contains(&t))
                && dist((m.x, m.y), obs.pos()) <= SKILL_RANGE
        }) else {
            return Ok(false);
        };
        tracing::info!(mob = %mob.name, "provoking a monster off a member");
        self.act(Action::Skill { skill: "SM_PROVOKE".into(), target: Some(mob.id) }).await.ok();
        Ok(true)
    }

    /// The receiving side of a trade: a member's request is taken, anyone else's declined; once the partner locked
    /// its offer the bot locks (it gives nothing) and confirms.
    pub(super) async fn answer_trade(&mut self, obs: &Observation) -> Result<bool> {
        let play = self.party.as_mut().expect("party play");
        let Some(trade) = &obs.trade else {
            play.trade_since = None;
            return Ok(false);
        };
        let since = *play.trade_since.get_or_insert_with(Instant::now);
        let from_member = obs.party.as_ref().is_some_and(|p| p.members.iter().any(|m| m.name.eq_ignore_ascii_case(&trade.partner_name)));
        let action = if trade.incoming_request {
            tracing::info!(from = %trade.partner_name, accept = from_member, "trade request");
            Some(if from_member { Action::TradeAccept } else { Action::TradeDecline })
        } else if since.elapsed() > TRADE_TIMEOUT {
            Some(Action::TradeCancel)
        } else if trade.my_items.is_empty() && done_locking(&trade.partner_phase) {
            match trade.phase.as_str() {
                "accepted" => Some(Action::TradeLock),
                "locked" => Some(Action::TradeConfirm),
                _ => None,
            }
        } else {
            None
        };
        let Some(action) = action else { return Ok(false) };
        self.act(action).await.ok();
        Ok(true)
    }

    /// Hands potions to a member in sight who said it has none, when the bot has some to spare. The members nearer
    /// to it get the first go: whoever gives says so, and the others leave it.
    pub(super) async fn give_potions_if_asked(&mut self, obs: &Observation) -> Result<Option<Outcome>> {
        let Some(play) = &self.party else { return Ok(None) };
        if self.knowledge.heal_items(obs) <= SPARE_ABOVE {
            return Ok(None);
        }
        for (name, asked) in play.potion_requests() {
            if let Some(player) = obs.player_near(&name, SIGHT).filter(|p| p.party && !p.dead) {
                let (id, name) = (player.id, player.name.clone());
                if asked < give_after(dist((player.x, player.y), obs.pos()), &obs.me.name) {
                    continue;
                }
                if !self.say_to_party(Said::SendingPotions(name.clone())).await {
                    // Said something else a moment ago: next time
                    return Ok(None);
                }
                let outcome = self.give_potions(id, &name).await?;
                // Asked once, tried once: the member asks again if it still needs them
                self.party.as_mut().expect("party play").potions_given(&name);
                return Ok(Some(outcome));
            }
        }
        Ok(None)
    }

    /// Walks next to the member and trades it some healing items.
    async fn give_potions(&mut self, to: u64, name: &str) -> Result<Outcome> {
        let obs = self.observe().await?;
        let Some(potion) = obs
            .inventory
            .iter()
            .filter(|i| !i.equipped && self.knowledge.heal_of(i.item_id).is_some())
            .max_by_key(|i| i.amount)
            .cloned()
        else {
            return Ok(Outcome::failed("no potion to give"));
        };
        // Only what is over the bot's own reserve
        let spare = self.knowledge.heal_items(&obs).saturating_sub(SPARE_ABOVE);
        let amount = spare.min(potion.amount).min(GIVE_AT_MOST);
        if amount == 0 {
            return Ok(Outcome::failed("no potion to spare"));
        }
        let Some(player) = obs.players.iter().find(|p| p.id == to) else { return Ok(Outcome::failed(format!("{name} is out of sight"))) };
        if dist((player.x, player.y), obs.pos()) > 2 {
            self.act(Action::Move { x: player.x, y: player.y, wait: true }).await.ok();
        }
        if let Err(error) = self.act(Action::TradeRequest { target: to }).await {
            return Ok(Outcome::failed(format!("cannot trade with {name}: {error}")));
        }
        tracing::info!(%name, item = %potion.name, amount, "giving potions");
        if !self.wait_trade(|t| t.phase == "accepted").await? {
            self.act(Action::TradeCancel).await.ok();
            return Ok(Outcome::failed(format!("{name} did not take the trade")));
        }
        self.act(Action::TradeOffer { index: potion.index, amount }).await?;
        self.act(Action::TradeLock).await?;
        if !self.wait_trade(|t| done_locking(&t.partner_phase)).await? {
            self.act(Action::TradeCancel).await.ok();
            return Ok(Outcome::failed(format!("{name} did not lock the trade")));
        }
        self.act(Action::TradeConfirm).await?;
        let deadline = Instant::now() + TRADE_STEP;
        while Instant::now() < deadline {
            Bot::pause(POLL_MS).await;
            if self.observe().await?.trade.is_none() {
                self.memory.note(format!("gave {amount} {} to {name}", potion.name));
                return Ok(Outcome::Done);
            }
        }
        self.act(Action::TradeCancel).await.ok();
        Ok(Outcome::failed(format!("the trade with {name} did not go through")))
    }

    /// Waits for the open trade to reach `ready`; false when it closes or takes too long.
    async fn wait_trade(&self, ready: impl Fn(&crate::api::Trade) -> bool) -> Result<bool> {
        let deadline = Instant::now() + TRADE_STEP;
        // A request takes a game tick to open the window
        let mut seen = false;
        while Instant::now() < deadline {
            Bot::pause(POLL_MS).await;
            match self.observe().await?.trade {
                Some(trade) if ready(&trade) => return Ok(true),
                Some(_) => seen = true,
                None if seen => return Ok(false),
                None => {}
            }
        }
        Ok(false)
    }
}
