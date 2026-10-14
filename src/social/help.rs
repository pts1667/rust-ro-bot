//! Doing what someone asked, once the strategist agreed: a `Goal::Help`, run step by step until it is over.

use std::time::{Duration, Instant};

use anyhow::Result;

use super::Channel;
use crate::api::{Action, Observation, dist};
use crate::bot::{Bot, Outcome};
use crate::goap::goals::HelpTask;
use crate::goap::state::HP_LOW;
use crate::party::recruit::{MAX_MEMBERS, party_name};
use crate::party::support::SPARE_ABOVE;
use crate::party::{SIGHT, attacking_a_member};

/// How long the bot walks along with someone.
const FOLLOW_FOR: Duration = Duration::from_secs(300);
/// The one followed is given up after this long out of sight.
const LOST_AFTER: Duration = Duration::from_secs(30);
/// The bot keeps this close to the one it follows.
const KEEP_WITHIN: u32 = 3;
const BASIC_SKILL_TO_CREATE: u32 = 7;
const CREATE_TRIES: u32 = 3;
const CREATE_WAIT: Duration = Duration::from_secs(5);

pub enum HelpOutcome {
    Going,
    Finished,
    Failed(String),
}

impl Bot {
    /// One step of helping `who` with `task`, asked for since `since`.
    pub async fn help(
        &mut self,
        obs: &Observation,
        who: &str,
        task: HelpTask,
        since: Instant,
    ) -> Result<HelpOutcome> {
        let member = obs
            .party
            .as_ref()
            .is_some_and(|p| p.members.iter().any(|m| m.name.eq_ignore_ascii_case(who)));
        match task {
            HelpTask::Follow if member && self.party.is_some() => {
                // The party already knows how to follow a member
                self.party.as_mut().expect("checked").follow_member(who);
                Ok(HelpOutcome::Finished)
            }
            HelpTask::Follow => self.walk_along(obs, who, since).await,
            HelpTask::Invite => self.invite(obs, who).await,
            HelpTask::JoinParty => Ok(self.join_party_of(obs, who).await),
            HelpTask::GivePotions => {
                let Some(player) = obs.player_near(who, SIGHT) else {
                    return Ok(HelpOutcome::Failed(format!("{who} is out of sight")));
                };
                if self.knowledge.heal_items(obs) <= SPARE_ABOVE {
                    return Ok(HelpOutcome::Failed("no potions to spare".into()));
                }
                Ok(match self.give_potions(player.id, who).await? {
                    Outcome::Failed(reason) => HelpOutcome::Failed(reason),
                    _ => HelpOutcome::Finished,
                })
            }
        }
    }

    /// Walks along with someone outside the party, fighting what attacks either, until told to stop or for a while.
    async fn walk_along(
        &mut self,
        obs: &Observation,
        who: &str,
        since: Instant,
    ) -> Result<HelpOutcome> {
        if self.social.stopped(who, since) {
            return Ok(HelpOutcome::Finished);
        }
        if since.elapsed() >= FOLLOW_FOR {
            if obs.player_near(who, SIGHT).is_some() {
                self.say_on(Channel::Public, who, "I have to go now, good luck!")
                    .await;
            }
            return Ok(HelpOutcome::Finished);
        }
        let Some(player) = obs.player_near(who, SIGHT).cloned() else {
            let lost = *self.social.lost_since.get_or_insert_with(Instant::now);
            if lost < since {
                // Out of sight since before: a new favour starts the count again
                self.social.lost_since = Some(Instant::now());
            } else if lost.elapsed() >= LOST_AFTER {
                self.social.lost_since = None;
                return Ok(HelpOutcome::Failed(format!("lost sight of {who}")));
            }
            // Where they were last seen, they may be in sight again
            match self.social.last_seen(who, obs) {
                Some(there) if dist(there, obs.pos()) > KEEP_WITHIN => {
                    self.walk_next_to(obs, there).await?;
                }
                _ => Bot::pause(500).await,
            }
            return Ok(HelpOutcome::Going);
        };
        self.social.lost_since = None;
        if obs.me.hp_frac() < HP_LOW && obs.me.action != "attacking" {
            self.rest().await?;
            return Ok(HelpOutcome::Going);
        }
        if let Some((id, name)) = attacking_a_member(obs, &[obs.me.char_id, player.id], SIGHT) {
            self.fight(id, &name).await?;
            return Ok(HelpOutcome::Going);
        }
        if dist((player.x, player.y), obs.pos()) > KEEP_WITHIN {
            self.walk_next_to(obs, (player.x, player.y)).await?;
        } else {
            Bot::pause(600).await;
        }
        Ok(HelpOutcome::Going)
    }

    /// Invites `who` to the bot's party, creating one first when the bot has none.
    async fn invite(&mut self, obs: &Observation, who: &str) -> Result<HelpOutcome> {
        let Some(party) = &obs.party else {
            if obs.skill_level("NV_BASIC") < BASIC_SKILL_TO_CREATE {
                return Ok(HelpOutcome::Failed("cannot create a party yet".into()));
            }
            if self
                .social
                .last_create
                .is_some_and(|at| at.elapsed() < CREATE_WAIT)
            {
                Bot::pause(500).await;
                return Ok(HelpOutcome::Going);
            }
            if self.social.create_tries >= CREATE_TRIES {
                return Ok(HelpOutcome::Failed("could not create a party".into()));
            }
            let name = party_name(&obs.me.name, self.social.create_tries);
            self.social.create_tries += 1;
            self.social.last_create = Some(Instant::now());
            tracing::info!(%name, %who, "creating a party to invite someone");
            self.act(Action::PartyCreate {
                name,
                item_pickup: true,
                item_share: false,
            })
            .await
            .ok();
            return Ok(HelpOutcome::Going);
        };
        if party
            .members
            .iter()
            .any(|m| m.name.eq_ignore_ascii_case(who))
        {
            return Ok(HelpOutcome::Finished);
        }
        if !party
            .members
            .iter()
            .any(|m| m.leader && m.name.eq_ignore_ascii_case(&obs.me.name))
        {
            return Ok(HelpOutcome::Failed("only the leader invites".into()));
        }
        if party.members.len() >= MAX_MEMBERS {
            return Ok(HelpOutcome::Failed("the party is full".into()));
        }
        self.social.create_tries = 0;
        Ok(
            match self
                .act(Action::PartyInvite {
                    name: who.to_owned(),
                })
                .await
            {
                Ok(_) => {
                    tracing::info!(%who, "invited to the party, as asked");
                    HelpOutcome::Finished
                }
                Err(error) => HelpOutcome::Failed(format!("cannot invite {who}: {error}")),
            },
        )
    }

    /// Gets ready to take the invitation of `who`: out of a party of one, then waiting for it.
    async fn join_party_of(&mut self, obs: &Observation, who: &str) -> HelpOutcome {
        match &obs.party {
            Some(p) if p.members.iter().any(|m| m.name.eq_ignore_ascii_case(who)) => {
                HelpOutcome::Finished
            }
            Some(p) if p.members.len() == 1 => {
                if obs.booking.is_some() {
                    self.act(Action::BookingDelete).await.ok();
                }
                tracing::info!(%who, "leaving a party of one to join someone");
                self.act(Action::PartyLeave).await.ok();
                HelpOutcome::Going
            }
            Some(_) => HelpOutcome::Failed("already in a party".into()),
            None => match self.party.as_mut() {
                Some(play) => {
                    play.recruiting.expect_invitation(who);
                    HelpOutcome::Finished
                }
                None => HelpOutcome::Failed("playing alone".into()),
            },
        }
    }
}
