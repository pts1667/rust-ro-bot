//! The goals the strategist can set. Each one is a condition on the [`WorldState`] that GOAP plans towards.

use serde::{Deserialize, Serialize};

use super::state::WorldState;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Goal {
    /// Fight on a map. Never satisfied: it lasts until the strategist picks something else or its budget runs out.
    Hunt { map: String },
    /// Get HP back.
    Rest,
    /// Sell the loot at a shop.
    SellLoot,
    /// Buy healing potions.
    BuyPotions,
    /// Spend status and skill points.
    SpendPoints,
    /// Change job at the Job Master.
    JobChange { job: String },
    /// Walk to a map.
    Explore { map: String },
    /// Leave the training grounds for a town.
    LeaveTraining,
}

impl Goal {
    pub fn satisfied(&self, ws: &WorldState) -> bool {
        match self {
            Goal::Hunt { .. } => ws.hunted,
            Goal::Rest => ws.hp_full,
            Goal::SellLoot => !ws.loot_to_sell,
            Goal::BuyPotions => ws.has_potions,
            Goal::SpendPoints => !ws.status_points && !ws.skill_points,
            // The server may spell it differently ("Swordsman", "Magician"): the first three letters are enough.
            Goal::JobChange { job } => {
                ws.job.eq_ignore_ascii_case(job)
                    || (ws.job.len() >= 3 && job.len() >= 3 && ws.job[..3].eq_ignore_ascii_case(&job[..3]))
            }
            Goal::Explore { map } => &ws.map == map,
            Goal::LeaveTraining => !ws.in_training,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Goal::Hunt { map } => format!("hunt on {map}"),
            Goal::Rest => "rest".into(),
            Goal::SellLoot => "sell loot".into(),
            Goal::BuyPotions => "buy potions".into(),
            Goal::SpendPoints => "spend points".into(),
            Goal::JobChange { job } => format!("change job to {job}"),
            Goal::Explore { map } => format!("explore {map}"),
            Goal::LeaveTraining => "leave the training grounds".into(),
        }
    }

    /// Map this goal needs to stand on, if any.
    pub fn map(&self) -> Option<&str> {
        match self {
            Goal::Hunt { map } | Goal::Explore { map } => Some(map),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_change_accepts_server_spellings() {
        let goal = Goal::JobChange { job: "Swordman".into() };
        let ws = |job: &str| WorldState { job: job.into(), ..Default::default() };
        assert!(goal.satisfied(&ws("Swordsman")));
        assert!(goal.satisfied(&ws("swordman")));
        assert!(!goal.satisfied(&ws("Novice")));
        assert!(Goal::JobChange { job: "Mage".into() }.satisfied(&ws("Magician")));
    }
}
