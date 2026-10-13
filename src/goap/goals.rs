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
    /// Buy a piece of equipment at the shop at `x`, `y` on `map` and wear it.
    BuyGear { item: u32, name: String, map: String, x: u16, y: u16 },
    /// Have `amount` of an item, bought at the shop at `x`, `y` on `map` (for a quest).
    BuyItem { item: u32, amount: u32, name: String, map: String, x: u16, y: u16 },
    /// Do the stage `stage` of a quest, on `map` (at `x`, `y` when it is an NPC).
    Quest { id: String, stage: usize, what: String, map: String, x: u16, y: u16 },
    /// Do what `who` asked in chat (see `social`). Not planned: the agent runs it until it is over.
    Help { who: String, task: HelpTask },
}

/// What someone can ask a bot to do for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelpTask {
    /// Walk along with them for a while, fighting what attacks either.
    Follow,
    /// Invite them to the bot's party, creating one if needed.
    Invite,
    /// Take their invitation to their party.
    JoinParty,
    /// Hand them some healing items.
    GivePotions,
}

impl HelpTask {
    pub fn label(self) -> &'static str {
        match self {
            HelpTask::Follow => "follow",
            HelpTask::Invite => "invite",
            HelpTask::JoinParty => "join the party of",
            HelpTask::GivePotions => "give potions to",
        }
    }
}

impl Goal {
    pub fn satisfied(&self, ws: &WorldState) -> bool {
        match self {
            Goal::Hunt { .. } => ws.hunted,
            Goal::Rest => ws.hp_full,
            Goal::SellLoot => !ws.loot_to_sell,
            Goal::BuyPotions => ws.has_potions && !ws.ammo_low,
            Goal::SpendPoints => !ws.status_points && !ws.skill_points,
            // The server may spell it differently ("Swordsman", "Magician"): the first three letters are enough.
            Goal::JobChange { job } => {
                ws.job.eq_ignore_ascii_case(job)
                    || (ws.job.len() >= 3 && job.len() >= 3 && ws.job[..3].eq_ignore_ascii_case(&job[..3]))
            }
            Goal::Explore { map } => &ws.map == map,
            Goal::LeaveTraining => !ws.in_training,
            Goal::BuyGear { item, .. } => ws.items.contains_key(item),
            Goal::BuyItem { item, amount, .. } => ws.items.get(item).is_some_and(|n| n >= amount),
            Goal::Quest { id, stage, .. } => ws.quest_stages.get(id).copied().unwrap_or(0) != *stage,
            Goal::Help { .. } => false,
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
            Goal::BuyGear { name, .. } => format!("buy {name}"),
            Goal::BuyItem { name, amount, .. } => format!("buy {amount} {name}"),
            Goal::Quest { id, what, .. } => format!("quest {id}: {what}"),
            Goal::Help { who, task } => format!("{} {who}", task.label()),
        }
    }

    /// Map this goal needs to stand on, if any.
    pub fn map(&self) -> Option<&str> {
        match self {
            Goal::Hunt { map } | Goal::Explore { map } | Goal::BuyGear { map, .. } | Goal::BuyItem { map, .. } | Goal::Quest { map, .. } => {
                Some(map)
            }
            _ => None,
        }
    }

    /// The spot on its map the goal is about, so a trip into a map of several rooms takes the right door.
    pub fn spot(&self) -> Option<(String, (u16, u16))> {
        match self {
            Goal::BuyGear { map, x, y, .. } | Goal::BuyItem { map, x, y, .. } => Some((map.clone(), (*x, *y))),
            Goal::Quest { map, x, y, .. } if (*x, *y) != (0, 0) => Some((map.clone(), (*x, *y))),
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
