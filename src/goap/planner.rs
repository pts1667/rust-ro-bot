//! Cheapest sequence of steps from a world state to a goal (uniform-cost search, the state space is tiny).

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};

use super::actions::{PlanContext, Step};
use super::goals::Goal;
use super::state::WorldState;

const MAX_NODES: usize = 20_000;
const MAX_DEPTH: usize = 10;

/// Steps worth considering for a goal.
pub fn candidate_steps(goal: &Goal, ctx: &PlanContext) -> Vec<Step> {
    let mut steps = vec![
        Step::ExitTraining,
        Step::Rest,
        Step::WalkToShop,
        Step::SellLoot,
        Step::BuyPotions,
        Step::RaiseStats,
        Step::LearnSkills,
    ];
    let mut maps: Vec<&str> = Vec::new();
    maps.extend(goal.map());
    maps.extend(ctx.shop_map.as_deref());
    maps.extend(ctx.job_map.as_deref());
    maps.sort_unstable();
    maps.dedup();
    for map in maps {
        steps.push(Step::Travel {
            map: map.to_owned(),
        });
    }
    if let Goal::Hunt { map } = goal {
        steps.push(Step::Hunt { map: map.clone() });
    }
    if let Goal::JobChange { job } = goal {
        steps.push(Step::ChangeJob { job: job.clone() });
    }
    match goal {
        Goal::BuyGear { item, map, .. } => steps.push(Step::BuyGear {
            item: *item,
            map: map.clone(),
        }),
        Goal::BuyItem {
            item, amount, map, ..
        } => steps.push(Step::BuyItem {
            item: *item,
            amount: *amount,
            map: map.clone(),
        }),
        Goal::Quest { id, map, .. } => steps.push(Step::Quest {
            id: id.clone(),
            map: map.clone(),
        }),
        _ => {}
    }
    steps
}

/// The plan, or `None` when the goal can not be reached from `start` with the known steps.
pub fn plan(start: &WorldState, goal: &Goal, ctx: &PlanContext) -> Option<Vec<Step>> {
    if goal.satisfied(start) {
        return Some(Vec::new());
    }
    let steps = candidate_steps(goal, ctx);
    let mut nodes: Vec<(WorldState, Option<(usize, usize)>)> = vec![(start.clone(), None)];
    let mut open = BinaryHeap::new();
    open.push(Reverse((0u32, 0usize, 0usize)));
    let mut closed: HashSet<WorldState> = HashSet::new();

    while let Some(Reverse((cost, depth, node))) = open.pop() {
        let state = nodes[node].0.clone();
        if goal.satisfied(&state) {
            return Some(unwind(&nodes, node, &steps));
        }
        if !closed.insert(state.clone()) || depth >= MAX_DEPTH || nodes.len() > MAX_NODES {
            continue;
        }
        for (i, step) in steps.iter().enumerate() {
            if !step.applicable(&state, ctx) {
                continue;
            }
            let next = step.apply(&state);
            if closed.contains(&next) {
                continue;
            }
            nodes.push((next, Some((node, i))));
            open.push(Reverse((cost + step.cost(ctx), depth + 1, nodes.len() - 1)));
        }
    }
    None
}

fn unwind(
    nodes: &[(WorldState, Option<(usize, usize)>)],
    mut node: usize,
    steps: &[Step],
) -> Vec<Step> {
    let mut out = Vec::new();
    while let Some((parent, step)) = nodes[node].1 {
        out.push(steps[step].clone());
        node = parent;
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn town() -> WorldState {
        WorldState {
            map: "prontera".into(),
            hp_ok: true,
            hp_full: true,
            job: "Novice".into(),
            ..Default::default()
        }
    }

    #[test]
    fn hunt_elsewhere_travels_first() {
        let goal = Goal::Hunt {
            map: "prt_fild08".into(),
        };
        let plan = plan(&town(), &goal, &PlanContext::default()).unwrap();
        assert_eq!(
            plan,
            vec![
                Step::Travel {
                    map: "prt_fild08".into()
                },
                Step::Hunt {
                    map: "prt_fild08".into()
                }
            ]
        );
    }

    #[test]
    fn low_hp_rests_before_hunting() {
        let mut ws = town();
        ws.map = "prt_fild08".into();
        ws.hp_ok = false;
        ws.hp_full = false;
        let plan = plan(
            &ws,
            &Goal::Hunt {
                map: "prt_fild08".into(),
            },
            &PlanContext::default(),
        )
        .unwrap();
        assert_eq!(
            plan,
            vec![
                Step::Rest,
                Step::Hunt {
                    map: "prt_fild08".into()
                }
            ]
        );
    }

    #[test]
    fn training_grounds_are_left_first() {
        let mut ws = town();
        ws.map = "new_1-1".into();
        ws.in_training = true;
        let plan = plan(
            &ws,
            &Goal::Hunt {
                map: "prt_fild08".into(),
            },
            &PlanContext::default(),
        )
        .unwrap();
        assert_eq!(plan[0], Step::ExitTraining);
        assert_eq!(
            plan.last(),
            Some(&Step::Hunt {
                map: "prt_fild08".into()
            })
        );
    }

    #[test]
    fn buying_potions_sells_loot_when_poor() {
        let mut ws = town();
        ws.loot_to_sell = true;
        let ctx = PlanContext {
            shop_map: Some("prt_fild05".into()),
            ..Default::default()
        };
        let plan = plan(&ws, &Goal::BuyPotions, &ctx).unwrap();
        assert_eq!(
            plan,
            vec![
                Step::Travel {
                    map: "prt_fild05".into()
                },
                Step::WalkToShop,
                Step::SellLoot,
                Step::BuyPotions
            ]
        );
    }

    #[test]
    fn job_change_needs_readiness() {
        let ctx = PlanContext {
            job_map: Some("prontera".into()),
            ..Default::default()
        };
        let goal = Goal::JobChange {
            job: "Swordman".into(),
        };
        assert!(plan(&town(), &goal, &ctx).is_none());
        let mut ws = town();
        ws.job_change_ready = true;
        assert_eq!(
            plan(&ws, &goal, &ctx).unwrap(),
            vec![Step::ChangeJob {
                job: "Swordman".into()
            }]
        );
    }

    #[test]
    fn gear_is_bought_at_its_shop() {
        let goal = Goal::BuyGear {
            item: 1107,
            name: "Blade".into(),
            map: "prt_in".into(),
            x: 1,
            y: 2,
        };
        let plan = plan(&town(), &goal, &PlanContext::default()).unwrap();
        assert_eq!(
            plan,
            vec![
                Step::Travel {
                    map: "prt_in".into()
                },
                Step::BuyGear {
                    item: 1107,
                    map: "prt_in".into()
                }
            ]
        );
    }

    #[test]
    fn arrows_are_supplies() {
        let mut ws = town();
        ws.has_potions = true;
        ws.ammo_low = true;
        ws.can_afford_potions = true;
        let ctx = PlanContext {
            shop_map: Some("prontera".into()),
            ..Default::default()
        };
        assert_eq!(
            plan(&ws, &Goal::BuyPotions, &ctx).unwrap(),
            vec![Step::WalkToShop, Step::BuyPotions]
        );
    }

    #[test]
    fn satisfied_goal_needs_no_steps() {
        assert_eq!(
            plan(&town(), &Goal::Rest, &PlanContext::default()),
            Some(vec![])
        );
    }
}
