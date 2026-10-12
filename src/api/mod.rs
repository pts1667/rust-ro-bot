//! The bot HTTP API of the rust-ro server.

pub mod action;
pub mod client;
pub mod observation;

pub use action::{Action, BuyLine, SellLine};
pub use client::{ApiError, BotApi};
pub use observation::{Observation, Party, PartyMember, Trade, dist};
#[cfg(test)]
pub use observation::{Message, Mob, Player};
