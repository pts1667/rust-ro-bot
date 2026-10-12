//! Goal-Oriented Action Planning: the strategist sets a [`Goal`], the planner finds the [`Step`]s that reach it
//! from the current [`WorldState`], and the agent runs the first step and plans again.

pub mod actions;
pub mod goals;
pub mod planner;
pub mod state;

pub use actions::{PlanContext, Step};
pub use goals::Goal;
pub use planner::plan;
pub use state::WorldState;
