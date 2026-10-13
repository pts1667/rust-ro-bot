//! GOAP actions: preconditions and effects on the [`WorldState`], with a cost. Each one is carried out by a
//! behaviour of `crate::behaviors`.

use super::state::WorldState;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Step {
    /// Talk the Training Grounds receptionist into sending the bot to a town.
    ExitTraining,
    /// Walk the warps to a map.
    Travel { map: String },
    /// Stand still until HP is back.
    Rest,
    /// Fight one monster on the map, then pick up what it dropped.
    Hunt { map: String },
    /// Walk to the shop NPC of the plan.
    WalkToShop,
    SellLoot,
    BuyPotions,
    RaiseStats,
    LearnSkills,
    /// Talk to the Job Master.
    ChangeJob { job: String },
    /// Buy a piece of equipment at a shop of this map and wear it.
    BuyGear { item: u32, map: String },
    /// Buy items at a shop of this map until the bot has `amount`.
    BuyItem { item: u32, amount: u32, map: String },
    /// Do the current stage of a quest, on this map.
    Quest { id: String, map: String },
}

/// Where the plan's shop and Job Master are, and how far maps are, as known when planning.
#[derive(Debug, Clone, Default)]
pub struct PlanContext {
    /// Map of the shop the bot uses to sell and buy.
    pub shop_map: Option<String>,
    /// Map of the Job Master.
    pub job_map: Option<String>,
    /// Warps from the current map to the maps the plan may need, when known.
    pub hops: Vec<(String, u32)>,
}

impl PlanContext {
    fn travel_cost(&self, map: &str) -> u32 {
        self.hops.iter().find(|(m, _)| m == map).map(|(_, h)| 2 + 2 * h).unwrap_or(12)
    }
}

impl Step {
    pub fn cost(&self, ctx: &PlanContext) -> u32 {
        match self {
            Step::ExitTraining => 5,
            Step::Travel { map } => ctx.travel_cost(map),
            Step::Rest => 4,
            Step::Hunt { .. } => 1,
            Step::WalkToShop => 1,
            Step::SellLoot | Step::BuyPotions => 1,
            Step::RaiseStats | Step::LearnSkills => 1,
            Step::ChangeJob { .. } => 2,
            Step::BuyGear { .. } | Step::BuyItem { .. } | Step::Quest { .. } => 1,
        }
    }

    pub fn applicable(&self, ws: &WorldState, ctx: &PlanContext) -> bool {
        match self {
            Step::ExitTraining => ws.in_training,
            Step::Travel { map } => !ws.in_training && &ws.map != map,
            Step::Rest => !ws.hp_full,
            Step::Hunt { map } => &ws.map == map && ws.hp_ok,
            Step::WalkToShop => ctx.shop_map.as_deref() == Some(ws.map.as_str()) && !ws.near_shop,
            Step::SellLoot => ws.near_shop && ws.loot_to_sell,
            Step::BuyPotions => ws.near_shop && ws.can_afford_potions && (!ws.has_potions || ws.ammo_low),
            Step::RaiseStats => ws.status_points,
            Step::LearnSkills => ws.skill_points,
            Step::ChangeJob { .. } => ctx.job_map.as_deref() == Some(ws.map.as_str()) && ws.job_change_ready,
            Step::BuyGear { map, .. } | Step::BuyItem { map, .. } | Step::Quest { map, .. } => &ws.map == map,
        }
    }

    /// The state expected after the step. Uncertain outcomes are assumed to go well; the agent replans from the
    /// real observation after every step anyway.
    pub fn apply(&self, ws: &WorldState) -> WorldState {
        let mut next = ws.clone();
        match self {
            Step::ExitTraining => {
                next.in_training = false;
                next.map = "<town>".into();
                next.near_shop = false;
            }
            Step::Travel { map } => {
                next.map = map.clone();
                next.near_shop = false;
            }
            Step::Rest => {
                next.hp_ok = true;
                next.hp_full = true;
            }
            Step::Hunt { .. } => next.hunted = true,
            Step::WalkToShop => next.near_shop = true,
            Step::SellLoot => {
                next.loot_to_sell = false;
                next.can_afford_potions = true;
            }
            Step::BuyPotions => {
                next.has_potions = true;
                next.ammo_low = false;
            }
            Step::RaiseStats => next.status_points = false,
            Step::LearnSkills => next.skill_points = false,
            Step::ChangeJob { job } => {
                next.job = job.clone();
                next.job_change_ready = false;
            }
            Step::BuyGear { item, .. } => {
                next.items.entry(*item).or_insert(1);
            }
            Step::BuyItem { item, amount, .. } => {
                let have = next.items.entry(*item).or_default();
                *have = (*have).max(*amount);
            }
            Step::Quest { id, .. } => {
                *next.quest_stages.entry(id.clone()).or_default() += 1;
            }
        }
        next
    }
}
