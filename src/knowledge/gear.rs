//! Equipment: what the bot's job can wear, what the weapon and armor dealers sell, and which piece is worth buying or
//! wearing. A piece is valued by its ATK (weapons) or DEF (the rest), one DEF counting as `DEF_WEIGHT` ATK.

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;

use super::db::{Db, ItemData, ShopPlace};
use crate::api::Observation;
use crate::api::observation::{Equip, InventoryItem};

/// Location bits of the item database.
pub const LOC_HEAD_LOW: u32 = 1;
pub const LOC_WEAPON: u32 = 2;
pub const LOC_GARMENT: u32 = 4;
pub const LOC_ARMOR: u32 = 16;
pub const LOC_SHIELD: u32 = 32;
pub const LOC_SHOES: u32 = 64;
pub const LOC_HEAD_TOP: u32 = 256;
pub const LOC_HEAD_MID: u32 = 512;
pub const LOC_TWO_HANDED: u32 = LOC_WEAPON | LOC_SHIELD;
/// One point of DEF takes about a percent off every hit; worth this much ATK.
const DEF_WEIGHT: f32 = 3.0;
/// Shops searched for equipment, by the name of their NPCs.
const SHOP_NAMES: [&str; 2] = ["Weapon Dealer", "Armor Dealer"];
/// A weapon is only replaced for this much more ATK.
const MIN_WEAPON_GAIN: f32 = 8.0;

/// Where a piece goes. Accessories are left out: their worth is in bonuses the bot does not weigh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Slot {
    Weapon,
    Shield,
    Armor,
    Garment,
    Shoes,
    HeadTop,
    HeadMid,
    HeadLow,
}

impl Slot {
    /// The slot of an item database location, `None` for accessories, ammunition and pieces that take two head slots.
    pub fn of_location(location: u32) -> Option<Slot> {
        Some(match location {
            LOC_WEAPON | LOC_TWO_HANDED => Slot::Weapon,
            LOC_SHIELD => Slot::Shield,
            LOC_ARMOR => Slot::Armor,
            LOC_GARMENT => Slot::Garment,
            LOC_SHOES => Slot::Shoes,
            LOC_HEAD_TOP => Slot::HeadTop,
            LOC_HEAD_MID => Slot::HeadMid,
            LOC_HEAD_LOW => Slot::HeadLow,
            _ => return None,
        })
    }

    /// The slot of what the observation lists for an inventory item.
    pub fn of_equip(equip: &Equip) -> Option<Slot> {
        let has = |name: &str| equip.slots.iter().any(|s| s == name);
        Some(match equip.slots.len() {
            1 => match equip.slots[0].as_str() {
                "HandRight" => Slot::Weapon,
                "HandLeft" => Slot::Shield,
                "Armor" => Slot::Armor,
                "Garment" => Slot::Garment,
                "Shoes" => Slot::Shoes,
                "HeadTop" => Slot::HeadTop,
                "HeadMid" => Slot::HeadMid,
                "HeadLow" => Slot::HeadLow,
                _ => return None,
            },
            2 if has("HandRight") && has("HandLeft") => Slot::Weapon,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Slot::Weapon => "weapon",
            Slot::Shield => "shield",
            Slot::Armor => "armor",
            Slot::Garment => "garment",
            Slot::Shoes => "shoes",
            Slot::HeadTop => "headgear",
            Slot::HeadMid => "mid headgear",
            Slot::HeadLow => "lower headgear",
        }
    }
}

/// The bits of `job_flags` a job wears (rust-ro's `EquipClassFlag`): a second job also wears its first job's things.
pub fn job_flags(job: &str) -> u64 {
    const ORDER: [&str; 26] = [
        "novice",
        "acolyte",
        "priest",
        "monk",
        "mage",
        "wizard",
        "sage",
        "thief",
        "assassin",
        "rogue",
        "archer",
        "hunter",
        "bard",
        "dancer",
        "swordman",
        "crusader",
        "knight",
        "merchant",
        "alchemist",
        "blacksmith",
        "gunslinger",
        "ninja",
        "soul linker",
        "star gladiator",
        "super novice",
        "taekwon",
    ];
    let bit = |name: &str| {
        ORDER
            .iter()
            .position(|j| *j == name)
            .map_or(0, |i| 1u64 << i)
    };
    let job = normal_job(job);
    let first = match job.as_str() {
        "priest" | "monk" => "acolyte",
        "wizard" | "sage" => "mage",
        "assassin" | "rogue" => "thief",
        "hunter" | "bard" | "dancer" => "archer",
        "crusader" | "knight" => "swordman",
        "alchemist" | "blacksmith" => "merchant",
        _ => "",
    };
    bit(&job) | bit(first)
}

/// The job's name as `job_flags` knows it: lower case, the game's other spellings folded.
pub fn normal_job(job: &str) -> String {
    match job.to_lowercase().as_str() {
        "swordsman" => "swordman".into(),
        "magician" => "mage".into(),
        "soullinker" => "soul linker".into(),
        "stargladiator" => "star gladiator".into(),
        "supernovice" => "super novice".into(),
        other => other.into(),
    }
}

/// Whether the job fights with a bow, and so needs arrows.
pub fn uses_bow(job: &str) -> bool {
    matches!(
        normal_job(job).as_str(),
        "archer" | "hunter" | "bard" | "dancer"
    )
}

/// Whether ATK is what the job's weapon is for. Casters keep the staff they have: their spells go by INT.
fn fights_with_weapon(job: &str) -> bool {
    !matches!(normal_job(job).as_str(), "mage" | "wizard" | "sage")
}

/// Weapon types worth carrying for a job, of those it can wear: archers stay on bows so their skills work.
fn weapon_type_ok(job: &str, weapon_type: &str) -> bool {
    if uses_bow(job) {
        return weapon_type == "Bow";
    }
    // Instruments, whips and books go with skills the bot does not use; a bow needs arrows it does not buy.
    !matches!(weapon_type, "Bow" | "Musical" | "Whip" | "Book" | "Staff")
}

/// Whether the bot can wear an item of the database now.
pub fn wearable(item: &ItemData, obs: &Observation) -> bool {
    let job = &obs.me.job;
    if item.job_flags & job_flags(job) == 0 || item.equip_level_min > obs.me.base_level {
        return false;
    }
    if let Some(gender) = item
        .gender
        .as_deref()
        .filter(|g| !g.eq_ignore_ascii_case("both"))
    {
        if !obs.me.sex.is_empty() && !gender.eq_ignore_ascii_case(&obs.me.sex) {
            return false;
        }
    }
    item.item_type != "Weapon" || weapon_type_ok(job, &item.weapon_type)
}

/// Whether `job` can wear an item too: a Novice buys nothing its next job takes off again.
fn wearable_by(item: &ItemData, job: &str) -> bool {
    item.job_flags & job_flags(job) != 0
        && (item.item_type != "Weapon" || weapon_type_ok(job, &item.weapon_type))
}

/// What a piece is worth to the job in its slot.
fn worth(job: &str, slot: Slot, attack: i32, defense: i32) -> f32 {
    match slot {
        Slot::Weapon if fights_with_weapon(job) => attack.max(0) as f32,
        Slot::Weapon => 0.0,
        _ => defense.max(0) as f32 * DEF_WEIGHT,
    }
}

/// What the bot wears, per slot: the item id, its worth and, for weapons, whether it takes both hands.
#[derive(Debug, Clone, Default)]
pub struct Worn {
    pub slots: BTreeMap<Slot, (u32, f32)>,
    pub two_handed: bool,
}

impl Worn {
    pub fn of(obs: &Observation) -> Self {
        let mut worn = Worn::default();
        for item in obs.inventory.iter().filter(|i| i.worn_and_usable()) {
            let Some(equip) = &item.equip else { continue };
            let Some(slot) = Slot::of_equip(equip) else {
                continue;
            };
            let value = worth(
                &obs.me.job,
                slot,
                equip.attack.unwrap_or(0) + equip.refine * 2,
                equip.defense.unwrap_or(0) + equip.refine,
            );
            worn.slots.insert(slot, (item.item_id, value));
            if slot == Slot::Weapon && equip.slots.len() == 2 {
                worn.two_handed = true;
            }
        }
        worn
    }

    fn value(&self, slot: Slot) -> f32 {
        self.slots.get(&slot).map_or(0.0, |(_, v)| *v)
    }

    /// How much better the bot fights wearing a piece of `value` in `slot`. A two-handed weapon also takes the
    /// shield off; with one in hand, a shield can not be worn.
    pub fn gain(&self, slot: Slot, two_handed: bool, value: f32) -> f32 {
        match slot {
            Slot::Weapon if two_handed => {
                value - self.value(Slot::Weapon) - self.value(Slot::Shield)
            }
            Slot::Shield if self.two_handed => 0.0,
            _ => value - self.value(slot),
        }
    }
}

/// An inventory piece better than what the bot wears in its slot.
pub fn better_in_bag(obs: &Observation) -> Option<&InventoryItem> {
    let worn = Worn::of(obs);
    obs.inventory
        .iter()
        .filter(|i| !i.equipped)
        .filter_map(|i| {
            let equip = i.equip.as_ref().filter(|e| e.can_equip && e.identified)?;
            let slot = Slot::of_equip(equip)?;
            if slot == Slot::Weapon && !weapon_kind_kept(&obs.me.job, i, obs) {
                return None;
            }
            let value = worth(
                &obs.me.job,
                slot,
                equip.attack.unwrap_or(0) + equip.refine * 2,
                equip.defense.unwrap_or(0) + equip.refine,
            );
            let gain = worn.gain(slot, equip.slots.len() == 2, value);
            (gain > 0.5).then_some((gain, i))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, i)| i)
}

/// A bow user keeps a bow: a dropped sword would leave its arrows useless. The observation does not give weapon types,
/// so a bow is told by taking both hands and the item's name.
fn weapon_kind_kept(job: &str, item: &InventoryItem, _obs: &Observation) -> bool {
    if !uses_bow(job) {
        return !item.name.to_lowercase().contains("bow");
    }
    let name = item.name.to_lowercase();
    name.contains("bow") || name.contains("arbalest") || name.contains("gakkung")
}

/// A piece the shops sell that would make the bot better.
#[derive(Debug, Clone)]
pub struct Upgrade {
    pub item_id: u32,
    pub name: String,
    pub slot: Slot,
    pub price: u64,
    pub gain: f32,
    /// The shop to buy it at, and how many warps away it is.
    pub shop: ShopPlace,
    pub hops: u32,
    /// What the piece and the one it replaces are worth, for the strategist.
    pub value: f32,
    pub replaces: f32,
}

/// The equipment the weapon and armor dealers sell, with its database entry.
pub async fn shop_catalog(db: &Db) -> Result<Vec<ItemData>> {
    let mut ids: Vec<u32> = Vec::new();
    for name in SHOP_NAMES {
        for link in db.search("npcs", name, 200).await? {
            let Ok(npc) = db.npc(&link.id_str()).await else {
                continue;
            };
            for offer in npc.shop.iter().flatten() {
                if let Some(id) = offer.item.id_u32()
                    && !ids.contains(&id)
                {
                    ids.push(id);
                }
            }
        }
    }
    let mut items = Vec::new();
    for id in ids {
        if let Ok(item) = db.item(id).await
            && matches!(item.item_type.as_str(), "Weapon" | "Armor")
        {
            items.push(item);
        }
    }
    Ok(items)
}

/// What the bot shops for gear with.
#[derive(Debug, Clone, Copy, Default)]
pub struct Shopping<'a> {
    /// The zeny one piece may cost.
    pub budget: u64,
    /// Most warps to the shop.
    pub max_hops: u32,
    /// The job the bot heads for: only what it can wear as well.
    pub next_job: Option<&'a str>,
    /// Shops left out, by their `key`.
    pub bad_shops: &'a [String],
}

/// The best piece the bot can buy with `shopping` at a shop it knows a way to, `hops` giving the warps to each map.
pub fn best_upgrade(
    catalog: &[ItemData],
    obs: &Observation,
    hops: &HashMap<String, u32>,
    shopping: &Shopping<'_>,
) -> Option<Upgrade> {
    let Shopping {
        budget,
        next_job,
        bad_shops,
        ..
    } = *shopping;
    let worn = Worn::of(obs);
    let mut best: Option<Upgrade> = None;
    for item in catalog
        .iter()
        .filter(|i| wearable(i, obs) && next_job.is_none_or(|j| wearable_by(i, j)))
    {
        let Some(slot) = Slot::of_location(item.location) else {
            continue;
        };
        let two_handed = item.location == LOC_TWO_HANDED;
        let value = worth(&obs.me.job, slot, item.attack as i32, item.defense as i32);
        let gain = worn.gain(slot, two_handed, value);
        let needed = if slot == Slot::Weapon {
            MIN_WEAPON_GAIN.max(worn.value(Slot::Weapon) * 0.15)
        } else {
            DEF_WEIGHT
        };
        if gain < needed {
            continue;
        }
        let Some((shop, hops)) = item
            .sold_by
            .iter()
            .filter(|s| s.price <= budget && !bad_shops.contains(&s.key()))
            .filter_map(|s| hops.get(&s.map.id_str()).map(|h| (s, *h)))
            .min_by_key(|(s, h)| (*h, s.price))
        else {
            continue;
        };
        let better = match &best {
            None => true,
            // More gain first; for the same gain, the cheaper one (the same armor with a card slot costs more).
            Some(b) => {
                gain > b.gain + 0.1 || ((gain - b.gain).abs() <= 0.1 && shop.price < b.price)
            }
        };
        if better {
            best = Some(Upgrade {
                item_id: item.id,
                name: item.name_english.clone(),
                slot,
                price: shop.price,
                gain,
                shop: shop.clone(),
                hops,
                value,
                replaces: if two_handed {
                    worn.value(Slot::Weapon) + worn.value(Slot::Shield)
                } else {
                    worn.value(slot)
                },
            });
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::observation::InventoryItem;

    fn equipped(
        item_id: u32,
        slots: &[&str],
        attack: Option<i32>,
        defense: Option<i32>,
    ) -> InventoryItem {
        InventoryItem {
            item_id,
            equipped: true,
            equip: Some(Equip {
                slots: slots.iter().map(|s| s.to_string()).collect(),
                attack,
                defense,
                can_equip: true,
                identified: true,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn sold(
        id: u32,
        name: &str,
        location: u32,
        weapon_type: &str,
        attack: u32,
        defense: u32,
        price: u64,
        map: &str,
    ) -> ItemData {
        let place = ShopPlace {
            map: super::super::db::Link {
                id: map.into(),
                ..Default::default()
            },
            price,
            ..Default::default()
        };
        ItemData {
            id,
            name_english: name.into(),
            item_type: if weapon_type.is_empty() {
                "Armor".into()
            } else {
                "Weapon".into()
            },
            weapon_type: weapon_type.into(),
            location,
            attack,
            defense,
            job_flags: job_flags("Swordman") | job_flags("Novice"),
            sold_by: vec![place],
            ..Default::default()
        }
    }

    fn swordman() -> Observation {
        let mut obs = Observation::default();
        obs.me.job = "Swordman".into();
        obs.me.base_level = 20;
        obs.inventory = vec![
            equipped(1201, &["HandRight"], Some(17), None),
            equipped(2301, &["Armor"], None, Some(1)),
        ];
        obs
    }

    #[test]
    fn second_jobs_wear_what_their_first_job_wears() {
        assert_eq!(
            job_flags("Knight"),
            job_flags("Knight") | job_flags("Swordsman")
        );
        assert_ne!(job_flags("Knight") & job_flags("Swordman"), 0);
        assert_eq!(job_flags("Mage"), job_flags("Magician"));
        assert_eq!(job_flags("Novice"), 1);
    }

    #[test]
    fn the_biggest_affordable_gain_at_a_known_shop_wins() {
        let catalog = vec![
            sold(1101, "Sword", LOC_WEAPON, "1hSword", 25, 0, 100, "prt_in"),
            sold(1107, "Blade", LOC_WEAPON, "1hSword", 53, 0, 2900, "prt_in"),
            sold(
                1157,
                "Two-Handed Sword",
                LOC_TWO_HANDED,
                "2hSword",
                160,
                0,
                60000,
                "prt_in",
            ),
            sold(
                2305,
                "Adventurer's Suit",
                LOC_ARMOR,
                "",
                0,
                3,
                1000,
                "far_in",
            ),
        ];
        let hops = HashMap::from([("prt_in".to_string(), 2)]);
        let shopping = Shopping {
            budget: 5000,
            ..Default::default()
        };
        let up = best_upgrade(&catalog, &swordman(), &hops, &shopping).unwrap();
        assert_eq!(up.name, "Blade");
        assert_eq!(up.gain, 36.0);
        // The suit is sold only where the bot knows no way to.
        let up = best_upgrade(&catalog[3..], &swordman(), &hops, &shopping);
        assert!(up.is_none());
    }

    #[test]
    fn a_novice_buys_only_what_its_next_job_wears() {
        let mut novice = swordman();
        novice.me.job = "Novice".into();
        let mut axe = sold(1301, "Axe", LOC_WEAPON, "1hAxe", 50, 0, 500, "prt_in");
        axe.job_flags = job_flags("Novice") | job_flags("Swordman") | job_flags("Merchant");
        let mut mace = sold(1501, "Club", LOC_WEAPON, "Mace", 40, 0, 60, "prt_in");
        mace.job_flags = job_flags("Novice") | job_flags("Acolyte") | job_flags("Swordman");
        let catalog = vec![axe, mace];
        let hops = HashMap::from([("prt_in".to_string(), 1)]);
        let pick = |next_job| {
            let shopping = Shopping {
                budget: 5000,
                next_job,
                ..Default::default()
            };
            best_upgrade(&catalog, &novice, &hops, &shopping).map(|u| u.name)
        };
        assert_eq!(pick(None).as_deref(), Some("Axe"));
        assert_eq!(pick(Some("Swordman")).as_deref(), Some("Axe"));
        assert_eq!(pick(Some("Acolyte")).as_deref(), Some("Club"));
        assert_eq!(pick(Some("Mage")), None);
    }

    #[test]
    fn a_two_handed_weapon_pays_for_the_shield_it_takes_off() {
        let mut obs = swordman();
        obs.inventory
            .push(equipped(2105, &["HandLeft"], None, Some(6)));
        let worn = Worn::of(&obs);
        assert_eq!(worn.gain(Slot::Weapon, true, 30.0), 30.0 - 17.0 - 18.0);
        assert_eq!(worn.gain(Slot::Weapon, false, 30.0), 13.0);
    }

    #[test]
    fn archers_stay_on_bows_and_casters_keep_their_staff() {
        let mut obs = swordman();
        obs.me.job = "Archer".into();
        let mut sword = sold(1107, "Blade", LOC_WEAPON, "1hSword", 53, 0, 2900, "prt_in");
        sword.job_flags = job_flags("Archer");
        assert!(!wearable(&sword, &obs));
        obs.me.job = "Mage".into();
        sword.job_flags = job_flags("Mage");
        assert_eq!(worth("Mage", Slot::Weapon, 53, 0), 0.0);
    }

    #[test]
    fn a_better_drop_is_worn() {
        let mut obs = swordman();
        let mut drop = equipped(2305, &["Armor"], None, Some(3));
        drop.equipped = false;
        drop.index = 7;
        obs.inventory.push(drop);
        assert_eq!(better_in_bag(&obs).map(|i| i.index), Some(7));
        obs.inventory
            .last_mut()
            .unwrap()
            .equip
            .as_mut()
            .unwrap()
            .can_equip = false;
        assert!(better_in_bag(&obs).is_none());
    }
}
