//! The symbolic world state GOAP plans over, derived from an observation.

use std::collections::BTreeMap;

use crate::api::Observation;
use crate::api::observation::InventoryItem;
use crate::knowledge::gear::uses_bow;

/// HP share under which the bot rests before fighting.
pub const HP_LOW: f32 = 0.5;
/// HP share at which resting stops.
pub const HP_FULL: f32 = 0.9;
/// The potion the bot buys.
pub const POTION_TO_BUY: u32 = 501;
/// Healing the bot must carry to count as supplied, in multiples of its max HP.
pub const HEAL_STOCK_MIN: f32 = 1.0;
/// Healing the bot buys up to, in multiples of its max HP.
pub const HEAL_STOCK_WANTED: f32 = 2.5;
/// A bow user with fewer arrows than this goes to buy some.
pub const ARROW_MIN: u32 = 150;
/// Arrows a bow user buys (or crafts) up to.
pub const ARROW_WANTED: u32 = 800;
/// The arrow the bot buys.
pub const ARROW_TO_BUY: u32 = 1750;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct WorldState {
    pub map: String,
    pub in_training: bool,
    pub hp_ok: bool,
    pub hp_full: bool,
    pub has_potions: bool,
    pub loot_to_sell: bool,
    pub near_shop: bool,
    pub can_afford_potions: bool,
    pub status_points: bool,
    pub skill_points: bool,
    pub job: String,
    pub job_change_ready: bool,
    /// Set by the hunt action only: the observation never says a fight just happened.
    pub hunted: bool,
    /// A bow user running out of arrows.
    pub ammo_low: bool,
    /// The items in the inventory: id and amount.
    pub items: BTreeMap<u32, u32>,
    /// The stage each quest the bot took up is at.
    pub quest_stages: BTreeMap<String, usize>,
}

/// Facts that come from memory rather than the observation.
#[derive(Debug, Clone, Default)]
pub struct Extra {
    /// The last action walked the bot to the shop of the current plan.
    pub near_shop: bool,
    /// What the shop asks for one potion.
    pub potion_price: u64,
    /// HP the healing items in the inventory would give back.
    pub heal_hp: u32,
    /// Items not to sell: what the bot's quests ask for.
    pub keep: Vec<u32>,
    /// The stage each quest the bot took up is at.
    pub quest_stages: BTreeMap<String, usize>,
}

pub fn is_loot(kind: &str, equipped: bool) -> bool {
    if equipped {
        return false;
    }
    // Healing items have the kind "Healing", so they are never loot. Arrows are kept for a bow.
    // Etc items (Jellopy, Feathers…) and unequipped gear; usable items other than healing are kept for now.
    matches!(kind, "Etc" | "Weapon" | "Armor" | "Card" | "") || kind.eq_ignore_ascii_case("etc")
}

/// An inventory item to sell: loot the bot's quests do not ask for.
pub fn is_loot_item(item: &InventoryItem, keep: &[u32]) -> bool {
    is_loot(&item.kind, item.equipped) && !keep.contains(&item.item_id)
}

/// The arrows the bot carries, equipped or not.
pub fn arrow_count(obs: &Observation) -> u32 {
    obs.inventory.iter().filter(|i| i.kind.eq_ignore_ascii_case("ammo")).map(|i| i.amount).sum()
}

impl WorldState {
    pub fn from_observation(obs: &Observation, extra: &Extra) -> Self {
        let me = &obs.me;
        let loot_items = obs.inventory.iter().filter(|i| is_loot_item(i, &extra.keep)).map(|i| i.amount).sum::<u32>();
        let price = extra.potion_price.max(1);
        WorldState {
            map: obs.map.name.clone(),
            in_training: obs.map.name.starts_with("new_"),
            hp_ok: me.hp_frac() >= HP_LOW,
            hp_full: me.hp_frac() >= HP_FULL,
            has_potions: extra.heal_hp as f32 >= me.max_hp as f32 * HEAL_STOCK_MIN,
            loot_to_sell: loot_items >= 1,
            near_shop: extra.near_shop,
            can_afford_potions: me.zeny >= price * 3,
            status_points: me.status_points > 0 && can_raise_any(obs),
            skill_points: me.skill_points > 0,
            job: me.job.clone(),
            job_change_ready: !next_jobs(&me.job).is_empty()
                && me.job_level >= if me.job.eq_ignore_ascii_case("novice") { 10 } else { 40 },
            hunted: false,
            ammo_low: uses_bow(&me.job) && arrow_count(obs) < ARROW_MIN,
            items: obs.inventory.iter().fold(BTreeMap::new(), |mut m, i| {
                *m.entry(i.item_id).or_default() += i.amount;
                m
            }),
            quest_stages: extra.quest_stages.clone(),
        }
    }
}

/// The jobs the Job Master offers to a character of `job` (rathena class names).
pub fn next_jobs(job: &str) -> &'static [&'static str] {
    match job.to_lowercase().as_str() {
        "novice" => &["Swordman", "Mage", "Archer", "Acolyte", "Merchant", "Thief"],
        "swordman" | "swordsman" => &["Knight", "Crusader"],
        "mage" | "magician" => &["Wizard", "Sage"],
        "archer" => &["Hunter", "Bard", "Dancer"],
        "acolyte" => &["Priest", "Monk"],
        "merchant" => &["Blacksmith", "Alchemist"],
        "thief" => &["Assassin", "Rogue"],
        _ => &[],
    }
}

/// True when the status points cover raising some stat.
pub fn can_raise_any(obs: &Observation) -> bool {
    let s = &obs.me.stats;
    [s.str, s.agi, s.vit, s.int, s.dex, s.luk].iter().any(|v| *v < 99 && stat_cost(*v) <= obs.me.status_points)
}

/// Points to raise a stat from `value` (pre-renewal, rathena's `PC_STATUS_POINT_COST`): 2 up to 10, 3 up to 20, ... 11 at 98.
pub fn stat_cost(value: u32) -> u32 {
    1 + (value + 9) / 10
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_costs_follow_pre_renewal_table() {
        assert_eq!(stat_cost(1), 2);
        assert_eq!(stat_cost(10), 2);
        assert_eq!(stat_cost(11), 3);
        assert_eq!(stat_cost(98), 11);
    }

    #[test]
    fn job_change_readiness_follows_the_job_master() {
        let mut obs = Observation::default();
        obs.me.job = "Novice".into();
        obs.me.job_level = 9;
        assert!(!WorldState::from_observation(&obs, &Extra::default()).job_change_ready);
        obs.me.job_level = 10;
        assert!(WorldState::from_observation(&obs, &Extra::default()).job_change_ready);
        obs.me.job = "Swordman".into();
        assert!(!WorldState::from_observation(&obs, &Extra::default()).job_change_ready);
        obs.me.job_level = 40;
        assert!(WorldState::from_observation(&obs, &Extra::default()).job_change_ready);
        obs.me.job = "Knight".into();
        assert!(!WorldState::from_observation(&obs, &Extra::default()).job_change_ready);
    }

    #[test]
    fn loot_excludes_potions_and_equipped() {
        assert!(is_loot("Etc", false));
        assert!(!is_loot("Healing", false));
        assert!(!is_loot("Weapon", true));
    }
}
