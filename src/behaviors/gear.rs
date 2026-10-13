//! Wearing the best equipment at hand, buying better pieces, and keeping a bow supplied with arrows.

use std::time::{Duration, Instant};

use anyhow::Result;

use crate::api::{Action, Observation, dist};
use crate::bot::{Bot, Outcome};
use crate::goap::state::{ARROW_WANTED, arrow_count, is_loot_item};
use crate::knowledge::gear::{Slot, better_in_bag, uses_bow};
use crate::memory::ShopSpot;

/// Arrow Crafting is tried at most this often when there is nothing to craft from.
const CRAFT_EVERY: Duration = Duration::from_secs(60);
/// SP Arrow Crafting costs.
const CRAFT_SP: u32 = 10;
/// Crafts in one go.
const CRAFTS_AT_ONCE: usize = 10;

impl Bot {
    /// Puts on what is better than what the bot wears, and arrows when a bow has none. Tells whether it did something.
    pub async fn gear_reflexes(&mut self, obs: &Observation) -> Result<bool> {
        if obs.me.action == "attacking" || obs.dialog.is_some() {
            return Ok(false);
        }
        if let Some(item) = better_in_bag(obs) {
            let (index, name) = (item.index, item.name.clone());
            let slot = item.equip.as_ref().and_then(Slot::of_equip).map_or("slot", Slot::label);
            if self.act(Action::Equip { index }).await.is_ok() {
                self.memory.note(format!("now wearing {name} ({slot})"));
                return Ok(true);
            }
        }
        if uses_bow(&obs.me.job) {
            let ammo_on = obs.inventory.iter().any(|i| i.equipped && i.kind.eq_ignore_ascii_case("ammo"));
            let spare = obs.inventory.iter().filter(|i| !i.equipped && i.kind.eq_ignore_ascii_case("ammo")).max_by_key(|i| i.amount);
            if !ammo_on && let Some(arrows) = spare {
                let (index, name) = (arrows.index, arrows.name.clone());
                if self.act(Action::Equip { index }).await.is_ok() {
                    tracing::info!(%name, "arrows equipped");
                    return Ok(true);
                }
            }
            if self.craft_arrows(obs).await? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Arrow Crafting from loot when arrows run short: the source that makes the most arrows first.
    async fn craft_arrows(&mut self, obs: &Observation) -> Result<bool> {
        if obs.skill_level("AC_MAKINGARROW") == 0 || arrow_count(obs) >= ARROW_WANTED || obs.me.sp < CRAFT_SP {
            return Ok(false);
        }
        if self.last_craft.is_some_and(|t| t.elapsed() < CRAFT_EVERY) {
            return Ok(false);
        }
        self.last_craft = Some(Instant::now());
        let keep = self.keep.clone();
        let mut sources: Vec<(u32, u32)> = Vec::new();
        for item in obs.inventory.iter().filter(|i| is_loot_item(i, &keep) && i.kind.eq_ignore_ascii_case("etc")) {
            let Ok(data) = self.knowledge.db.item(item.item_id).await else { continue };
            let makes: u32 = data.arrow_crafting.as_source.iter().flat_map(|r| &r.makes).map(|m| m.amount).sum();
            if makes > 0 {
                sources.push((item.item_id, makes));
            }
        }
        sources.sort_by_key(|(_, makes)| std::cmp::Reverse(*makes));
        let Some(&(first, _)) = sources.first() else { return Ok(false) };
        let mut crafted = 0;
        for _ in 0..CRAFTS_AT_ONCE {
            let now = self.observe().await?;
            if arrow_count(&now) >= ARROW_WANTED || now.me.sp < CRAFT_SP {
                break;
            }
            let Some(&(source, _)) = sources.iter().find(|(id, _)| now.item_count(*id) > 0) else { break };
            if self.act(Action::Skill { skill: "AC_MAKINGARROW".into(), target: None }).await.is_err() {
                break;
            }
            let mut menu = None;
            for _ in 0..10 {
                Bot::pause(300).await;
                menu = self.observe().await?.skill_menu;
                if menu.is_some() {
                    break;
                }
            }
            let Some(menu) = menu else { break };
            let choice = menu.choices.iter().find(|c| c.choice.as_u64() == Some(source as u64)).map(|c| c.choice.clone());
            let Some(choice) = choice else {
                self.act(Action::SkillCancel).await.ok();
                break;
            };
            if self.act(Action::SkillChoose { choice }).await.is_err() {
                break;
            }
            crafted += 1;
        }
        if crafted > 0 {
            let after = self.observe().await?;
            self.memory.note(format!("crafted arrows {crafted} times (from item {first} first), {} arrows now", arrow_count(&after)));
        }
        Ok(crafted > 0)
    }

    /// Buys an item at a shop of the current map that sells it until the bot has `amount`, and wears it when
    /// `wear` (a piece of equipment).
    pub async fn buy_item(&mut self, item_id: u32, amount: u32, wear: bool) -> Result<Outcome> {
        let obs = self.observe().await?;
        let have = if wear { 0 } else { obs.item_count(item_id) };
        if have >= amount {
            return Ok(Outcome::Done);
        }
        let amount = amount - have;
        let data = self.knowledge.db.item(item_id).await?;
        let Some(place) = data
            .sold_by
            .iter()
            .filter(|s| s.map.id_str() == obs.map.name && !self.memory.bad_shops.contains(&s.key()))
            .min_by_key(|s| dist(obs.pos(), (s.x, s.y)))
        else {
            return Ok(Outcome::failed(format!("{} is not sold on {}", data.name_english, obs.map.name)));
        };
        let spot = ShopSpot {
            map: obs.map.name.clone(),
            x: place.x,
            y: place.y,
            name: place.npc.name.split('#').next().unwrap_or_default().to_owned(),
            price: place.price,
        };
        if let Outcome::Failed(reason) = self.walk_to_spot(&spot).await? {
            return Ok(Outcome::Failed(reason));
        }
        let Some(dialog) = self.open_shop_at(&spot, "buy").await? else {
            // Next to it and it will not open (out of sight, or it never gets to its list): another seller from now on
            self.memory.note(format!("could not open the shop of {} on {}, looking for another", spot.name, spot.map));
            self.memory.mark_bad_shop(place.key());
            return Ok(Outcome::failed("could not open the shop"));
        };
        let list = dialog.prompt.map(|p| p.items).unwrap_or_default();
        let Some(line) = list.iter().find(|l| l.item_id == item_id) else {
            self.act(Action::DialogClose).await.ok();
            return Ok(Outcome::failed(format!("{} does not sell {}", spot.name, data.name_english)));
        };
        if line.price * amount as u64 > obs.me.zeny {
            self.act(Action::DialogClose).await.ok();
            return Ok(Outcome::failed(format!(
                "cannot afford {amount} {} at {} z with {} z",
                data.name_english, line.price, obs.me.zeny
            )));
        }
        self.act(Action::DialogBuy { items: vec![crate::api::BuyLine { item_id, amount }] }).await?;
        let after = self.observe().await?;
        if after.item_count(item_id) < have + amount {
            return Ok(Outcome::failed("the purchase did not go through"));
        }
        self.memory.note(format!("bought {amount} {} for {} z", data.name_english, obs.me.zeny.saturating_sub(after.me.zeny)));
        if !wear {
            return Ok(Outcome::Done);
        }
        let Some(bought) = after.inventory.iter().find(|i| i.item_id == item_id && !i.equipped) else {
            return Ok(Outcome::failed("the purchase did not go through"));
        };
        let index = bought.index;
        if let Err(error) = self.act(Action::Equip { index }).await {
            tracing::warn!(%error, "could not wear what was bought");
        }
        Ok(Outcome::Done)
    }
}

/// The zeny a piece of equipment may cost: what is left after the potions the bot will need.
pub fn gear_budget(zeny: u64, potion_price: u64) -> u64 {
    zeny.saturating_sub(500 + potion_price.max(50) * 15)
}
