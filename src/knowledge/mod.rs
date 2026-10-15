//! What the bot knows about the game besides its observation: the server's `/db` reference, the warp graph,
//! a fight model and hunting-ground scoring.

pub mod combat;
pub mod db;
pub mod gear;
pub mod graph;
pub mod grounds;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::api::{Action, Observation};
use combat::Fighter;
use db::Db;
use graph::WorldGraph;

/// Shops dropped for want of a safe way before the bot gives up on buying gear for now.
const SAFE_SHOP_TRIES: usize = 5;

/// Shared access to the reference data.
#[derive(Clone)]
pub struct Knowledge {
    pub db: Db,
    pub graph: WorldGraph,
    equip_stats: Arc<Mutex<HashMap<u32, (u32, u32)>>>,
    /// Average HP each item heals, `None` for items that do not heal HP.
    heals: Arc<Mutex<HashMap<u32, Option<f32>>>>,
    /// The equipment the weapon and armor dealers sell, read once.
    gear_catalog: Arc<Mutex<Option<Arc<Vec<db::ItemData>>>>>,
}

impl Knowledge {
    pub fn new(db: Db) -> Self {
        Self {
            graph: WorldGraph::new(db.clone()),
            db,
            equip_stats: Default::default(),
            heals: Default::default(),
            gear_catalog: Default::default(),
        }
    }

    /// The best piece of equipment the bot can buy for `budget` zeny, at a shop at most `max_hops` warps away on a way
    /// that crosses no deadly map (`grounds::safe_hops`), that `next_job` can wear too. Counted by plain warps, shops in
    /// Einbroch, Lighthalzen and Veins sent bots from Rachel through Drosera and Hill Wind: 9 deaths in 40 minutes.
    pub async fn best_upgrade(
        &self,
        obs: &Observation,
        budget: u64,
        max_hops: u32,
        next_job: Option<&str>,
        bad_shops: &[String],
    ) -> Result<Option<gear::Upgrade>> {
        let cached = self.gear_catalog.lock().unwrap().clone();
        let catalog = match cached {
            Some(c) => c,
            None => {
                let c = Arc::new(gear::shop_catalog(&self.db).await?);
                *self.gear_catalog.lock().unwrap() = Some(c.clone());
                c
            }
        };
        let mut hops: HashMap<String, u32> = self
            .graph
            .neighbourhood(&obs.map.name, max_hops)
            .await?
            .into_iter()
            .collect();
        let fighter = self.fighter(obs).await?;
        let mut lethal = HashMap::new();
        // The best upgrade at a shop the bot can reach safely: shops on maps without such a way are left out, one at a time
        for _ in 0..SAFE_SHOP_TRIES {
            let Some(up) = gear::best_upgrade(&catalog, obs, budget, &hops, next_job, bad_shops)
            else {
                return Ok(None);
            };
            let map = up.shop.map.id_str();
            let safe = grounds::safe_hops(
                &self.db,
                &self.graph,
                &fighter,
                &obs.map.name,
                &map,
                &Default::default(),
                &mut lethal,
            )
            .await?;
            match safe {
                Some(h) if h <= max_hops => return Ok(Some(gear::Upgrade { hops: h, ..up })),
                _ => {
                    hops.remove(&map);
                }
            }
        }
        Ok(None)
    }

    /// Looks up the heal amount of the usable items in the inventory it has not seen yet.
    pub async fn learn_heals(&self, obs: &Observation) {
        for item in obs
            .inventory
            .iter()
            .filter(|i| i.kind.eq_ignore_ascii_case("healing"))
        {
            if self.heals.lock().unwrap().contains_key(&item.item_id) {
                continue;
            }
            let heal = self
                .db
                .item(item.item_id)
                .await
                .ok()
                .and_then(|d| d.hp_heal());
            self.heals.lock().unwrap().insert(item.item_id, heal);
        }
    }

    /// Average HP one of this item heals, once [`Self::learn_heals`] has seen it.
    pub fn heal_of(&self, item_id: u32) -> Option<f32> {
        self.heals.lock().unwrap().get(&item_id).copied().flatten()
    }

    /// HP all the healing items of the inventory would give back.
    pub fn heal_stock(&self, obs: &Observation) -> u32 {
        let rate = item_heal_rate(obs);
        obs.inventory
            .iter()
            .filter_map(|i| self.heal_of(i.item_id).map(|h| h * rate * i.amount as f32))
            .sum::<f32>() as u32
    }

    /// Number of healing items in the inventory.
    pub fn heal_items(&self, obs: &Observation) -> u32 {
        obs.inventory
            .iter()
            .filter(|i| self.heal_of(i.item_id).is_some())
            .map(|i| i.amount)
            .sum()
    }

    /// The heal to use now: the smallest item or skill that heals the missing HP completely, else the biggest.
    pub fn heal_action(&self, obs: &Observation) -> Option<Heal> {
        let missing = obs.me.max_hp.saturating_sub(obs.me.hp) as f32;
        if missing <= 0.0 {
            return None;
        }
        let rate = item_heal_rate(obs);
        let mut options: Vec<Heal> = heal_skills(obs);
        options.extend(
            obs.inventory
                .iter()
                .filter(|i| i.amount > 0)
                .filter_map(|i| {
                    self.heal_of(i.item_id).map(|hp| Heal {
                        hp: hp * rate,
                        action: Action::UseItem { index: i.index },
                        skill: false,
                    })
                }),
        );
        pick_heal(options, missing)
    }

    /// The bot as the fight model sees it, with weapon attack and armor defense of what it wears.
    pub async fn fighter(&self, obs: &Observation) -> Result<Fighter> {
        let mut weapon_atk = 0;
        let mut armor_def = 0;
        for item in obs.inventory.iter().filter(|i| i.worn_and_usable()) {
            let cached = self.equip_stats.lock().unwrap().get(&item.item_id).copied();
            let (atk, def) = match cached {
                Some(v) => v,
                None => {
                    let v = self
                        .db
                        .item(item.item_id)
                        .await
                        .map(|d| (d.attack, d.defense))
                        .unwrap_or((0, 0));
                    self.equip_stats.lock().unwrap().insert(item.item_id, v);
                    v
                }
            };
            weapon_atk += atk;
            armor_def += def;
        }
        let s = &obs.me.stats;
        Ok(Fighter {
            level: obs.me.base_level,
            max_hp: obs.me.max_hp,
            str: s.str,
            agi: s.agi,
            vit: s.vit,
            dex: s.dex,
            luk: s.luk,
            weapon_atk,
            armor_def,
        })
    }
}

/// One way to get HP back.
#[derive(Debug, Clone)]
pub struct Heal {
    /// Average HP it gives back.
    pub hp: f32,
    pub action: Action,
    /// A skill, paid with SP rather than an item.
    pub skill: bool,
}

/// How much more than its base amount a healing item gives back (rathena `pc_itemheal`): 2% per VIT, 10% per
/// level of Increase HP Recovery, 5% per level of Potion Research.
pub fn item_heal_rate(obs: &Observation) -> f32 {
    let level = |name: &str| {
        obs.skills
            .iter()
            .find(|s| s.name == name)
            .map_or(0, |s| s.level)
    };
    (100 + 2 * obs.me.stats.vit + 10 * level("SM_RECOVERY") + 5 * level("AM_LEARNINGPOTION")) as f32
        / 100.0
}

/// The smallest heal that covers `missing` HP, else the biggest. Skills come first in `options`, so they win a tie.
fn pick_heal(options: Vec<Heal>, missing: f32) -> Option<Heal> {
    let complete = options
        .iter()
        .filter(|h| h.hp >= missing)
        .min_by(|a, b| a.hp.total_cmp(&b.hp));
    complete
        .or_else(|| options.iter().max_by(|a, b| a.hp.total_cmp(&b.hp)))
        .cloned()
}

/// The healing skills the bot can cast on itself now, with pre-renewal heal amounts and SP costs.
fn heal_skills(obs: &Observation) -> Vec<Heal> {
    let me = &obs.me;
    let mut out = Vec::new();
    for skill in obs.skills.iter().filter(|s| s.level > 0) {
        let (hp, sp) = match skill.name.as_str() {
            // Heal: (base level + INT) / 8 * (4 + 8 * level), 10 + 3 * level SP.
            "AL_HEAL" => (
                ((me.base_level + me.stats.int) / 8 * (4 + 8 * skill.level)) as f32,
                10 + 3 * skill.level,
            ),
            // First Aid: 5 HP for 3 SP.
            "NV_FIRSTAID" => (5.0, 3),
            _ => continue,
        };
        if hp > 0.0 && me.sp >= sp {
            let target = if skill.aim == "self" {
                None
            } else {
                Some(me.char_id)
            };
            out.push(Heal {
                hp,
                action: Action::Skill {
                    skill: skill.name.clone(),
                    target,
                },
                skill: true,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::observation::Skill;

    #[test]
    fn item_heals_grow_with_vit_and_skills() {
        let mut obs = Observation::default();
        obs.me.stats.vit = 10;
        obs.skills = vec![Skill {
            name: "SM_RECOVERY".into(),
            level: 2,
            ..Default::default()
        }];
        assert!((item_heal_rate(&obs) - 1.4).abs() < 1e-6);
    }

    #[test]
    fn the_smallest_complete_heal_wins() {
        let item = |index, hp| Heal {
            hp,
            action: Action::UseItem { index },
            skill: false,
        };
        let pick =
            |missing| match pick_heal(vec![item(0, 18.0), item(1, 55.0), item(2, 85.0)], missing)
                .unwrap()
                .action
            {
                Action::UseItem { index } => index,
                _ => unreachable!(),
            };
        assert_eq!(pick(10.0), 0);
        assert_eq!(pick(40.0), 1);
        assert_eq!(pick(60.0), 2);
        assert_eq!(pick(200.0), 2);
    }

    #[test]
    fn heal_skills_follow_the_formula() {
        let mut obs = Observation::default();
        obs.me.base_level = 20;
        obs.me.stats.int = 20;
        obs.me.sp = 30;
        obs.me.char_id = 7;
        obs.skills = vec![Skill {
            name: "AL_HEAL".into(),
            level: 2,
            aim: "target".into(),
            ..Default::default()
        }];
        let heals = heal_skills(&obs);
        assert_eq!(heals.len(), 1);
        assert_eq!(heals[0].hp, 100.0);
        assert!(matches!(
            &heals[0].action,
            Action::Skill {
                target: Some(7),
                ..
            }
        ));
        obs.me.sp = 10;
        assert!(heal_skills(&obs).is_empty());
    }
}
