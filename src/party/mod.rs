//! Playing in a party with only what a player knows: the party window (the members, their map and, on the same map,
//! their position and HP bar), party chat, and what is in sight. Nothing passes between the bots any other way; a
//! person can lead them or play along with the same words (see `chat`).
//!
//! The leader creates the party, invites its friends, turns experience sharing on, and hunts where its strategist
//! says, telling the party where. The others accept their friends' invitations and follow the leader: they walk to its
//! map, keep near it, fight what attacks a member, then what the leader hits, then what is near the leader. Errands
//! (selling, buying potions, a job change) still go first, with a word to the party.

pub mod chat;
pub mod support;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::ValueEnum;

use crate::api::{Action, Observation, Party, PartyMember, dist};
use crate::bot::{Bot, Outcome};
use crate::goap::Goal;
use crate::goap::state::HP_LOW;
use chat::Said;

/// How far a character sees around itself, in cells.
pub const SIGHT: u32 = 14;
/// A follower walks back to the one it follows when further than this.
const FOLLOW_FAR: u32 = 7;
/// A follower fights monsters this close to the one it follows.
const ASSIST_RADIUS: u32 = 10;
/// Basic Skill level that creating a party takes.
const BASIC_SKILL_TO_CREATE: u32 = 7;
const CREATE_EVERY: Duration = Duration::from_secs(30);
const INVITE_EVERY: Duration = Duration::from_secs(60);
const SHARE_EVERY: Duration = Duration::from_secs(120);
/// Shortest time between two lines of party chat.
const CHAT_GAP: Duration = Duration::from_secs(2);
/// An errand someone announced is over after this long, even without a "back".
const AWAY_LASTS: Duration = Duration::from_secs(600);
const HEAL_REQUEST_LASTS: Duration = Duration::from_secs(20);
const POTION_REQUEST_LASTS: Duration = Duration::from_secs(180);
const ASK_POTIONS_EVERY: Duration = Duration::from_secs(300);
const ASK_HEAL_EVERY: Duration = Duration::from_secs(15);
/// HP share under which a member asks a healer in sight for a heal.
const ASK_HEAL_BELOW: f32 = 0.4;

/// What the bot does for the party, and so the job it heads for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Role {
    /// Acolyte, then Priest: heals and buffs the members.
    Healer,
    /// Swordman, then Knight: draws the monsters that attack the others.
    Tank,
    /// Whatever job the strategist picks.
    Fighter,
}

impl Role {
    /// The jobs of the role, first then second.
    pub fn jobs(self) -> &'static [&'static str] {
        match self {
            Role::Healer => &["Acolyte", "Priest"],
            Role::Tank => &["Swordman", "Knight"],
            Role::Fighter => &[],
        }
    }

    pub fn stat_build(self) -> Option<&'static str> {
        match self {
            Role::Healer => Some("caster"),
            Role::Tank => Some("tank"),
            Role::Fighter => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PartyConfig {
    /// The party to create, when the bot leads.
    pub name: String,
    /// The characters the bot plays with: it invites them when it leads, and takes only their invitations.
    pub friends: Vec<String>,
    pub lead: bool,
    pub role: Role,
}

impl PartyConfig {
    pub fn is_friend(&self, name: &str) -> bool {
        self.friends.iter().any(|f| f.eq_ignore_ascii_case(name))
    }
}

/// What the bot knows of its party and has said to it.
pub struct PartyPlay {
    pub config: PartyConfig,
    last_seq: u64,
    /// The map the leader last said it hunts on.
    pub hunt_map: Option<String>,
    /// Someone said "follow me": follow them rather than the leader.
    follow: Option<String>,
    /// Someone said "stay here".
    staying: bool,
    /// Members off on an errand, since when.
    away: HashMap<String, Instant>,
    heal_requests: HashMap<String, Instant>,
    potion_requests: HashMap<String, Instant>,
    /// Base and job level the members told.
    pub levels: HashMap<String, (u32, u32)>,
    last_said: Option<Instant>,
    last_create: Option<Instant>,
    last_invites: HashMap<String, Instant>,
    last_share: Option<Instant>,
    announced_hunt: Option<String>,
    announced_level: Option<(u32, u32)>,
    errand: Option<String>,
    asked_potions: Option<Instant>,
    asked_heal: Option<Instant>,
    /// When the bot last cast a buff on someone, by (name, skill): a player remembers it, nothing shows it.
    pub buffs: HashMap<(String, &'static str), Instant>,
    /// When the current trade window opened.
    pub trade_since: Option<Instant>,
}

impl PartyPlay {
    pub fn new(config: PartyConfig, last_seq: u64) -> Self {
        Self {
            config,
            last_seq,
            hunt_map: None,
            follow: None,
            staying: false,
            away: HashMap::new(),
            heal_requests: HashMap::new(),
            potion_requests: HashMap::new(),
            levels: HashMap::new(),
            last_said: None,
            last_create: None,
            last_invites: HashMap::new(),
            last_share: None,
            announced_hunt: None,
            announced_level: None,
            errand: None,
            asked_potions: None,
            asked_heal: None,
            buffs: HashMap::new(),
            trade_since: None,
        }
    }

    /// Takes in the party chat said since the last call.
    pub fn hear(&mut self, obs: &Observation) {
        let me = obs.me.name.clone();
        let leader = obs.party.as_ref().and_then(|p| p.leader.clone());
        let last_seq = self.last_seq;
        for message in obs.messages.iter().filter(|m| m.seq > last_seq && m.channel == "party") {
            let Some(line) = chat::parse(&message.text) else { continue };
            if line.from.eq_ignore_ascii_case(&me) {
                continue;
            }
            tracing::info!(from = %line.from, said = ?line.said, "party chat");
            let now = Instant::now();
            match line.said {
                Said::Hunting(map) => {
                    if leader.as_deref().is_some_and(|l| l.eq_ignore_ascii_case(&line.from)) {
                        self.hunt_map = Some(map);
                    }
                }
                Said::FollowMe => {
                    self.follow = Some(line.from);
                    self.staying = false;
                }
                Said::Stay => self.staying = true,
                Said::Level { base, job, .. } => {
                    self.levels.insert(line.from, (base, job));
                }
                Said::NeedHeal => {
                    self.heal_requests.insert(line.from, now);
                }
                Said::NeedPotions => {
                    self.potion_requests.insert(line.from, now);
                }
                // Another member takes care of it
                Said::SendingPotions(to) => self.potions_given(&to),
                Said::Away(_) => {
                    self.away.insert(line.from, now);
                }
                Said::Back => {
                    self.away.remove(&line.from);
                }
                Said::Other(_) => {}
            }
        }
        self.last_seq = self.last_seq.max(obs.last_message_seq());
        self.away.retain(|_, at| at.elapsed() < AWAY_LASTS);
        self.heal_requests.retain(|_, at| at.elapsed() < HEAL_REQUEST_LASTS);
        self.potion_requests.retain(|_, at| at.elapsed() < POTION_REQUEST_LASTS);
    }

    pub fn wants_heal(&self, name: &str) -> bool {
        self.heal_requests.keys().any(|n| n.eq_ignore_ascii_case(name))
    }

    /// Who asked for potions, and how long ago.
    pub fn potion_requests(&self) -> Vec<(String, Duration)> {
        self.potion_requests.iter().map(|(name, at)| (name.clone(), at.elapsed())).collect()
    }

    pub fn potions_given(&mut self, to: &str) {
        self.potion_requests.retain(|n, _| !n.eq_ignore_ascii_case(to));
    }

    fn is_away(&self, name: &str) -> bool {
        self.away.keys().any(|n| n.eq_ignore_ascii_case(name))
    }

    /// The member to follow: the one who said "follow me", or else the leader. `None` for the leader itself, after
    /// "stay here", and when that member is out of the game or away on an errand.
    pub fn followed<'a>(&self, obs: &'a Observation) -> Option<&'a PartyMember> {
        let party = obs.party.as_ref()?;
        if self.staying {
            return None;
        }
        let me = &obs.me.name;
        let wanted = self.follow.as_deref().or(party.leader.as_deref())?;
        party
            .members
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(wanted) && !m.name.eq_ignore_ascii_case(me) && m.online && !self.is_away(&m.name))
    }

    fn may_speak(&self) -> bool {
        self.last_said.is_none_or(|at| at.elapsed() >= CHAT_GAP)
    }
}

/// The character ids of the party members on the bot's map, the bot included.
pub fn member_ids(obs: &Observation) -> Vec<u64> {
    let mut ids: Vec<u64> = obs.players.iter().filter(|p| p.party).map(|p| p.id).collect();
    if let Some(party) = &obs.party {
        ids.extend(party.members.iter().filter(|m| m.map == obs.map.name).filter_map(|m| m.char_id));
    }
    ids.push(obs.me.char_id);
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Someone outside the party fights this monster: it is theirs (it attacks them, or they hit it).
pub fn taken_by_others(obs: &Observation, mob_id: u64, mob_target: Option<u64>, members: &[u64]) -> bool {
    mob_target.is_some_and(|t| !members.contains(&t))
        || obs.players.iter().any(|p| !p.party && !members.contains(&p.id) && p.attack_target == Some(mob_id))
}

/// The monster that attacks a party member, nearest to the bot first: the first thing to fight in a party.
pub fn attacking_a_member(obs: &Observation, members: &[u64], radius: u32) -> Option<(u64, String)> {
    obs.mobs
        .iter()
        .filter(|m| m.hp > 0 && m.target.is_some_and(|t| members.contains(&t)) && dist((m.x, m.y), obs.pos()) <= radius)
        .min_by_key(|m| dist((m.x, m.y), obs.pos()))
        .map(|m| (m.id, m.name.clone()))
}

fn errand_label(goal: &Goal) -> Option<String> {
    match goal {
        Goal::SellLoot => Some("selling loot".into()),
        Goal::BuyPotions => Some("buying potions".into()),
        Goal::JobChange { job } => Some(format!("changing job to {job}")),
        Goal::LeaveTraining => Some("leaving the training grounds".into()),
        _ => None,
    }
}

fn leader_of(party: &Party) -> Option<&PartyMember> {
    party.members.iter().find(|m| m.leader)
}

impl Bot {
    /// Says something on the party channel, at most every `CHAT_GAP`. Returns whether it was said.
    pub async fn say_to_party(&mut self, said: Said) -> bool {
        let Some(play) = self.party.as_mut() else { return false };
        if !play.may_speak() {
            return false;
        }
        play.last_said = Some(Instant::now());
        let text = said.text();
        match self.act(Action::PartyChat { text: text.clone() }).await {
            Ok(_) => {
                tracing::info!(%text, "said to the party");
                true
            }
            Err(error) => {
                tracing::debug!(%error, "party chat refused");
                false
            }
        }
    }

    /// The party's quick reactions, before any goal: invitations, trades, forming the party, telling the others what
    /// they need to know, and the support role. Returns whether it acted, which ends the tick.
    pub async fn party_reflexes(&mut self, obs: &Observation, goal: Option<&Goal>) -> Result<bool> {
        let Some(play) = self.party.as_mut() else { return Ok(false) };
        play.hear(obs);
        let config = play.config.clone();

        if let Some(invitation) = &obs.party_invitation {
            let accept = obs.party.is_none() && config.is_friend(&invitation.inviter_name);
            tracing::info!(from = %invitation.inviter_name, accept, "party invitation");
            self.act(if accept { Action::PartyAccept } else { Action::PartyDecline }).await.ok();
            return Ok(true);
        }
        if self.answer_trade(obs).await? {
            return Ok(true);
        }
        if config.lead && self.form_party(obs).await? {
            return Ok(true);
        }
        let Some(party) = &obs.party else { return Ok(false) };

        // Tell the others the level (for experience sharing, which needs levels within 15) and where the hunt is.
        let level = (obs.me.base_level, obs.me.job_level);
        let play = self.party.as_ref().expect("checked above");
        if play.announced_level != Some(level) && play.may_speak() {
            if self.say_to_party(Said::Level { base: level.0, job: level.1, job_name: obs.me.job.clone() }).await {
                self.party.as_mut().expect("checked above").announced_level = Some(level);
            }
            return Ok(false);
        }
        let leads = leader_of(party).is_some_and(|l| l.name.eq_ignore_ascii_case(&obs.me.name));
        if leads && let Some(Goal::Hunt { map }) = goal {
            let play = self.party.as_ref().expect("checked above");
            if play.announced_hunt.as_deref() != Some(map) && play.may_speak() && self.say_to_party(Said::Hunting(map.clone())).await {
                self.party.as_mut().expect("checked above").announced_hunt = Some(map.clone());
            }
        }
        self.ask_for_help(obs).await;
        self.support(obs).await
    }

    /// The leader's part in forming the party: create it, invite the friends who are not in it, share experience.
    async fn form_party(&mut self, obs: &Observation) -> Result<bool> {
        let play = self.party.as_mut().expect("party play");
        let Some(party) = &obs.party else {
            if obs.skill_level("NV_BASIC") < BASIC_SKILL_TO_CREATE || play.last_create.is_some_and(|at| at.elapsed() < CREATE_EVERY) {
                return Ok(false);
            }
            play.last_create = Some(Instant::now());
            let name = play.config.name.clone();
            match self.act(Action::PartyCreate { name: name.clone(), item_pickup: true, item_share: false }).await {
                Ok(_) => tracing::info!(%name, "creating the party"),
                Err(error) => tracing::info!(%error, "could not create the party"),
            }
            return Ok(true);
        };
        if !leader_of(party).is_some_and(|l| l.name.eq_ignore_ascii_case(&obs.me.name)) {
            return Ok(false);
        }
        let missing: Vec<String> = play
            .config
            .friends
            .iter()
            .filter(|f| !f.eq_ignore_ascii_case(&obs.me.name) && !party.members.iter().any(|m| m.name.eq_ignore_ascii_case(f)))
            .filter(|f| play.last_invites.get(*f).is_none_or(|at| at.elapsed() >= INVITE_EVERY))
            .cloned()
            .collect();
        if let Some(friend) = missing.first() {
            play.last_invites.insert(friend.clone(), Instant::now());
            match self.act(Action::PartyInvite { name: friend.clone() }).await {
                Ok(_) => tracing::info!(%friend, "invited to the party"),
                // Not in the game yet: try again later
                Err(error) => tracing::debug!(%friend, %error, "invitation refused"),
            }
            return Ok(true);
        }
        let play = self.party.as_mut().expect("party play");
        if !party.exp_share && party.members.len() > 1 && play.last_share.is_none_or(|at| at.elapsed() >= SHARE_EVERY) {
            play.last_share = Some(Instant::now());
            self.act(Action::PartyOptions { exp_share: Some(true), item_pickup: None, item_share: None }).await.ok();
            if party.exp_share_denied {
                tracing::info!("experience sharing refused: the levels are more than 15 apart");
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// Asks the party for a heal when a healer is in sight, and for potions when none are left.
    async fn ask_for_help(&mut self, obs: &Observation) {
        let play = self.party.as_ref().expect("party play");
        let healer_near = obs.players.iter().any(|p| {
            p.party && matches!(p.job.to_lowercase().as_str(), "acolyte" | "priest" | "monk") && dist((p.x, p.y), obs.pos()) <= SIGHT
        });
        if healer_near && obs.me.hp_frac() < ASK_HEAL_BELOW && play.asked_heal.is_none_or(|at| at.elapsed() >= ASK_HEAL_EVERY) {
            if self.say_to_party(Said::NeedHeal).await {
                self.party.as_mut().expect("party play").asked_heal = Some(Instant::now());
            }
            return;
        }
        let play = self.party.as_ref().expect("party play");
        if self.knowledge.heal_items(obs) == 0 && play.asked_potions.is_none_or(|at| at.elapsed() >= ASK_POTIONS_EVERY) && self.say_to_party(Said::NeedPotions).await {
            self.party.as_mut().expect("party play").asked_potions = Some(Instant::now());
        }
    }

    /// The party's take on the next step: `None` lets the goal of the strategist run, as for a bot playing alone.
    /// Errands go first; otherwise a member follows the leader and fights by its side.
    pub async fn party_step(&mut self, obs: &Observation, goal: Option<&Goal>) -> Result<Option<Outcome>> {
        let Some(play) = self.party.as_ref() else { return Ok(None) };
        if obs.party.is_none() {
            return Ok(None);
        }
        let errand = goal.and_then(errand_label);
        match (&errand, &play.errand) {
            (Some(label), None) => {
                self.say_to_party(Said::Away(label.clone())).await;
                self.party.as_mut().expect("party play").errand = errand.clone();
                return Ok(None);
            }
            (Some(_), Some(_)) => return Ok(None),
            (None, Some(_)) => {
                if self.say_to_party(Said::Back).await {
                    self.party.as_mut().expect("party play").errand = None;
                }
            }
            (None, None) => {}
        }
        if let Some(outcome) = self.give_potions_if_asked(obs).await? {
            return Ok(Some(outcome));
        }
        let Some(followed) = self.party.as_ref().expect("party play").followed(obs).cloned() else { return Ok(None) };
        self.follow(obs, &followed).await.map(Some)
    }

    /// One step at the side of `followed`: to its map, near it, then the fight.
    async fn follow(&mut self, obs: &Observation, followed: &PartyMember) -> Result<Outcome> {
        if followed.map.is_empty() {
            Bot::pause(500).await;
            return Ok(Outcome::Done);
        }
        if followed.map != obs.map.name {
            tracing::info!(to = %followed.map, member = %followed.name, "following to another map");
            return self.travel(&followed.map).await;
        }
        let Some(there) = followed.pos() else {
            Bot::pause(500).await;
            return Ok(Outcome::Done);
        };
        if obs.me.hp_frac() < HP_LOW && obs.me.action != "attacking" {
            return self.rest().await;
        }
        let members = member_ids(obs);
        // A member under attack first, then what the leader hits when it is in sight, then what is near it.
        let target = attacking_a_member(obs, &members, SIGHT).or_else(|| {
            let hit = obs.player_near(&followed.name, SIGHT)?.attack_target?;
            obs.mobs.iter().find(|m| m.id == hit && m.hp > 0).map(|m| (m.id, m.name.clone()))
        });
        let near_warp = |id: u64| obs.mobs.iter().any(|m| m.id == id && obs.near_warp((m.x, m.y), 2));
        let target = match target {
            Some((id, _)) if near_warp(id) => None,
            Some(found) => Some(found),
            None if dist(obs.pos(), there) <= FOLLOW_FAR => self.pick_target(obs, Some((there, ASSIST_RADIUS))).await?,
            None => None,
        };
        if let Some((id, name)) = target {
            return self.fight(id, &name).await;
        }
        if dist(obs.pos(), there) > FOLLOW_FAR {
            return self.walk_next_to(obs, there).await;
        }
        Bot::pause(800).await;
        Ok(Outcome::Done)
    }

    /// Walks to a cell next to `there` (or onto it).
    async fn walk_next_to(&mut self, obs: &Observation, there: (u16, u16)) -> Result<Outcome> {
        if obs.near_warp(there, 1) {
            // A member just arrived through a warp, or stands at one: it moves on soon, wait for it here.
            Bot::pause(800).await;
            return Ok(Outcome::Done);
        }
        for (dx, dy) in [(1, 1), (-1, 1), (1, -1), (-1, -1), (0, 0), (2, 0), (0, 2)] {
            let x = (there.0 as i32 + dx).max(0) as u16;
            let y = (there.1 as i32 + dy).max(0) as u16;
            if obs.near_warp((x, y), 0) {
                continue;
            }
            if self.act(Action::Move { x, y, wait: true }).await.is_ok() {
                return Ok(Outcome::Done);
            }
        }
        Ok(Outcome::failed("cannot walk to the party"))
    }

    /// For the leader: a member in sight with little HP left. The leader waits for it before pulling more.
    pub fn member_in_trouble(&self, obs: &Observation) -> bool {
        let Some(party) = &obs.party else { return false };
        party.members.iter().any(|m| {
            !m.name.eq_ignore_ascii_case(&obs.me.name)
                && m.map == obs.map.name
                && m.hp_frac().is_some_and(|f| f < 0.3)
                && m.pos().is_some_and(|p| dist(p, obs.pos()) <= SIGHT)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Message, Mob, Player};

    fn config() -> PartyConfig {
        PartyConfig { name: "Wanderers".into(), friends: vec!["Alpha".into(), "Beta".into()], lead: false, role: Role::Fighter }
    }

    fn member(name: &str, leader: bool, map: &str, pos: Option<(u16, u16)>) -> PartyMember {
        PartyMember {
            name: name.into(),
            char_id: Some(if leader { 1 } else { 2 }),
            map: map.into(),
            leader,
            online: true,
            x: pos.map(|p| p.0),
            y: pos.map(|p| p.1),
            ..Default::default()
        }
    }

    fn observation() -> Observation {
        let mut obs = Observation::default();
        obs.me.name = "Beta".into();
        obs.me.char_id = 2;
        obs.me.x = 100;
        obs.me.y = 100;
        obs.map.name = "prt_fild08".into();
        obs.party = Some(Party {
            name: "Wanderers".into(),
            leader: Some("Alpha".into()),
            members: vec![member("Alpha", true, "prt_fild08", Some((110, 100))), member("Beta", false, "prt_fild08", Some((100, 100)))],
            ..Default::default()
        });
        obs
    }

    fn party_line(seq: u64, text: &str) -> Message {
        Message { seq, channel: "party".into(), text: text.into() }
    }

    #[test]
    fn a_member_follows_the_leader_until_told_otherwise() {
        let mut obs = observation();
        let mut play = PartyPlay::new(config(), 0);
        assert_eq!(play.followed(&obs).map(|m| m.name.as_str()), Some("Alpha"));
        obs.messages = vec![party_line(1, "Alpha : stay here")];
        play.hear(&obs);
        assert!(play.followed(&obs).is_none());
        obs.messages.push(party_line(2, "Alpha : follow me"));
        play.hear(&obs);
        assert_eq!(play.followed(&obs).map(|m| m.name.as_str()), Some("Alpha"));
        obs.messages.push(party_line(3, "Alpha : brb, selling loot"));
        play.hear(&obs);
        assert!(play.followed(&obs).is_none(), "the leader is away");
        obs.messages.push(party_line(4, "Alpha : back"));
        obs.messages.push(party_line(5, "Alpha : hunting at prt_fild07"));
        play.hear(&obs);
        assert_eq!(play.followed(&obs).map(|m| m.name.as_str()), Some("Alpha"));
        assert_eq!(play.hunt_map.as_deref(), Some("prt_fild07"));
    }

    #[test]
    fn the_leader_follows_nobody_and_old_lines_are_not_heard_again() {
        let mut obs = observation();
        obs.me.name = "Alpha".into();
        let mut play = PartyPlay::new(config(), 5);
        assert!(play.followed(&obs).is_none());
        obs.messages = vec![party_line(3, "Beta : heal please"), party_line(6, "Beta : out of potions")];
        play.hear(&obs);
        assert!(!play.wants_heal("Beta"));
        assert_eq!(play.potion_requests().into_iter().map(|(name, _)| name).collect::<Vec<_>>(), vec!["Beta".to_string()]);
        obs.messages.push(party_line(7, "Gamma : sending potions to beta"));
        play.hear(&obs);
        assert!(play.potion_requests().is_empty());
    }

    #[test]
    fn monsters_of_other_players_are_left_to_them() {
        let mut obs = observation();
        obs.players = vec![
            Player { id: 1, name: "Alpha".into(), party: true, ..Default::default() },
            Player { id: 9, name: "Stranger".into(), attack_target: Some(500), ..Default::default() },
        ];
        let members = member_ids(&obs);
        assert_eq!(members, vec![1, 2]);
        assert!(taken_by_others(&obs, 500, None, &members), "a stranger hits it");
        assert!(taken_by_others(&obs, 501, Some(9), &members), "it attacks a stranger");
        assert!(!taken_by_others(&obs, 502, Some(1), &members), "it attacks a member");
        obs.mobs = vec![
            Mob { id: 502, name: "Poring".into(), x: 104, y: 100, hp: 50, max_hp: 50, target: Some(1), ..Default::default() },
            Mob { id: 503, name: "Lunatic".into(), x: 101, y: 100, hp: 50, max_hp: 50, target: Some(9), ..Default::default() },
        ];
        assert_eq!(attacking_a_member(&obs, &members, SIGHT).map(|(id, _)| id), Some(502));
    }

    #[test]
    fn roles_lead_to_their_jobs() {
        assert_eq!(Role::Healer.jobs(), &["Acolyte", "Priest"]);
        assert_eq!(Role::Tank.stat_build(), Some("tank"));
        assert!(Role::Fighter.jobs().is_empty());
        assert!(config().is_friend("alpha"));
    }
}
