//! The break a bot takes in a town now and then, as players do between hunts: it sits down where others are, says
//! hello, keeps its party ad up for whoever looks for one, and sometimes opens a chat room, or, a Merchant with a
//! cart, a store of the loot no shop sells. Joining a party ends the break: finding one is what it is for. A party takes
//! its break together, when its leader calls one, and it lasts until the leader ends it.

use std::time::{Duration, Instant};

use anyhow::Result;
use rand::Rng;
use rand::seq::IndexedRandom;

use super::dialog::{DialogEnd, Intent, dialog_of};
use crate::api::{Action, Observation, VendLine, dist};
use crate::bot::Bot;
use crate::goap::state::is_loot_item;

/// A break whose town is not reached by then is skipped.
const REACH_WITHIN: Duration = Duration::from_secs(10 * 60);
/// Chance that a Merchant who can opens a store on a break.
const STORE_CHANCE: f64 = 0.3;
/// Chance that a bot opens a chat room on a break.
const ROOM_CHANCE: f64 = 0.35;
const ROOM_BASIC_SKILL: u32 = 4;
const EMOTE_BASIC_SKILL: u32 = 2;
/// Zeny a Merchant keeps after renting a cart, for potions.
const CART_SPARE: u64 = 1500;
/// The base level from which a character that is not reborn rents no cart, so as to be reborn at 99.
const NO_CART_FROM: u32 = 90;
/// Cells from the others the bot sits down at.
const COMPANY: u16 = 3;
/// Others within this many cells count as company.
const NEAR: u16 = 12;
const HELLOS: &[&str] = &["hi all", "hello", "hey everyone", "o/", "hi :)", "yo"];
const EMOTES: &[&str] = &["heh", "ok", "hmm", "lv", "ho", "no1", "swt"];

/// A break in a town.
#[derive(Debug, Clone)]
pub struct TownBreak {
    pub town: String,
    pub started: Instant,
    pub length: Duration,
    /// Since when the bot is in the town.
    pub arrived: Option<Instant>,
    /// The party's break, called by its leader: finding a party does not end it.
    pub with_party: bool,
    /// It sat down and set up what it shows.
    settled: bool,
    next_emote: Option<Instant>,
}

impl TownBreak {
    pub fn new(town: String, length: Duration) -> Self {
        Self {
            town,
            started: Instant::now(),
            length,
            arrived: None,
            with_party: false,
            settled: false,
            next_emote: None,
        }
    }

    /// The break of the party.
    pub fn of_party(self) -> Self {
        Self {
            with_party: true,
            ..self
        }
    }

    /// The break is over, or the town took too long to reach.
    pub fn over(&self) -> bool {
        match self.arrived {
            Some(at) => at.elapsed() >= self.length,
            None => self.started.elapsed() >= REACH_WITHIN,
        }
    }
}

/// Another member of the bot's party is in the game: a party found, which ends a break.
pub fn partied(obs: &Observation) -> bool {
    obs.party.as_ref().is_some_and(|p| {
        p.members
            .iter()
            .any(|m| m.online && !m.name.eq_ignore_ascii_case(&obs.me.name))
    })
}

/// Where to sit among the others: a few cells from the player with the most others around, or from the town's Kafra
/// when nobody is about.
fn company_spot(obs: &Observation) -> Option<(u16, u16)> {
    let others: Vec<(u16, u16)> = obs
        .players
        .iter()
        .filter(|p| !p.dead)
        .map(|p| (p.x, p.y))
        .collect();
    let crowd = others
        .iter()
        .max_by_key(|&&c| {
            others
                .iter()
                .filter(|&&o| dist(o, c) <= NEAR as u32)
                .count()
        })
        .copied();
    let kafra = || {
        obs.npcs
            .iter()
            .filter(|n| n.name.to_lowercase().contains("kafra"))
            .min_by_key(|n| dist((n.x, n.y), obs.pos()))
            .map(|n| (n.x, n.y))
    };
    let (x, y) = crowd.or_else(kafra)?;
    let mut rng = rand::rng();
    let mut off = |v: u16| {
        let d = rng.random_range(1..=COMPANY) as i32 * if rng.random_bool(0.5) { 1 } else { -1 };
        (v as i32 + d).max(1) as u16
    };
    // Off the warps: a crowd at a town's gate put a bot on the field outside
    (0..8)
        .map(|_| (off(x), off(y)))
        .find(|&c| !obs.near_warp(c, 2))
}

/// The title of a chat room: what the bot would like.
fn room_title(obs: &Observation, finds_parties: bool) -> String {
    let me = &obs.me;
    let alone = obs.party.as_ref().is_some_and(|p| p.members.len() <= 1);
    let mut rng = rand::rng();
    let title = if finds_parties && alone {
        format!("lv{} party, pm to join", me.base_level)
    } else if finds_parties && obs.party.is_none() {
        format!("lv{} {} lf party", me.base_level, me.job.to_lowercase())
    } else {
        ["afk a bit", "resting", "chilling", "brb"]
            .choose(&mut rng)
            .copied()
            .unwrap_or("resting")
            .to_owned()
    };
    title.chars().take(36).collect()
}

/// What a store asks for an item: the reference's price, twice what a shop pays at least, a little more or less.
fn store_price(price_buy: u64, sell_price: u32) -> u64 {
    let base = price_buy.max(sell_price as u64 * 2).max(2);
    let markup: f64 = rand::rng().random_range(0.9..1.3);
    ((base as f64 * markup).round() as u64).max(1)
}

impl Bot {
    /// One step of the break, standing in its town.
    pub async fn town_break_step(&mut self, obs: &Observation, b: &mut TownBreak) -> Result<()> {
        if b.arrived.is_none() {
            b.arrived = Some(Instant::now());
            tracing::info!(town = %b.town, minutes = b.length.as_secs() / 60, "on a break in town");
        }
        if !b.settled {
            b.settled = true;
            self.settle(obs).await?;
            return Ok(());
        }
        let company = obs
            .players
            .iter()
            .any(|p| dist((p.x, p.y), obs.pos()) <= NEAR as u32);
        let emotion = EMOTES.choose(&mut rand::rng()).copied();
        if company
            && obs.skill_level("NV_BASIC") >= EMOTE_BASIC_SKILL
            && b.next_emote.is_some_and(|at| Instant::now() >= at)
            && let Some(emotion) = emotion
        {
            self.act(Action::Emote {
                emotion: emotion.to_owned(),
            })
            .await
            .ok();
        }
        if b.next_emote.is_none_or(|at| Instant::now() >= at) {
            let secs = rand::rng().random_range(90..240);
            b.next_emote = Some(Instant::now() + Duration::from_secs(secs));
        }
        Bot::pause(2000).await;
        Ok(())
    }

    /// Walks over to the others, greets them, opens a store or a chat room maybe, and sits down.
    async fn settle(&mut self, obs: &Observation) -> Result<()> {
        if let Some((x, y)) = company_spot(obs)
            && dist((x, y), obs.pos()) > 1
        {
            if let Err(error) = self.act(Action::Move { x, y, wait: true }).await {
                tracing::debug!(%error, "could not walk over to the others");
            }
        }
        let obs = self.observe().await?;
        let (try_store, try_room, greet) = {
            let mut rng = rand::rng();
            (
                rng.random_bool(STORE_CHANCE),
                rng.random_bool(ROOM_CHANCE),
                HELLOS
                    .choose(&mut rng)
                    .filter(|_| rng.random_bool(0.6))
                    .copied(),
            )
        };
        let store = try_store && self.open_store(&obs).await?;
        let company = obs
            .players
            .iter()
            .any(|p| dist((p.x, p.y), obs.pos()) <= NEAR as u32);
        // Hello first: in a chat room, what the bot says stays in it
        if company && let Some(text) = greet {
            self.act(Action::Say {
                text: text.to_owned(),
            })
            .await
            .ok();
        }
        let finds_parties = self.party.as_ref().is_some_and(|p| p.config.auto);
        if !store
            && try_room
            && obs.skill_level("NV_BASIC") >= ROOM_BASIC_SKILL
            && obs.chat_room.is_none()
        {
            let title = room_title(&obs, finds_parties);
            match self
                .act(Action::RoomCreate {
                    title: title.clone(),
                    limit: 20,
                    public: true,
                })
                .await
            {
                Ok(_) => tracing::info!(%title, "opened a chat room"),
                Err(error) => tracing::debug!(%error, "chat room refused"),
            }
        }
        if !store && obs.me.action != "sitting" {
            self.act(Action::Sit).await.ok();
        }
        Ok(())
    }

    /// Ends the break: the store and the chat room close, the bot stands up.
    pub async fn end_town_break(&mut self, obs: &Observation) {
        if obs.vending.is_some() {
            self.act(Action::VendingClose).await.ok();
        }
        if obs.chat_room.as_ref().is_some_and(|r| r.npc.is_none()) {
            self.act(Action::RoomLeave).await.ok();
        }
        if obs.me.action == "sitting" {
            self.act(Action::Stand).await.ok();
        }
    }

    /// A Merchant with the Vending skill puts what no shop sells into its cart, renting one first if it can, and
    /// opens a store. Whether it opened one.
    async fn open_store(&mut self, obs: &Observation) -> Result<bool> {
        let slots = obs.skill_level("MC_VENDING");
        if slots == 0 {
            return Ok(false);
        }
        let goods = self.store_goods(obs, slots + 2).await;
        if goods.is_empty() {
            return Ok(false);
        }
        if obs.cart.is_none() && !self.rent_cart(obs).await? {
            return Ok(false);
        }
        for &(index, _, amount, _) in &goods {
            if let Err(error) = self.act(Action::CartPut { index, amount }).await {
                tracing::debug!(%error, index, "cart refused an item");
            }
        }
        let obs = self.observe().await?;
        let cart = obs.cart.clone().unwrap_or_default();
        let items: Vec<VendLine> = goods
            .iter()
            .filter_map(|&(_, item_id, _, price)| {
                let c = cart.iter().find(|c| c.item_id == item_id)?;
                Some(VendLine {
                    cart_index: c.index,
                    amount: c.amount,
                    price,
                })
            })
            .collect();
        if items.is_empty() {
            return Ok(false);
        }
        let names: Vec<&str> = goods
            .iter()
            .filter_map(|&(_, id, _, _)| cart.iter().find(|c| c.item_id == id))
            .map(|c| c.name.as_str())
            .collect();
        let title: String = match names.as_slice() {
            [one] => format!("S> {one}"),
            [one, ..] => format!("S> {one} & more"),
            [] => "S> loot".to_owned(),
        }
        .chars()
        .take(36)
        .collect();
        match self
            .act(Action::VendingOpen {
                title: title.clone(),
                items,
            })
            .await
        {
            Ok(_) => {
                tracing::info!(%title, "opened a store");
                self.memory
                    .note(format!("opened a store in {}: {title}", obs.map.name));
                Ok(true)
            }
            Err(error) => {
                tracing::info!(%error, "store refused");
                Ok(false)
            }
        }
    }

    /// Loot that no NPC shop sells, the most worth first, at most `slots` kinds: `(inventory index, item id, amount,
    /// price)`. Cards are kept out: a bot cannot tell what players pay for them.
    async fn store_goods(&self, obs: &Observation, slots: u32) -> Vec<(u32, u32, u32, u64)> {
        let mut goods = Vec::new();
        for item in obs
            .inventory
            .iter()
            .filter(|i| is_loot_item(i, &self.keep) && !i.kind.eq_ignore_ascii_case("card"))
        {
            let Ok(data) = self.knowledge.db.item(item.item_id).await else {
                continue;
            };
            if !data.sold_by.is_empty() {
                continue;
            }
            let price = store_price(data.price_buy, item.sell_price);
            goods.push((item.index, item.item_id, item.amount, price));
        }
        goods.sort_by_key(|&(_, _, amount, price)| std::cmp::Reverse(price * amount as u64));
        goods.truncate(slots as usize);
        goods
    }

    /// Rents a Pushcart at a Kafra in sight. Whether the bot has a cart afterwards.
    async fn rent_cart(&mut self, obs: &Observation) -> Result<bool> {
        // The Valkyrie takes no one with a cart into a new life, and nothing the bot can do gives a cart back
        if obs.skill_level("MC_PUSHCART") == 0
            || obs.me.zeny < CART_SPARE + 1000
            || !obs.me.reborn && obs.me.base_level >= NO_CART_FROM
        {
            return Ok(false);
        }
        let Some(kafra) = obs
            .npcs
            .iter()
            .filter(|n| n.name.to_lowercase().contains("kafra"))
            .min_by_key(|n| dist((n.x, n.y), obs.pos()))
            .map(|n| n.id)
        else {
            return Ok(false);
        };
        let result = match self.act(Action::Use { target: kafra }).await {
            Ok(r) => r,
            Err(error) => {
                tracing::debug!(%error, "could not talk to the Kafra");
                return Ok(false);
            }
        };
        let intent = Intent {
            goal: "Rent a Pushcart.".into(),
            keywords: vec!["Rent a Pushcart".into()],
            strict: true,
            ..Default::default()
        };
        let end = self.run_dialog(dialog_of(&result), &intent).await?;
        let after = self.observe().await?;
        let rented = after.cart.is_some();
        if rented {
            self.memory
                .note(format!("rented a Pushcart on {}", after.map.name));
        } else if !matches!(end, DialogEnd::Closed) {
            tracing::debug!(?end, "no cart from the Kafra");
        }
        Ok(rented)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Party, PartyMember, Player};

    #[test]
    fn a_party_member_in_the_game_ends_the_break() {
        let mut obs = Observation::default();
        obs.me.name = "Alfa".into();
        assert!(!partied(&obs));
        let member = |name: &str, online| PartyMember {
            name: name.into(),
            online,
            ..Default::default()
        };
        obs.party = Some(Party {
            members: vec![member("Alfa", true), member("Bravo", false)],
            ..Default::default()
        });
        assert!(
            !partied(&obs),
            "a party of one, or with members away, is no party found"
        );
        obs.party.as_mut().unwrap().members[1].online = true;
        assert!(partied(&obs));
    }

    #[test]
    fn the_bot_sits_near_the_crowd() {
        let mut obs = Observation::default();
        (obs.me.x, obs.me.y) = (100, 100);
        assert_eq!(company_spot(&obs), None);
        let player = |x, y| Player {
            x,
            y,
            ..Default::default()
        };
        obs.players = vec![
            player(20, 20),
            player(150, 150),
            player(152, 149),
            player(149, 153),
        ];
        let (x, y) = company_spot(&obs).unwrap();
        assert!(dist((x, y), (150, 150)) <= 3 + COMPANY as u32, "{x},{y}");
        obs.warps = vec![crate::api::observation::Warp {
            x: 147,
            y: 151,
            to_map: "prt_fild05".into(),
            ..Default::default()
        }];
        for _ in 0..20 {
            assert!(company_spot(&obs).is_none_or(|c| !obs.near_warp(c, 2)));
        }
    }

    #[test]
    fn a_store_asks_at_least_what_a_shop_pays_twice() {
        for _ in 0..20 {
            assert!(store_price(0, 100) >= 180);
            assert!(store_price(1000, 10) >= 900);
        }
    }
}
