//! Finding a potion shop through `/db`, walking to it, selling loot and buying potions and arrows.

use std::collections::HashMap;

use anyhow::Result;

use super::dialog::{DialogEnd, Intent, dialog_of};
use crate::api::observation::Dialog;
use crate::api::{Action, BuyLine, Observation, SellLine, dist};
use crate::bot::{Bot, Outcome};
use crate::goap::state::{ARROW_TO_BUY, ARROW_WANTED, HEAL_STOCK_WANTED, POTION_TO_BUY, arrow_count, is_loot_item};
use crate::knowledge::gear::uses_bow;
use crate::memory::ShopSpot;

/// Zeny kept after buying potions.
const ZENY_RESERVE: u64 = 100;
/// Warps searched for a shop.
const SHOP_SEARCH_HOPS: u32 = 8;
/// A shop this many warps closer than the known one replaces it.
const SHOP_SWITCH_HOPS: u32 = 2;
/// Share of the zeny left after potions that arrows may take.
const ARROW_BUDGET: f32 = 0.5;

impl Bot {
    /// The shop the bot uses: the seller of Red Potion nearest to `from`. The one it knows is kept unless another is
    /// at least `SHOP_SWITCH_HOPS` warps closer (checked once per map).
    pub async fn potion_shop(&mut self, from: &str) -> Result<Option<ShopSpot>> {
        if self.memory.potion_shop.is_some() && self.shop_checked_on.as_deref() == Some(from) {
            return Ok(self.memory.potion_shop.clone());
        }
        self.shop_checked_on = Some(from.to_owned());
        let item = self.knowledge.db.item(POTION_TO_BUY).await?;
        let hops: HashMap<String, u32> = self.knowledge.graph.neighbourhood(from, SHOP_SEARCH_HOPS).await?.into_iter().collect();
        let best = item
            .sold_by
            .iter()
            .filter(|s| !self.memory.bad_shops.contains(&s.key()))
            .filter_map(|s| hops.get(&s.map.id_str()).map(|h| (*h, s)))
            .min_by_key(|(h, s)| (*h, s.price));
        if let Some(known) = &self.memory.potion_shop {
            let known_hops = hops.get(&known.map);
            if known_hops.is_some_and(|k| best.is_none_or(|(b, _)| *k < b + SHOP_SWITCH_HOPS)) {
                return Ok(Some(known.clone()));
            }
        }
        let shop = best.map(|(_, s)| ShopSpot {
            map: s.map.id_str(),
            x: s.x,
            y: s.y,
            name: s.npc.name.split('#').next().unwrap_or_default().to_owned(),
            price: s.price,
        });
        if let Some(s) = &shop {
            self.memory.note(format!("potion shop: {} on {} at ({}, {}), {} z", s.name, s.map, s.x, s.y, s.price));
        }
        self.memory.potion_shop = shop.clone();
        Ok(shop)
    }

    fn shop_npc(obs: &Observation, shop: &ShopSpot) -> Option<u64> {
        obs.npcs.iter().filter(|n| dist((n.x, n.y), (shop.x, shop.y)) <= 3).min_by_key(|n| dist((n.x, n.y), (shop.x, shop.y))).map(|n| n.id)
    }

    pub async fn walk_to_shop(&mut self) -> Result<Outcome> {
        let obs = self.observe().await?;
        let Some(shop) = self.potion_shop(&obs.map.name).await? else { return Ok(Outcome::failed("no potion shop known")) };
        let outcome = self.walk_to_spot(&shop).await?;
        if outcome == Outcome::Done {
            self.near_shop = true;
        }
        Ok(outcome)
    }

    /// Walks next to the NPC of a shop on the current map.
    pub async fn walk_to_spot(&mut self, shop: &ShopSpot) -> Result<Outcome> {
        let obs = self.observe().await?;
        if obs.map.name != shop.map {
            return Ok(Outcome::failed(format!("the shop is on {}", shop.map)));
        }
        if dist(obs.pos(), (shop.x, shop.y)) > 4 {
            self.act(Action::Move { x: shop.x, y: shop.y.saturating_sub(2), wait: true }).await.ok();
        }
        let obs = self.observe().await?;
        if Self::shop_npc(&obs, shop).is_none() {
            return Ok(Outcome::failed(format!("no shop NPC near ({}, {}) on {}", shop.x, shop.y, shop.map)));
        }
        Ok(Outcome::Done)
    }

    /// Opens the potion shop on the `deal` side ("buy" or "sell") and returns its list.
    async fn open_shop(&mut self, deal: &'static str) -> Result<Option<Dialog>> {
        let Some(shop) = self.memory.potion_shop.clone() else { return Ok(None) };
        self.open_shop_at(&shop, deal).await
    }

    /// Opens the shop of the NPC at the spot on the `deal` side and returns its list.
    pub async fn open_shop_at(&mut self, shop: &ShopSpot, deal: &'static str) -> Result<Option<Dialog>> {
        let obs = self.observe().await?;
        let Some(npc) = Self::shop_npc(&obs, shop) else { return Ok(None) };
        let result = match self.act(Action::Use { target: npc }).await {
            Ok(r) => r,
            Err(error) if error.to_string().contains("reachable") => {
                // The NPC stands where the bot cannot get close (behind a counter): use another shop from now on.
                self.memory.note(format!("cannot reach {} on {}, looking for another shop", shop.name, shop.map));
                self.memory.mark_bad_shop(format!("{}:{},{}", shop.map, shop.x, shop.y));
                if self.memory.potion_shop.as_ref() == Some(shop) {
                    self.memory.potion_shop = None;
                }
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let dialog = dialog_of(&result);
        if dialog.as_ref().and_then(|d| d.prompt.as_ref()).is_none_or(|p| p.kind != "shop") {
            // Some shop NPCs talk first.
            let intent = Intent { goal: format!("open the shop to {deal}"), keywords: vec![deal.into(), "shop".into()], ..Default::default() };
            if !matches!(self.run_dialog(dialog, &intent).await?, DialogEnd::Shop) {
                return Ok(None);
            }
        }
        let result = self.act(Action::DialogShop { deal }).await?;
        Ok(dialog_of(&result))
    }

    pub async fn sell_loot(&mut self) -> Result<Outcome> {
        let before = self.observe().await?;
        let Some(dialog) = self.open_shop("sell").await? else { return Ok(Outcome::failed("could not open the shop")) };
        let sellable = dialog.prompt.map(|p| p.items).unwrap_or_default();
        let keep = self.keep.clone();
        let lines: Vec<SellLine> = before
            .inventory
            .iter()
            .filter(|i| is_loot_item(i, &keep))
            .filter(|i| sellable.iter().any(|s| s.index == Some(i.index) || (s.index.is_none() && s.item_id == i.item_id)))
            .map(|i| SellLine { index: i.index, amount: i.amount })
            .collect();
        if lines.is_empty() {
            self.act(Action::DialogClose).await.ok();
            return Ok(Outcome::failed("the shop buys none of the loot"));
        }
        self.act(Action::DialogSell { items: lines }).await?;
        let after = self.observe().await?;
        let earned = after.me.zeny.saturating_sub(before.me.zeny);
        self.memory.counters.zeny_earned += earned;
        self.memory.note(format!("sold loot for {earned} z, now {} z", after.me.zeny));
        Ok(Outcome::Done)
    }

    /// Buys Red Potions up to the healing the bot wants and, for a bow user, arrows.
    pub async fn buy_potions(&mut self) -> Result<Outcome> {
        let before = self.observe().await?;
        let Some(dialog) = self.open_shop("buy").await? else { return Ok(Outcome::failed("could not open the shop")) };
        let list = dialog.prompt.map(|p| p.items).unwrap_or_default();
        let mut lines = Vec::new();
        let mut zeny = before.me.zeny.saturating_sub(ZENY_RESERVE);
        let mut potion_price = None;
        if let Some(line) = list.iter().find(|l| l.item_id == POTION_TO_BUY) {
            let price = line.price.max(1);
            potion_price = Some(price);
            if let Some(shop) = &mut self.memory.potion_shop {
                shop.price = price;
            }
            self.knowledge.learn_heals(&before).await;
            let have = self.knowledge.heal_stock(&before) as f32;
            let per_potion = self.knowledge.db.item(POTION_TO_BUY).await.ok().and_then(|d| d.hp_heal()).unwrap_or(55.0)
                * crate::knowledge::item_heal_rate(&before);
            let wanted = ((before.me.max_hp as f32 * HEAL_STOCK_WANTED - have) / per_potion).ceil().max(0.0) as u32;
            let amount = wanted.min((zeny / price) as u32);
            if amount > 0 {
                zeny -= amount as u64 * price;
                lines.push(BuyLine { item_id: POTION_TO_BUY, amount });
            }
        }
        let arrows_wanted = if uses_bow(&before.me.job) { ARROW_WANTED.saturating_sub(arrow_count(&before)) } else { 0 };
        let mut arrows_sold = false;
        if arrows_wanted > 0
            && let Some(line) = list.iter().find(|l| l.item_id == ARROW_TO_BUY)
        {
            arrows_sold = true;
            let price = line.price.max(1);
            let amount = arrows_wanted.min(((zeny as f32 * ARROW_BUDGET) as u64 / price) as u32);
            if amount > 0 {
                lines.push(BuyLine { item_id: ARROW_TO_BUY, amount });
            }
        }
        if lines.is_empty() {
            self.act(Action::DialogClose).await.ok();
            return Ok(Outcome::failed(match potion_price {
                None if !arrows_sold => "the shop does not sell potions".to_string(),
                _ if arrows_wanted > 0 && !arrows_sold => "the shop does not sell arrows".to_string(),
                Some(price) => format!("cannot afford potions at {price} z with {} z", before.me.zeny),
                None => format!("cannot afford arrows with {} z", before.me.zeny),
            }));
        }
        self.act(Action::DialogBuy { items: lines.clone() }).await?;
        let after = self.observe().await?;
        let mut got = Vec::new();
        for line in &lines {
            let n = after.item_count(line.item_id).saturating_sub(before.item_count(line.item_id));
            if n > 0 {
                got.push(format!("{n} {}", if line.item_id == ARROW_TO_BUY { "Arrows" } else { "Red Potions" }));
            }
        }
        if got.is_empty() {
            return Ok(Outcome::failed("the purchase did not go through"));
        }
        self.memory.note(format!("bought {} for {} z", got.join(" and "), before.me.zeny.saturating_sub(after.me.zeny)));
        if arrows_wanted > 0 && !arrows_sold && potion_price.is_some() {
            // The potions are bought; the arrows have to come from elsewhere (crafting, another shop).
            return Ok(Outcome::failed("the shop does not sell arrows"));
        }
        Ok(Outcome::Done)
    }
}
