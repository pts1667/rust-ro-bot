//! The menu of goals offered to the strategist, built by code so the small model picks rather than invents,
//! and the deterministic choice used without the LLM or when it fails.

use std::collections::HashSet;

use anyhow::Result;

use super::Snapshot;
use super::skill_state::StrategicState;
use crate::behaviors::gear::gear_budget;
use crate::behaviors::quest::{awake, stage_place};
use crate::goap::Goal;
use crate::goap::goals::HelpTask;
use crate::goap::state::{HEAL_STOCK_MIN, HEAL_STOCK_WANTED, arrow_count, next_jobs};
use crate::knowledge::Knowledge;
use crate::knowledge::gear::Slot;
use crate::knowledge::grounds::{farm_ground, hunting_grounds, is_hunting_map_name};
use crate::party::recruit::MAX_MEMBERS;
use crate::party::support::SPARE_ABOVE;
use crate::party::{SIGHT, same_job};
use crate::quests::{self, Act, Need, Quest, QuestProgress, Reward, stage_map};
use crate::social::Ask;

/// The map of the Job Master.
pub const JOB_MAP: &str = "prontera";
const HUNT_OPTIONS: usize = 5;
const HUNT_HOPS: u32 = 4;
const DEFAULT_POTION_PRICE: u64 = 50;
/// Warps the bot goes to buy equipment.
const GEAR_HOPS: u32 = 10;
/// Warps the bot goes to buy what a quest asks for.
const QUEST_SHOP_HOPS: u32 = 15;
/// Warps the bot goes to hunt what a quest asks for.
const QUEST_FARM_HOPS: u32 = 6;
/// Score of the quest option of a test the bot is in: nothing else is offered then.
pub const IN_TEST: f32 = 10.0;
const JOB_QUEST: f32 = 2.0;
/// Score of a skill quest the bot has begun that the next job may not take: finished before the job change.
pub const CLOSING_QUEST: f32 = 3.0;
const SKILL_QUEST: f32 = 1.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Opt {
    /// Stable id the model answers with, e.g. `hunt:prt_fild08`.
    pub id: String,
    pub goal: Goal,
    /// One line of description for the prompt.
    pub line: String,
    /// Hunting score, for the fallback.
    pub score: f32,
}

fn opt(id: impl Into<String>, goal: Goal, line: impl Into<String>) -> Opt {
    Opt {
        id: id.into(),
        goal,
        line: line.into(),
        score: 0.0,
    }
}

pub async fn build_options(
    snap: &Snapshot,
    knowledge: &Knowledge,
    state: &StrategicState,
) -> Result<Vec<Opt>> {
    // A favour the bot agreed to is kept until it is over.
    if let Some(goal @ Goal::Help { who, .. }) = &snap.goal {
        return Ok(vec![opt(
            "keep_helping",
            goal.clone(),
            format!("go on helping {who} as you agreed"),
        )]);
    }
    let mut out = game_options(snap, knowledge, state).await?;
    let busy = snap.ws.in_training || (out.len() == 1 && out[0].score >= IN_TEST);
    if !busy {
        let asks = request_options(snap, &out);
        out.extend(asks);
    }
    Ok(out)
}

/// What the requests of `snap` would have the bot do, those it can: `ask:<id>` options. Their score is 1 when the
/// fallback takes them: what costs little, and potions only for a member or friend unless the bot has plenty. A
/// stranger's level is not known (a client shows it for party members only), so it does not count.
pub fn request_options(snap: &Snapshot, others: &[Opt]) -> Vec<Opt> {
    request_verdicts(snap, others)
        .into_iter()
        .filter_map(|(_, v, _)| v.ok())
        .collect()
}

/// Why the bot does not do what was asked, by request id: it cannot, or would rather not.
pub fn request_refusals(snap: &Snapshot) -> Vec<(u64, String)> {
    request_verdicts(snap, &[])
        .into_iter()
        .filter_map(|(id, v, rather_not)| v.err().or(rather_not).map(|why| (id, why)))
        .collect()
}

/// For each request, the option that does it, or why the bot cannot; and why it would rather not, when it may.
fn request_verdicts(
    snap: &Snapshot,
    others: &[Opt],
) -> Vec<(u64, Result<Opt, String>, Option<String>)> {
    let obs = &snap.obs;
    let me = &obs.me.name;
    let mut out = Vec::new();
    for r in &snap.requests {
        let member = obs.party.as_ref().is_some_and(|p| {
            p.members
                .iter()
                .any(|m| m.name.eq_ignore_ascii_case(&r.from))
        });
        let friend = member || snap.friends.iter().any(|f| f.eq_ignore_ascii_case(&r.from));
        // As far as a player sees; the party window shows members anywhere
        let seen = obs.player_near(&r.from, SIGHT).is_some();
        tracing::debug!(target: "strategist", from = %r.from, seen, me = ?obs.pos(), players = ?obs.players.iter().map(|p| (&p.name, p.x, p.y)).collect::<Vec<_>>(), "request in sight?");
        let leads = obs.party.as_ref().is_some_and(|p| {
            p.members
                .iter()
                .any(|m| m.leader && m.name.eq_ignore_ascii_case(me))
        });
        let alone = obs.party.as_ref().is_none_or(|p| p.members.len() == 1);
        let help = |task| Goal::Help {
            who: r.from.clone(),
            task,
        };
        let mut rather_not = None;
        let verdict: Result<(Goal, String, bool), String> = match &r.ask {
            Ask::Help(HelpTask::Follow) => {
                if member || seen {
                    Ok((
                        help(HelpTask::Follow),
                        format!(
                            "walk along with {} for a few minutes, fighting by their side",
                            r.from
                        ),
                        true,
                    ))
                } else {
                    Err("you do not see them anywhere near".into())
                }
            }
            Ask::Help(HelpTask::Invite) => match &obs.party {
                _ if !snap.party_play => Err("you play alone".into()),
                _ if member => Err("they are in your party already".into()),
                None if obs.skill_level("NV_BASIC") < 7 => {
                    Err("you cannot create a party yet (Basic Skill 7 needed)".into())
                }
                Some(_) if !leads => Err("only the leader of your party can invite".into()),
                Some(p) if p.members.len() >= MAX_MEMBERS => Err("your party is full".into()),
                _ => Ok((
                    help(HelpTask::Invite),
                    format!("invite {} to your party", r.from),
                    true,
                )),
            },
            Ask::Help(HelpTask::JoinParty) => {
                if !snap.party_play {
                    Err("you play alone".into())
                } else if member {
                    Err("you are in their party already".into())
                } else if !alone {
                    Err("you are in another party".into())
                } else {
                    Ok((
                        help(HelpTask::JoinParty),
                        format!("join the party of {}", r.from),
                        true,
                    ))
                }
            }
            Ask::Help(HelpTask::GivePotions) => {
                if !seen {
                    Err("you do not see them anywhere near".into())
                } else if snap.heal_items <= SPARE_ABOVE {
                    Err("you have no potions to spare".into())
                } else {
                    let take = friend || snap.heal_items > 3 * SPARE_ABOVE;
                    if !take {
                        rather_not = Some(format!(
                            "you need your {} healing items for your own fights",
                            snap.heal_items
                        ));
                    }
                    Ok((
                        help(HelpTask::GivePotions),
                        format!(
                            "give {} a few of your {} healing items",
                            r.from, snap.heal_items
                        ),
                        take,
                    ))
                }
            }
            Ask::Hunt(map) => {
                let goal = Goal::Hunt { map: map.clone() };
                let listed = others.iter().any(|o| o.goal == goal);
                let note = if listed {
                    ""
                } else {
                    " (not one of your hunting grounds)"
                };
                Ok((goal, format!("hunt on {map} with them{note}"), listed))
            }
        };
        let who = if member {
            " (party member)"
        } else if friend {
            " (friend)"
        } else {
            ""
        };
        let said: String = r.text.chars().take(100).collect();
        let verdict = verdict.map(|(goal, does, take)| Opt {
            id: format!("ask:{}", r.id),
            goal,
            line: format!("{}{who} asked \"{said}\": {does}", r.from),
            score: if take { 1.0 } else { 0.0 },
        });
        out.push((r.id, verdict, rather_not));
    }
    out
}

async fn game_options(
    snap: &Snapshot,
    knowledge: &Knowledge,
    state: &StrategicState,
) -> Result<Vec<Opt>> {
    let ws = &snap.ws;
    if ws.in_training {
        return Ok(vec![opt(
            "leave_training",
            Goal::LeaveTraining,
            "talk to the receptionist and go to a town (one way)",
        )]);
    }
    let failed = |g: &Goal| snap.recent_failures.iter().any(|f| f == g);
    let mut out = Vec::new();

    let quest_opts = quest_options(snap, knowledge, state).await?;
    // In the middle of a test, the test is all there is.
    if let Some(o) = quest_opts.iter().find(|o| o.score >= IN_TEST) {
        return Ok(vec![o.clone()]);
    }
    // The Job Master makes first jobs; second jobs come from the guilds' quests.
    if ws.job_change_ready && ws.job.eq_ignore_ascii_case("novice") {
        for job in next_jobs(&ws.job) {
            let goal = Goal::JobChange {
                job: (*job).to_owned(),
            };
            if !failed(&goal) {
                out.push(opt(
                    format!("job_change:{job}"),
                    goal,
                    format!("go to the Job Master in {JOB_MAP} and become a {job}"),
                ));
            }
        }
        // A job change is always worth more than another hunt, but the model kept hunting when offered both.
        // So it only chooses which job, and only when no target is set yet.
        if !out.is_empty() {
            if let Some(target) = &state.job_target
                && let Some(i) = out
                    .iter()
                    .position(|o| o.id.eq_ignore_ascii_case(&format!("job_change:{target}")))
            {
                return Ok(vec![out.swap_remove(i)]);
            }
            return Ok(out);
        }
    }

    out.extend(quest_opts);
    let fighter = knowledge.fighter(&snap.obs).await?;
    let avoid_maps: HashSet<String> = state.avoid_maps.iter().cloned().collect();
    let avoid_mobs: HashSet<String> = state
        .avoided_mobs(snap.obs.me.base_level)
        .into_iter()
        .chain(snap.losing_to.iter().cloned())
        .collect();
    let grounds = hunting_grounds(
        &knowledge.db,
        &knowledge.graph,
        &fighter,
        &ws.map,
        HUNT_HOPS,
        &avoid_maps,
        &avoid_mobs,
        HUNT_OPTIONS + 3,
    )
    .await?;
    // The model weighs facts it reads next to an option, and overlooks them elsewhere in the observation.
    let here = grounds
        .iter()
        .find(|g| g.map == ws.map)
        .map(|g| g.exp_rate)
        .filter(|r| *r > 0.0);
    let mut hunts = 0;
    for g in &grounds {
        let goal = Goal::Hunt { map: g.map.clone() };
        if failed(&goal) || hunts >= HUNT_OPTIONS {
            continue;
        }
        hunts += 1;
        let current = if snap.goal.as_ref() == Some(&goal) {
            " [current]"
        } else {
            ""
        };
        let compare = match here {
            Some(r) if g.map != ws.map => {
                format!(" ({:.1}x the exp/s of the current map)", g.exp_rate / r)
            }
            _ => String::new(),
        };
        out.push(Opt {
            id: format!("hunt:{}", g.map),
            goal,
            line: format!("hunt: {}{current}{compare}", g.describe()),
            score: g.score,
        });
    }

    if snap.loot_items > 0 && !failed(&Goal::SellLoot) {
        out.push(opt(
            "sell_loot",
            Goal::SellLoot,
            format!(
                "sell {} loot items at the shop (weight {:.0}%, healing {:.1}x max HP, zeny {})",
                snap.loot_items,
                snap.weight_frac * 100.0,
                heal_ratio(snap),
                snap.obs.me.zeny
            ),
        ));
    }
    let price = if snap.potion_price > 0 {
        snap.potion_price
    } else {
        DEFAULT_POTION_PRICE
    };
    let potions = heal_ratio(snap) < HEAL_STOCK_WANTED && snap.obs.me.zeny >= price * 3 + 100;
    if (potions || snap.ws.ammo_low) && !failed(&Goal::BuyPotions) {
        let arrows = if snap.ws.ammo_low {
            format!(" and arrows ({} left)", arrow_count(&snap.obs))
        } else {
            String::new()
        };
        out.push(opt(
            "buy_potions",
            Goal::BuyPotions,
            format!(
                "buy Red Potions at {price} z each{arrows} (healing {:.1}x max HP, zeny {})",
                heal_ratio(snap),
                snap.obs.me.zeny
            ),
        ));
    }
    if snap.obs.me.hp_frac() < 0.9 {
        out.push(opt("rest", Goal::Rest, "stand still until HP is back"));
    }
    let budget = gear_budget(snap.obs.me.zeny, snap.potion_price);
    if budget > 0
        && let Some(up) = knowledge
            .best_upgrade(
                &snap.obs,
                budget,
                GEAR_HOPS,
                next_job(state, &snap.ws.job),
                &snap.bad_shops,
            )
            .await?
    {
        let goal = Goal::BuyGear {
            item: up.item_id,
            name: up.name.clone(),
            map: up.shop.map.id_str(),
            x: up.shop.x,
            y: up.shop.y,
        };
        if !failed(&goal) {
            let what = if up.slot == Slot::Weapon {
                "ATK"
            } else {
                "worth"
            };
            out.push(opt(
                format!("gear:{}", up.item_id),
                goal,
                format!(
                    "buy {} ({}) for {} z on {} ({} warps away): {what} {:.0} instead of {:.0}",
                    up.name,
                    up.slot.label(),
                    up.price,
                    up.shop.map.id_str(),
                    up.hops,
                    up.value,
                    up.replaces
                ),
            ));
        }
    }

    // A couple of unvisited maps nearby, for curiosity.
    let mut explore = 0;
    for (map, hops) in knowledge.graph.neighbourhood(&ws.map, 2).await? {
        if explore >= 2
            || hops == 0
            || snap.visited.contains(&map)
            || avoid_maps.contains(&map)
            || !is_hunting_map_name(&map)
        {
            continue;
        }
        let goal = Goal::Explore { map: map.clone() };
        if failed(&goal) {
            continue;
        }
        explore += 1;
        out.push(opt(
            format!("explore:{map}"),
            goal,
            format!("visit {map} for the first time ({hops} warps away)"),
        ));
    }
    if out.is_empty() {
        out.push(opt("rest", Goal::Rest, "nothing else to do: stand still"));
    }
    Ok(out)
}

/// The quests the bot may take up now, each with the goal of its next stage: the stage itself, or buying or
/// hunting what it asks for.
async fn quest_options(
    snap: &Snapshot,
    knowledge: &Knowledge,
    state: &StrategicState,
) -> Result<Vec<Opt>> {
    let obs = &snap.obs;
    let failed = |g: &Goal| snap.recent_failures.iter().any(|f| f == g);
    let mut out = Vec::new();
    for q in quests::all() {
        let progress = snap.quests.get(q.id);
        if !q.open_to(obs) || !awake(progress) {
            continue;
        }
        if let Reward::Job(job) = q.reward {
            let wanted = match &state.job_target {
                Some(target) => same_job(target, job),
                None => next_jobs(&obs.me.job).iter().any(|j| same_job(j, job)),
            };
            if !wanted {
                continue;
            }
        }
        let p = progress.cloned().unwrap_or_default();
        let i = q.actionable_stage(&p, obs);
        let Some(stage) = q.stages.get(i) else {
            continue;
        };
        let head = format!("{} (step {} of {})", q.title, i + 1, q.stages.len());
        let score = if stage.inside && stage_map(stage) == Some(obs.map.name.as_str()) {
            IN_TEST
        } else if matches!(q.reward, Reward::Job(_)) {
            JOB_QUEST
        } else if i > 0
            && !next_jobs(&obs.me.job)
                .iter()
                .any(|next| q.jobs.iter().any(|j| same_job(next, j)))
        {
            CLOSING_QUEST
        } else {
            SKILL_QUEST
        };
        let option = match &stage.act {
            Act::Gather(need) => {
                gather_option(snap, knowledge, state, q, &p, need, &head, stage.what)
                    .await?
                    .filter(|o| !failed(&o.goal))
            }
            _ => {
                let Some((map, spot)) = stage_place(q, i) else {
                    continue;
                };
                let goal = Goal::Quest {
                    id: q.id.into(),
                    stage: i,
                    what: stage.what.into(),
                    map: map.into(),
                    x: spot.0,
                    y: spot.1,
                };
                (!failed(&goal)).then(|| {
                    opt(
                        format!("quest:{}", q.id),
                        goal,
                        format!("{head}: {} on {map}", stage.what),
                    )
                })
            }
        };
        if let Some(mut o) = option {
            o.score = score;
            out.push(o);
        }
    }
    Ok(out)
}

/// How to get the first item a gathering stage lacks that the bot can get: bought nearby when it has the zeny,
/// else hunted.
#[allow(clippy::too_many_arguments)]
async fn gather_option(
    snap: &Snapshot,
    knowledge: &Knowledge,
    state: &StrategicState,
    q: &Quest,
    p: &QuestProgress,
    need: &Need,
    head: &str,
    what: &str,
) -> Result<Option<Opt>> {
    let obs = &snap.obs;
    let here = obs.map.name.as_str();
    let avoid_maps: HashSet<String> = state.avoid_maps.iter().cloned().collect();
    let avoid_mobs: HashSet<String> = state
        .avoided_mobs(snap.obs.me.base_level)
        .into_iter()
        .chain(snap.losing_to.iter().cloned())
        .collect();
    let id = format!("quest:{}", q.id);
    // Zeny kept for potions.
    let spare = obs.me.zeny.saturating_sub(500);
    for (item, amount) in quests::missing(need, p, obs) {
        let Ok(data) = knowledge.db.item(item).await else {
            continue;
        };
        let name = data.name_english.clone();
        let mut shop: Option<(u32, &crate::knowledge::db::ShopPlace)> = None;
        for place in data
            .sold_by
            .iter()
            .filter(|s| s.price * amount as u64 <= spare && !snap.bad_shops.contains(&s.key()))
        {
            let map = place.map.id_str();
            let hops = if map == here {
                0
            } else {
                match knowledge.graph.route(here, &map, &avoid_maps).await? {
                    Some(route) => route.len() as u32,
                    None => continue,
                }
            };
            if hops <= QUEST_SHOP_HOPS && shop.is_none_or(|(h, _)| hops < h) {
                shop = Some((hops, place));
            }
        }
        if let Some((hops, place)) = shop {
            let map = place.map.id_str();
            let npc = place
                .npc
                .name
                .split('#')
                .next()
                .unwrap_or_default()
                .to_owned();
            let goal = Goal::BuyItem {
                item,
                amount: obs.item_count(item) + amount,
                name: name.clone(),
                map: map.clone(),
                x: place.x,
                y: place.y,
            };
            return Ok(Some(opt(
                id,
                goal,
                format!(
                    "{head}: {what}: buy {amount} {name} from the {npc} on {map} ({hops} warps away, {} z each)",
                    place.price
                ),
            )));
        }
        let fighter = knowledge.fighter(obs).await?;
        if let Some((ground, mob)) = farm_ground(
            &knowledge.db,
            &knowledge.graph,
            &fighter,
            here,
            &data,
            QUEST_FARM_HOPS,
            &avoid_maps,
            &avoid_mobs,
        )
        .await?
        {
            return Ok(Some(opt(
                id,
                Goal::Hunt {
                    map: ground.map.clone(),
                },
                format!(
                    "{head}: {what}: hunt {mob} on {} for {amount} more {name} ({} warps away)",
                    ground.map, ground.hops
                ),
            )));
        }
    }
    Ok(None)
}

/// Healing carried, in multiples of the max HP.
pub fn heal_ratio(snap: &Snapshot) -> f32 {
    snap.heal_hp as f32 / snap.obs.me.max_hp.max(1) as f32
}

/// The job a build suits.
/// The first job a Novice heads for: the target's, or the one its stat build leads to. Past Novice, a second job
/// wears what its first job wears, so none.
fn next_job(state: &StrategicState, job: &str) -> Option<&'static str> {
    if !job.eq_ignore_ascii_case("novice") {
        return None;
    }
    let target = state
        .job_target
        .as_deref()
        .unwrap_or_else(|| job_for_build(&state.stat_build));
    super::skill_state::job_path(target).map(|(path, _)| path[0])
}

pub fn job_for_build(build: &str) -> &'static str {
    match build {
        "caster" => "Mage",
        "archer" => "Archer",
        _ => "Swordman",
    }
}

/// Deterministic choice: progress first, then upkeep, then the best hunting ground.
pub fn fallback(snap: &Snapshot, options: &[Opt], state: &StrategicState) -> Option<Opt> {
    let find = |id: &str| options.iter().find(|o| o.id == id).cloned();
    if let Some(o) = find("leave_training") {
        return Some(o);
    }
    if let Some(o) = options
        .iter()
        .find(|o| o.id.starts_with("quest:") && o.score >= IN_TEST)
    {
        return Some(o.clone());
    }
    // Someone asked for something the bot gladly does
    if let Some(o) = options
        .iter()
        .find(|o| o.id.starts_with("ask:") && o.score > 0.0)
    {
        return Some(o.clone());
    }
    // A skill only this job learns, half earned: the job change would end it.
    if let Some(o) = options
        .iter()
        .find(|o| o.id.starts_with("quest:skill:") && o.score >= CLOSING_QUEST)
    {
        return Some(o.clone());
    }
    let job = state
        .job_target
        .clone()
        .unwrap_or_else(|| job_for_build(&state.stat_build).to_owned());
    if let Some(o) = find(&format!("job_change:{job}")) {
        return Some(o);
    }
    // Past the first job: the first offered branch.
    if let Some(o) = options
        .iter()
        .find(|o| o.id.starts_with("job_change:") && !snap.ws.job.eq_ignore_ascii_case("novice"))
    {
        return Some(o.clone());
    }
    // Without potions and the zeny to buy them, loot is the only way to get some.
    let broke = heal_ratio(snap) < HEAL_STOCK_MIN
        && snap.obs.me.zeny < DEFAULT_POTION_PRICE * 5
        && snap.loot_items >= 15;
    if snap.weight_frac >= 0.4 || snap.loot_items >= 60 || broke {
        if let Some(o) = find("sell_loot") {
            return Some(o);
        }
    }
    if (heal_ratio(snap) < HEAL_STOCK_MIN && snap.obs.me.base_level >= 5) || snap.ws.ammo_low {
        if let Some(o) = find("buy_potions") {
            return Some(o);
        }
    }
    // The quest to the next job.
    if let Some(o) = options.iter().find(|o| o.id.starts_with("quest:job:")) {
        return Some(o.clone());
    }
    // Better equipment the bot can pay for, potions kept aside.
    if let Some(o) = options.iter().find(|o| o.id.starts_with("gear:")) {
        return Some(o.clone());
    }
    // A skill a guild teaches for errands.
    if let Some(o) = options.iter().find(|o| o.id.starts_with("quest:skill:")) {
        return Some(o.clone());
    }
    let best = options
        .iter()
        .filter(|o| o.id.starts_with("hunt:"))
        .max_by(|a, b| a.score.total_cmp(&b.score));
    if let Some(current) = options
        .iter()
        .find(|o| o.id.starts_with("hunt:") && Some(&o.goal) == snap.goal.as_ref())
        && let Some(best) = best
        && current.score >= best.score * 0.7
    {
        return Some(current.clone());
    }
    best.cloned()
        .or_else(|| {
            options
                .iter()
                .find(|o| o.id.starts_with("explore:"))
                .cloned()
        })
        .or_else(|| options.first().cloned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Party, PartyMember, Player};
    use crate::social::{Channel, Request};

    fn request(id: u64, from: &str, ask: Ask) -> Request {
        Request {
            id,
            from: from.into(),
            channel: Channel::Whisper,
            text: "...".into(),
            ask,
            at: std::time::Instant::now(),
        }
    }

    /// Alfa, level 20 with spare potions, leads a party with Bravo; Tom (level 22) and Old (level 60) stand by.
    fn social_snapshot(requests: Vec<Request>) -> Snapshot {
        let mut snap = Snapshot {
            requests,
            party_play: true,
            heal_items: 40,
            ..Default::default()
        };
        let obs = &mut snap.obs;
        obs.me.name = "Alfa".into();
        obs.me.base_level = 20;
        let member = |name: &str, leader| PartyMember {
            name: name.into(),
            leader,
            online: true,
            ..Default::default()
        };
        obs.party = Some(Party {
            name: "Wanderers".into(),
            members: vec![member("Alfa", true), member("Bravo", false)],
            ..Default::default()
        });
        obs.players = vec![
            Player {
                id: 1,
                name: "Tom".into(),
                base_level: 22,
                ..Default::default()
            },
            Player {
                id: 2,
                name: "Old".into(),
                base_level: 60,
                ..Default::default()
            },
            Player {
                id: 3,
                name: "Bravo".into(),
                base_level: 19,
                party: true,
                ..Default::default()
            },
            Player {
                id: 4,
                name: "Far".into(),
                x: 120,
                ..Default::default()
            },
        ];
        snap
    }

    #[test]
    fn requests_become_options_the_fallback_takes_when_they_cost_little() {
        let snap = social_snapshot(vec![
            request(1, "Bravo", Ask::Help(HelpTask::Follow)),
            request(2, "Old", Ask::Help(HelpTask::Follow)),
            request(3, "Tom", Ask::Help(HelpTask::Invite)),
            request(4, "Tom", Ask::Help(HelpTask::JoinParty)),
            request(5, "Nobody", Ask::Help(HelpTask::GivePotions)),
            request(8, "Far", Ask::Help(HelpTask::Follow)),
            request(6, "Tom", Ask::Hunt("prt_fild08".into())),
            request(7, "Tom", Ask::Hunt("gef_dun03".into())),
        ]);
        let hunts = vec![hunt("prt_fild08", 1.0)];
        let opts = request_options(&snap, &hunts);
        let score = |id: &str| opts.iter().find(|o| o.id == id).map(|o| o.score);
        assert_eq!(score("ask:1"), Some(1.0), "a member");
        assert_eq!(score("ask:2"), Some(1.0), "a stranger in sight");
        assert_eq!(score("ask:3"), Some(1.0), "the leader has room");
        assert_eq!(score("ask:4"), None, "already in a party of two");
        assert_eq!(score("ask:5"), None, "out of sight");
        assert_eq!(
            score("ask:6"),
            Some(1.0),
            "one of the bot's hunting grounds"
        );
        assert_eq!(score("ask:7"), Some(0.0));
        assert_eq!(score("ask:8"), None, "on the map but out of sight");
        let refusals = request_refusals(&snap);
        assert_eq!(
            refusals.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![4, 5, 8]
        );
        assert_eq!(refusals[2].1, "you do not see them anywhere near");
        assert!(
            opts.iter()
                .find(|o| o.id == "ask:2")
                .unwrap()
                .line
                .starts_with("Old asked")
        );

        let mut options = hunts.clone();
        options.extend(opts);
        assert_eq!(
            fallback(&snap, &options, &StrategicState::default())
                .unwrap()
                .id,
            "ask:1"
        );
    }

    #[test]
    fn a_favour_is_kept_until_it_is_over() {
        let mut snap = social_snapshot(vec![request(1, "Tom", Ask::Help(HelpTask::Invite))]);
        snap.goal = Some(Goal::Help {
            who: "Tom".into(),
            task: HelpTask::Follow,
        });
        let state = StrategicState::default();
        // The favour comes first: the reference is not asked
        let knowledge =
            Knowledge::new(crate::knowledge::db::Db::new("http://127.0.0.1:9", None).unwrap());
        let options = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(build_options(&snap, &knowledge, &state))
            .unwrap();
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].goal, snap.goal.clone().unwrap());
    }

    fn hunt(map: &str, score: f32) -> Opt {
        Opt {
            id: format!("hunt:{map}"),
            goal: Goal::Hunt { map: map.into() },
            line: String::new(),
            score,
        }
    }

    #[test]
    fn fallback_keeps_a_good_current_map() {
        let snap = Snapshot {
            goal: Some(Goal::Hunt { map: "a".into() }),
            ..Default::default()
        };
        let options = vec![hunt("a", 0.8), hunt("b", 1.0)];
        assert_eq!(
            fallback(&snap, &options, &StrategicState::default())
                .unwrap()
                .id,
            "hunt:a"
        );
        let options = vec![hunt("a", 0.5), hunt("b", 1.0)];
        assert_eq!(
            fallback(&snap, &options, &StrategicState::default())
                .unwrap()
                .id,
            "hunt:b"
        );
    }

    #[test]
    fn fallback_sells_when_heavy() {
        let snap = Snapshot {
            weight_frac: 0.45,
            ..Default::default()
        };
        let options = vec![hunt("a", 1.0), opt("sell_loot", Goal::SellLoot, "")];
        assert_eq!(
            fallback(&snap, &options, &StrategicState::default())
                .unwrap()
                .id,
            "sell_loot"
        );
    }

    #[test]
    fn fallback_sells_when_broke_without_potions() {
        let mut snap = Snapshot {
            loot_items: 20,
            heal_hp: 100,
            ..Default::default()
        };
        snap.obs.me.max_hp = 200;
        let options = vec![hunt("a", 1.0), opt("sell_loot", Goal::SellLoot, "")];
        assert_eq!(
            fallback(&snap, &options, &StrategicState::default())
                .unwrap()
                .id,
            "sell_loot"
        );
    }

    #[test]
    fn fallback_buys_gear_before_hunting_and_arrows_first() {
        let gear = Goal::BuyGear {
            item: 1107,
            name: "Blade".into(),
            map: "prt_in".into(),
            x: 1,
            y: 2,
        };
        let options = vec![
            hunt("a", 1.0),
            opt("gear:1107", gear, ""),
            opt("buy_potions", Goal::BuyPotions, ""),
        ];
        let mut snap = Snapshot {
            heal_hp: 1000,
            ..Default::default()
        };
        snap.obs.me.max_hp = 200;
        assert_eq!(
            fallback(&snap, &options, &StrategicState::default())
                .unwrap()
                .id,
            "gear:1107"
        );
        snap.ws.ammo_low = true;
        assert_eq!(
            fallback(&snap, &options, &StrategicState::default())
                .unwrap()
                .id,
            "buy_potions"
        );
    }

    #[test]
    fn fallback_finishes_a_closing_skill_quest_before_the_job_change() {
        let snap = Snapshot::default();
        let quest = Opt {
            id: "quest:skill:NV_FIRSTAID".into(),
            goal: Goal::Hunt {
                map: "prt_fild08".into(),
            },
            line: String::new(),
            score: CLOSING_QUEST,
        };
        let change = Opt {
            id: "job_change:Swordman".into(),
            goal: Goal::JobChange {
                job: "Swordman".into(),
            },
            line: String::new(),
            score: 0.0,
        };
        let state = StrategicState::default();
        assert_eq!(
            fallback(&snap, &[change.clone(), quest.clone()], &state).map(|o| o.id),
            Some(quest.id.clone())
        );
        let fresh = Opt {
            score: SKILL_QUEST,
            ..quest
        };
        assert_eq!(
            fallback(&snap, &[change.clone(), fresh], &state).map(|o| o.id),
            Some(change.id)
        );
    }

    #[test]
    fn fallback_takes_the_job_matching_the_build() {
        let options = vec![
            opt(
                "job_change:Mage",
                Goal::JobChange { job: "Mage".into() },
                "",
            ),
            opt(
                "job_change:Swordman",
                Goal::JobChange {
                    job: "Swordman".into(),
                },
                "",
            ),
        ];
        let state = StrategicState {
            stat_build: "caster".into(),
            ..Default::default()
        };
        assert_eq!(
            fallback(&Snapshot::default(), &options, &state).unwrap().id,
            "job_change:Mage"
        );
    }
}
