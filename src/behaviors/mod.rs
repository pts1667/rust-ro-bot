//! The behaviours that carry out GOAP steps through the bot API.

pub mod combat;
pub mod commerce;
pub mod dialog;
pub mod progression;
pub mod travel;

use anyhow::Result;

use crate::bot::{Bot, Outcome};
use crate::goap::Step;

impl Bot {
    pub async fn execute(&mut self, step: &Step) -> Result<Outcome> {
        if !matches!(step, Step::WalkToShop | Step::SellLoot | Step::BuyPotions) {
            self.near_shop = false;
        }
        match step {
            Step::ExitTraining => self.exit_training().await,
            Step::Travel { map } => self.travel(map).await,
            Step::Rest => self.rest().await,
            Step::Hunt { .. } => self.hunt_once().await,
            Step::WalkToShop => self.walk_to_shop().await,
            Step::SellLoot => self.sell_loot().await,
            Step::BuyPotions => self.buy_potions().await,
            Step::RaiseStats => self.raise_stats().await,
            Step::LearnSkills => self.learn_skills().await,
            Step::ChangeJob { job } => self.change_job(job).await,
        }
    }
}
