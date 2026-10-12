//! Finding a potion shop through `/db`, walking to it, selling loot and buying potions.

use std::collections::HashMap;

use anyhow::Result;

use super::dialog::{DialogEnd, Intent, dialog_of};
use crate::api::observation::Dialog;
use crate::api::{Action, BuyLine, Observation, SellLine, dist};
use crate::bot::{Bot, Outcome};
use crate::goap::state::{HEAL_STOCK_WANTED, POTION_TO_BUY, is_loot};
use crate::memory::ShopSpot;

/// Zeny kept after buying potions.
const ZENY_RESERVE: u64 = 100;
/// Warps searched for a shop.
const SHOP_SEARCH_HOPS: u32 = 8;
/// A shop this many warps closer than the known one replaces it.
const SHOP_SWITCH_HOPS: u32 = 2;

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
            .filter(|s| !self.memory.bad_shops.contains(&format!("{}:{},{}", s.map.id_str(), s.x, s.y)))
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
        if obs.map.name != shop.map {
            return Ok(Outcome::failed(format!("the shop is on {}", shop.map)));
        }
        if dist(obs.pos(), (shop.x, shop.y)) > 4 {
            self.act(Action::Move { x: shop.x, y: shop.y.saturating_sub(2), wait: true }).await.ok();
        }
        let obs = self.observe().await?;
        if Self::shop_npc(&obs, &shop).is_none() {
            return Ok(Outcome::failed(format!("no shop NPC near ({}, {}) on {}", shop.x, shop.y, shop.map)));
        }
        self.near_shop = true;
        Ok(Outcome::Done)
    }

    /// Opens the shop on the `deal` side ("buy" or "sell") and returns its list.
    async fn open_shop(&mut self, deal: &'static str) -> Result<Option<Dialog>> {
        let obs = self.observe().await?;
        let Some(shop) = self.memory.potion_shop.clone() else { return Ok(None) };
        let Some(npc) = Self::shop_npc(&obs, &shop) else { return Ok(None) };
        let result = match self.act(Action::Use { target: npc }).await {
            Ok(r) => r,
            Err(error) if error.to_string().contains("reachable") => {
                // The NPC stands where the bot cannot get close (behind a counter): use another shop from now on.
                self.memory.note(format!("cannot reach {} on {}, looking for another shop", shop.name, shop.map));
                self.memory.bad_shops.push(format!("{}:{},{}", shop.map, shop.x, shop.y));
                self.memory.potion_shop = None;
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let dialog = dialog_of(&result);
        if dialog.as_ref().and_then(|d| d.prompt.as_ref()).is_none_or(|p| p.kind != "shop") {
            // Some shop NPCs talk first.
            let intent = Intent { goal: format!("open the shop to {deal}"), keywords: vec![deal.into(), "shop".into()], text_answer: None };
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
        let lines: Vec<SellLine> = before
            .inventory
            .iter()
            .filter(|i| is_loot(&i.kind, i.equipped))
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

    pub async fn buy_potions(&mut self) -> Result<Outcome> {
        let before = self.observe().await?;
        let Some(dialog) = self.open_shop("buy").await? else { return Ok(Outcome::failed("could not open the shop")) };
        let list = dialog.prompt.map(|p| p.items).unwrap_or_default();
        let Some(line) = list.iter().find(|l| l.item_id == POTION_TO_BUY) else {
            self.act(Action::DialogClose).await.ok();
            return Ok(Outcome::failed("the shop does not sell potions"));
        };
        let price = line.price.max(1);
        if let Some(shop) = &mut self.memory.potion_shop {
            shop.price = price;
        }
        self.knowledge.learn_heals(&before).await;
        let have = self.knowledge.heal_stock(&before) as f32;
        let per_potion = self.knowledge.db.item(POTION_TO_BUY).await.ok().and_then(|d| d.hp_heal()).unwrap_or(55.0)
            * crate::knowledge::item_heal_rate(&before);
        let wanted = ((before.me.max_hp as f32 * HEAL_STOCK_WANTED - have) / per_potion).ceil().max(0.0) as u32;
        let affordable = (before.me.zeny.saturating_sub(ZENY_RESERVE) / price) as u32;
        let amount = wanted.min(affordable);
        if amount == 0 {
            self.act(Action::DialogClose).await.ok();
            return Ok(Outcome::failed(format!("cannot afford potions at {price} z with {} z", before.me.zeny)));
        }
        self.act(Action::DialogBuy { items: vec![BuyLine { item_id: POTION_TO_BUY, amount }] }).await?;
        let after = self.observe().await?;
        let got = after.item_count(POTION_TO_BUY).saturating_sub(before.item_count(POTION_TO_BUY));
        self.memory.note(format!("bought {got} Red Potions for {} z", before.me.zeny.saturating_sub(after.me.zeny)));
        if got == 0 { Ok(Outcome::failed("the purchase did not go through")) } else { Ok(Outcome::Done) }
    }
}
