//! Finding a party with only what a player has: the party booking board of the game, whispers, and what is in sight.
//!
//! A bot without a party looks at the board now and then and whispers whoever put up an ad that fits (levels close
//! enough to share experience), then takes the invitation that comes back. Finding none, a bot that can create a party
//! does, and puts up its own ad; it invites whoever whispers it with a fitting level, until the party is full. Two lone
//! leaders merge: the one with the newer ad gives its party up and asks the other. A member leaves when the leader has
//! been out of the game for long, or their levels drifted too far apart to share experience.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Result;

use super::chat::{self, Said};
use super::{SIGHT, leader_of};
use crate::api::{Action, BookingAd, Observation, Party, PartyInvitation};
use crate::bot::Bot;

/// Members a bot-made party takes.
pub const MAX_MEMBERS: usize = 4;
/// Most levels apart for a bot to party up with someone: under the 15 of shared experience, with room to level.
const LEVEL_GAP: u32 = 10;
/// How long a level someone gave is taken as theirs.
const LEVEL_HEARD_FOR: Duration = Duration::from_secs(30 * 60);
/// A member leaves once the leader is this many levels away: experience is no longer shared.
const LEAVE_LEVEL_GAP: u32 = 15;
const SEARCH_EVERY: Duration = Duration::from_secs(45);
/// A lone bot gives the board this long to answer before it creates its own party.
const RESULTS_WAIT: Duration = Duration::from_secs(4);
/// The same ad is not answered again before this.
const ASK_AGAIN: Duration = Duration::from_secs(300);
/// How long an asked leader has to send its invitation, the bot waits meanwhile.
const INVITE_WAIT: Duration = Duration::from_secs(30);
const CREATE_EVERY: Duration = Duration::from_secs(30);
const REGISTER_EVERY: Duration = Duration::from_secs(60);
/// A leader out of the game this long is given up.
const LEADER_GONE: Duration = Duration::from_secs(600);
const BASIC_SKILL_TO_CREATE: u32 = 7;
const PARTY_NAME_LEN: usize = 23;
/// A bot nobody answers says it looks for a party or members this often, when someone is in sight to hear it.
const SHOUT_EVERY: Duration = Duration::from_secs(180);

/// What a bot finding a party remembers.
#[derive(Debug, Default)]
pub struct Recruiting {
    last_search: Option<Instant>,
    /// Whom the bot asked to join, by lowercase name.
    asked: HashMap<String, Instant>,
    /// Who said no, by lowercase name.
    refused: HashMap<String, Instant>,
    last_whisper_seq: u64,
    last_register: Option<Instant>,
    last_create: Option<Instant>,
    create_tries: u32,
    leader_gone_since: Option<Instant>,
    last_shout: Option<Instant>,
    /// Since when the bot leads a party of one.
    alone_since: Option<Instant>,
    /// Base levels others gave of themselves (a booking ad, "party? level 12/9 Swordman"), by lowercase name. A game
    /// client shows levels only for party members, so these are all the bot knows of a stranger's.
    heard_levels: HashMap<String, (u32, Instant)>,
}

impl Recruiting {
    pub fn new(last_seq: u64) -> Self {
        Self {
            last_whisper_seq: last_seq,
            ..Default::default()
        }
    }

    fn recently(map: &HashMap<String, Instant>, name: &str, within: Duration) -> bool {
        map.get(&name.to_lowercase())
            .is_some_and(|at| at.elapsed() < within)
    }

    /// Someone said they would invite the bot: their invitation is taken.
    pub fn expect_invitation(&mut self, name: &str) {
        self.asked.insert(name.to_lowercase(), Instant::now());
        self.refused.remove(&name.to_lowercase());
    }

    /// Whether the bot asked `name` to join a while ago and waits for its invitation.
    pub fn waits_for(&self, name: &str) -> bool {
        Self::recently(&self.asked, name, ASK_AGAIN)
            && !Self::recently(&self.refused, name, ASK_AGAIN)
    }

    fn waiting(&self) -> bool {
        self.asked.iter().any(|(name, at)| {
            at.elapsed() < INVITE_WAIT && !Self::recently(&self.refused, name, ASK_AGAIN)
        })
    }

    /// An ad worth answering: someone else's, close in level, open to the bot's job, not asked or refused lately.
    pub fn fits(&self, ad: &BookingAd, me: &str, level: u32, job: &str) -> bool {
        !ad.name.eq_ignore_ascii_case(me)
            && level.abs_diff(ad.level.max(0) as u32) <= LEVEL_GAP
            && (ad.jobs.is_empty() || ad.jobs.iter().any(|j| super::same_job(j, job)))
            && !Self::recently(&self.asked, &ad.name, ASK_AGAIN)
            && !Self::recently(&self.refused, &ad.name, ASK_AGAIN)
    }

    /// The whispers since the last call, read as party chat.
    fn whispers(&mut self, obs: &Observation) -> Vec<chat::Line> {
        let me = obs.me.name.clone();
        let last = self.last_whisper_seq;
        self.last_whisper_seq = last.max(obs.last_message_seq());
        obs.messages
            .iter()
            .filter(|m| m.seq > last && m.channel == "whisper")
            .filter_map(|m| chat::parse(&m.text))
            .filter(|line| !line.from.eq_ignore_ascii_case(&me))
            .collect()
    }

    /// Players in sight outside the party who said in public they look for one ("lf party, level 12/9 Swordman"),
    /// in the messages after `last`.
    fn shouts(&self, obs: &Observation, last: u64) -> Vec<chat::Line> {
        obs.messages
            .iter()
            .filter(|m| m.seq > last && m.channel == "chat")
            .filter_map(|m| chat::parse(&m.text))
            .filter(|line| {
                matches!(line.said, Said::WantParty { .. })
                    && obs.player_near(&line.from, SIGHT).is_some_and(|p| !p.party)
            })
            .filter(|line| !line.from.eq_ignore_ascii_case(&obs.me.name))
            .collect()
    }

    /// Whether to say aloud that the bot looks for a party: someone outside it is in sight, and it was not said lately.
    fn may_shout(&self, obs: &Observation) -> bool {
        obs.players.iter().any(|p| !p.party)
            && self.last_shout.is_none_or(|at| at.elapsed() >= SHOUT_EVERY)
    }

    /// Whether to take a party invitation, outside a party: from someone the bot asked, or from someone in sight who
    /// said a level close enough to the bot's.
    pub fn takes(&self, obs: &Observation, invitation: &PartyInvitation) -> bool {
        let inviter = &invitation.inviter_name;
        self.waits_for(inviter)
            || obs.player_near(inviter, SIGHT).is_some()
                && self
                    .heard_level(inviter)
                    .is_some_and(|level| level.abs_diff(obs.me.base_level) <= LEVEL_GAP)
    }

    /// Notes the base level someone gave of themselves.
    pub fn hear_level(&mut self, name: &str, level: u32) {
        if level > 0 {
            self.heard_levels
                .insert(name.to_lowercase(), (level, Instant::now()));
        }
    }

    /// The level `name` gave lately, if any.
    fn heard_level(&self, name: &str) -> Option<u32> {
        self.heard_levels
            .get(&name.to_lowercase())
            .filter(|(_, at)| at.elapsed() < LEVEL_HEARD_FOR)
            .map(|(level, _)| *level)
    }
}

/// The name of the party a bot creates, another one after a refused try (the name may be taken).
pub(crate) fn party_name(me: &str, tries: u32) -> String {
    let name = if tries == 0 {
        format!("{me}'s party")
    } else {
        format!("{me}'s party {tries}")
    };
    name.chars().take(PARTY_NAME_LEN).collect()
}

impl Bot {
    fn recruiting(&mut self) -> &mut Recruiting {
        &mut self.party.as_mut().expect("party play").recruiting
    }

    /// Finding a party, or members for the bot's own. Returns whether it acted.
    pub(super) async fn recruit(&mut self, obs: &Observation) -> Result<bool> {
        let last = self.recruiting().last_whisper_seq;
        let shouts = self.recruiting().shouts(obs, last);
        let whispers = self.recruiting().whispers(obs);
        for line in shouts.iter().chain(&whispers) {
            if let Said::WantParty { base, .. } = line.said {
                self.recruiting().hear_level(&line.from, base);
            }
        }
        if let Some(results) = &obs.booking_results {
            for ad in &results.ads {
                self.recruiting()
                    .hear_level(&ad.name, ad.level.max(0) as u32);
            }
        }
        for line in &whispers {
            if let Said::Sorry(reason) = &line.said {
                tracing::info!(from = %line.from, %reason, "not taken in the party");
                self.recruiting()
                    .refused
                    .insert(line.from.to_lowercase(), Instant::now());
            }
        }
        match &obs.party {
            None => self.look_for_party(obs).await,
            Some(party)
                if leader_of(party).is_some_and(|l| l.name.eq_ignore_ascii_case(&obs.me.name)) =>
            {
                self.gather_members(obs, party, &whispers, &shouts).await
            }
            Some(party) => self.stay_or_leave(obs, party).await,
        }
    }

    async fn look_for_party(&mut self, obs: &Observation) -> Result<bool> {
        if obs.booking.is_some() {
            // An ad of a party the bot no longer has
            self.act(Action::BookingDelete).await.ok();
            return Ok(true);
        }
        let r = self.recruiting();
        if r.waiting() {
            return Ok(false);
        }
        let searched = r.last_search.map(|at| at.elapsed());
        if let (Some(since), Some(results)) = (searched, &obs.booking_results)
            && since < SEARCH_EVERY
            && let Some(ad) = results
                .ads
                .iter()
                .find(|ad| r.fits(ad, &obs.me.name, obs.me.base_level, &obs.me.job))
        {
            let to = ad.name.clone();
            r.asked.insert(to.to_lowercase(), Instant::now());
            let said = Said::WantParty {
                base: obs.me.base_level,
                job: obs.me.job_level,
                job_name: obs.me.job.clone(),
            };
            tracing::info!(%to, level = ad.level, "asking to join a party");
            self.act(Action::Whisper {
                to,
                text: said.text(),
            })
            .await
            .ok();
            return Ok(true);
        }
        if searched.is_none_or(|since| since >= SEARCH_EVERY) {
            r.last_search = Some(Instant::now());
            self.act(Action::BookingSearch).await.ok();
            return Ok(true);
        }
        // Nobody to join and no party of its own possible yet: say so to whoever is around
        if searched.is_some_and(|since| since >= RESULTS_WAIT)
            && obs.skill_level("NV_BASIC") < BASIC_SKILL_TO_CREATE
            && r.may_shout(obs)
        {
            r.last_shout = Some(Instant::now());
            let text = format!(
                "lf party, level {}/{} {}",
                obs.me.base_level, obs.me.job_level, obs.me.job
            );
            tracing::info!(%text, "looking for a party aloud");
            self.act(Action::Say { text }).await.ok();
            return Ok(true);
        }
        // Nobody to join: start a party, when the bot can
        if searched.is_some_and(|since| since >= RESULTS_WAIT)
            && obs.skill_level("NV_BASIC") >= BASIC_SKILL_TO_CREATE
            && r.last_create.is_none_or(|at| at.elapsed() >= CREATE_EVERY)
        {
            r.last_create = Some(Instant::now());
            let name = party_name(&obs.me.name, r.create_tries);
            r.create_tries += 1;
            tracing::info!(%name, "nobody to join, creating a party");
            self.act(Action::PartyCreate {
                name,
                item_pickup: true,
                item_share: false,
            })
            .await
            .ok();
            return Ok(true);
        }
        Ok(false)
    }

    async fn gather_members(
        &mut self,
        obs: &Observation,
        party: &Party,
        whispers: &[chat::Line],
        shouts: &[chat::Line],
    ) -> Result<bool> {
        let full = party.members.len() >= MAX_MEMBERS;
        // Someone in sight looking for a party, close in level: invited without a word
        for line in shouts {
            let Said::WantParty { base, .. } = &line.said else {
                continue;
            };
            let member = party
                .members
                .iter()
                .any(|m| m.name.eq_ignore_ascii_case(&line.from));
            if full || member || (*base > 0 && base.abs_diff(obs.me.base_level) > LEVEL_GAP) {
                continue;
            }
            tracing::info!(from = %line.from, level = base, "invited someone looking for a party");
            self.act(Action::PartyInvite {
                name: line.from.clone(),
            })
            .await
            .ok();
            return Ok(true);
        }
        for line in whispers {
            let Said::WantParty { base, .. } = &line.said else {
                continue;
            };
            if party
                .members
                .iter()
                .any(|m| m.name.eq_ignore_ascii_case(&line.from))
            {
                continue;
            }
            let refusal = if full {
                Some("the party is full")
            } else if *base > 0 && base.abs_diff(obs.me.base_level) > LEVEL_GAP {
                Some("our levels are too far apart")
            } else {
                None
            };
            let action = match refusal {
                Some(reason) => Action::Whisper {
                    to: line.from.clone(),
                    text: Said::Sorry(reason.into()).text(),
                },
                None => Action::PartyInvite {
                    name: line.from.clone(),
                },
            };
            tracing::info!(from = %line.from, level = base, invited = refusal.is_none(), "asked to join the party");
            self.act(action).await.ok();
            return Ok(true);
        }
        let r = self.recruiting();
        if party.members.len() == 1 {
            let alone = *r.alone_since.get_or_insert_with(Instant::now);
            // Nobody came for a while: tell whoever is around
            if alone.elapsed() >= SHOUT_EVERY && r.may_shout(obs) {
                r.last_shout = Some(Instant::now());
                let text = format!(
                    "lf members, level {}/{} {}, whisper me",
                    obs.me.base_level, obs.me.job_level, obs.me.job
                );
                tracing::info!(%text, "looking for members aloud");
                self.act(Action::Say { text }).await.ok();
                return Ok(true);
            }
        } else {
            r.alone_since = None;
        }
        // Alone with an ad: join an older party that fits rather than wait
        if party.members.len() == 1 {
            let r = self.recruiting();
            let searched = r.last_search.map(|at| at.elapsed());
            let mine = obs.booking.as_ref().map(|ad| ad.index);
            if let (Some(since), Some(results), Some(mine)) = (searched, &obs.booking_results, mine)
                && since < SEARCH_EVERY
                && let Some(ad) = results.ads.iter().find(|ad| {
                    ad.index < mine && r.fits(ad, &obs.me.name, obs.me.base_level, &obs.me.job)
                })
            {
                let to = ad.name.clone();
                r.asked.insert(to.to_lowercase(), Instant::now());
                tracing::info!(%to, "alone in the party, joining an older one");
                self.act(Action::BookingDelete).await.ok();
                self.act(Action::PartyLeave).await.ok();
                let said = Said::WantParty {
                    base: obs.me.base_level,
                    job: obs.me.job_level,
                    job_name: obs.me.job.clone(),
                };
                self.act(Action::Whisper {
                    to,
                    text: said.text(),
                })
                .await
                .ok();
                return Ok(true);
            }
            if searched.is_none_or(|since| since >= SEARCH_EVERY) && obs.booking.is_some() {
                r.last_search = Some(Instant::now());
                self.act(Action::BookingSearch).await.ok();
                return Ok(true);
            }
        }
        let r = self.recruiting();
        if !full
            && obs.booking.is_none()
            && r.last_register
                .is_none_or(|at| at.elapsed() >= REGISTER_EVERY)
        {
            r.last_register = Some(Instant::now());
            tracing::info!(
                members = party.members.len(),
                "looking for members on the booking board"
            );
            self.act(Action::BookingRegister).await.ok();
            return Ok(true);
        }
        if full && obs.booking.is_some() {
            self.act(Action::BookingDelete).await.ok();
            return Ok(true);
        }
        Ok(false)
    }

    async fn stay_or_leave(&mut self, obs: &Observation, party: &Party) -> Result<bool> {
        if obs.booking.is_some() {
            self.act(Action::BookingDelete).await.ok();
            return Ok(true);
        }
        let Some(leader) = leader_of(party) else {
            return Ok(false);
        };
        let r = self.recruiting();
        let gone = if leader.online {
            r.leader_gone_since = None;
            false
        } else {
            r.leader_gone_since
                .get_or_insert_with(Instant::now)
                .elapsed()
                >= LEADER_GONE
        };
        let apart = leader
            .base_level
            .is_some_and(|level| level.abs_diff(obs.me.base_level) > LEAVE_LEVEL_GAP);
        if !gone && !apart {
            return Ok(false);
        }
        let reason = if gone {
            "the leader is gone"
        } else {
            "our levels are too far apart now"
        };
        tracing::info!(%reason, "leaving the party");
        self.say_to_party(Said::Other(format!("bye, {reason}")))
            .await;
        self.act(Action::PartyLeave).await.ok();
        r_reset(self);
        Ok(true)
    }
}

/// Starts the search over, once out of a party.
fn r_reset(bot: &mut Bot) {
    let r = bot.recruiting();
    r.leader_gone_since = None;
    r.last_search = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Player;

    fn ad(index: u64, name: &str, level: i32, jobs: &[&str]) -> BookingAd {
        BookingAd {
            index,
            name: name.into(),
            level,
            map_id: 0,
            jobs: jobs.iter().map(|j| (*j).to_owned()).collect(),
        }
    }

    #[test]
    fn an_ad_fits_when_levels_are_close_and_the_job_is_wanted() {
        let mut r = Recruiting::new(0);
        assert!(r.fits(&ad(1, "Alfa", 15, &[]), "Bravo", 12, "Novice"));
        assert!(!r.fits(&ad(1, "Bravo", 12, &[]), "Bravo", 12, "Novice"));
        assert!(!r.fits(&ad(1, "Alfa", 30, &[]), "Bravo", 12, "Novice"));
        assert!(r.fits(&ad(1, "Alfa", 12, &["Swordsman"]), "Bravo", 12, "Swordman"));
        assert!(!r.fits(&ad(1, "Alfa", 12, &["Acolyte"]), "Bravo", 12, "Swordman"));
        r.asked.insert("alfa".into(), Instant::now());
        assert!(!r.fits(&ad(1, "Alfa", 15, &[]), "Bravo", 12, "Novice"));
    }

    #[test]
    fn invitations_come_from_whom_the_bot_asked_or_from_someone_close_in_level() {
        let mut r = Recruiting::new(0);
        let mut obs = Observation::default();
        obs.me.base_level = 12;
        let invitation = |name: &str| PartyInvitation {
            party_id: 1,
            inviter: 2,
            inviter_name: name.into(),
        };
        assert!(!r.takes(&obs, &invitation("Alfa")));
        r.asked.insert("alfa".into(), Instant::now());
        assert!(r.takes(&obs, &invitation("Alfa")));
        r.refused.insert("alfa".into(), Instant::now());
        assert!(!r.takes(&obs, &invitation("Alfa")));
        // A stranger in sight: their level is not shown, only what they said of it counts
        obs.players = vec![Player {
            id: 3,
            name: "Gus".into(),
            base_level: 18,
            ..Default::default()
        }];
        assert!(!r.takes(&obs, &invitation("Gus")));
        r.hear_level("Gus", 18);
        assert!(r.takes(&obs, &invitation("Gus")));
        r.hear_level("Gus", 40);
        assert!(!r.takes(&obs, &invitation("Gus")));
    }

    #[test]
    fn party_names_fit_the_window() {
        assert_eq!(party_name("Alfa", 0), "Alfa's party");
        assert_eq!(party_name("Alfa", 2), "Alfa's party 2");
        assert_eq!(
            party_name("AVeryLongCharacterName", 0).chars().count(),
            PARTY_NAME_LEN
        );
    }
}
