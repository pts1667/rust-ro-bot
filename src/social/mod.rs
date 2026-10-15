//! Talking with the people around, with only what a player has: the lines said, who is in sight, the party window.
//!
//! A bot answers a whisper, a party line that names it or asks something, and public chat that names it, comes from
//! someone it is talking with, or from the only player standing by. The LLM helper writes the answers, from the last
//! lines of the conversation and a few facts of the game reference about what was named. When someone asks for
//! something the bot can do (follow them, a party, potions, a hunt), the request goes to the strategist, which takes
//! it or not; the bot then says yes or no. Each person gets a few answers per ten minutes, so two bots never talk to
//! each other forever.
//!
//! Without the LLM the bot does not make small talk, but still takes requests said in plain words (`rules`).

pub mod facts;
pub mod help;
pub mod rules;

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::api::observation::Message;
use crate::api::{Action, Observation, dist};
use crate::bot::Bot;
use crate::goap::Goal;
use crate::goap::goals::HelpTask;
use crate::party::SIGHT;
use crate::party::chat::{self, Said};
use crate::strategist::helper::ChatContext;
use crate::voice::known::{Known, names_in_sight, party_of};

/// Lines kept of a conversation.
const TALK_LINES: usize = 8;
/// A conversation is forgotten after this long without a word.
const TALK_LASTS: Duration = Duration::from_secs(600);
/// Someone the bot answered this recently may go on talking without naming it.
const TALK_OPEN: Duration = Duration::from_secs(120);
/// Answers to one person per `BUDGET_SPAN`.
const BUDGET: usize = 6;
const BUDGET_SPAN: Duration = Duration::from_secs(600);
/// Shortest time between two answers.
const ANSWER_GAP: Duration = Duration::from_secs(3);
/// Lines heard and not answered yet are dropped after this long.
const HEARD_LASTS: Duration = Duration::from_secs(30);
const HEARD_KEPT: usize = 4;
/// A request the strategist did not decide on in this time is declined.
const REQUEST_LASTS: Duration = Duration::from_secs(90);
/// Public chat from someone this close, with nobody else in sight, is said to the bot.
const FACE_TO_FACE: u32 = 5;
/// Words that end a favour, said by the one the bot helps.
const STOP_WORDS: [&str; 6] = ["stop", "thanks", "thx", "ty", "bye", "stay"];
const STOP_PHRASES: [&str; 3] = ["thank you", "wait here", "that's enough"];

/// Where a line was said, and so where the answer goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Public,
    Whisper,
    Party,
}

impl Channel {
    pub fn label(self) -> &'static str {
        match self {
            Channel::Public => "public chat",
            Channel::Whisper => "whisper",
            Channel::Party => "party chat",
        }
    }
}

/// What someone asked the bot to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Help(HelpTask),
    /// Hunt on this map, with them.
    Hunt(String),
}

/// A request waiting for the strategist.
#[derive(Debug, Clone)]
pub struct Request {
    pub id: u64,
    pub from: String,
    pub channel: Channel,
    /// What they said.
    pub text: String,
    pub ask: Ask,
    pub at: Instant,
}

/// A line someone else said.
#[derive(Debug, Clone)]
pub struct Heard {
    pub from: String,
    pub text: String,
    pub channel: Channel,
    pub at: Instant,
}

#[derive(Debug, Default)]
struct Talk {
    /// (said by the bot, text), oldest first.
    lines: VecDeque<(bool, String)>,
    last: Option<Instant>,
    /// When the bot answered.
    answers: VecDeque<Instant>,
}

impl Talk {
    fn add(&mut self, mine: bool, text: &str) {
        self.lines.push_back((mine, text.to_owned()));
        while self.lines.len() > TALK_LINES {
            self.lines.pop_front();
        }
        self.last = Some(Instant::now());
    }

    fn answers_left(&self) -> bool {
        self.answers
            .iter()
            .filter(|at| at.elapsed() < BUDGET_SPAN)
            .count()
            < BUDGET
    }

    fn open(&self) -> bool {
        self.answers
            .back()
            .is_some_and(|at| at.elapsed() < TALK_OPEN)
    }
}

/// What the bot remembers of its conversations.
#[derive(Debug, Default)]
pub struct Social {
    last_seq: u64,
    /// By lowercase name.
    talks: HashMap<String, Talk>,
    heard: VecDeque<Heard>,
    last_answer: Option<Instant>,
    pub pending: Vec<Request>,
    next_id: u64,
    /// Someone who said a word that ends a favour, and when.
    pub stopped_by: Option<(String, Instant)>,
    /// When the one followed went out of sight.
    pub lost_since: Option<Instant>,
    /// Where the bot last saw each player in sight, by lowercase name, on its map.
    seen: HashMap<String, (String, (u16, u16))>,
    /// The members of the bot's party it knows of, lowercase: someone new is greeted.
    members: Vec<String>,
    /// Parties created to invite someone, for the name of the next try.
    pub create_tries: u32,
    pub last_create: Option<Instant>,
}

/// Whether `lower` says the favour is over.
fn ends_favour(lower: &str) -> bool {
    lower
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| STOP_WORDS.contains(&w))
        || STOP_PHRASES.iter().any(|p| lower.contains(p))
}

/// Whether `m` is someone talking to the bot: a whisper, or a line that names it.
pub fn calls_for_answer(m: &Message, me: &str) -> bool {
    let Some((from, text)) = m.text.split_once(" : ") else {
        return false;
    };
    !from.trim().eq_ignore_ascii_case(me)
        && match m.channel.as_str() {
            "whisper" => true,
            "chat" | "party" => names(text, me),
            _ => false,
        }
}

/// Whether `text` names `me` as a word.
fn names(text: &str, me: &str) -> bool {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|w| w.eq_ignore_ascii_case(me))
}

impl Social {
    /// Starts listening from the current messages on. The members of a party the bot is already in were greeted before.
    pub fn start(&mut self, obs: &Observation) {
        self.last_seq = obs.last_message_seq();
        self.members = party_members(obs);
    }

    /// What to say about who joined the party since the last call: hello from a bot that just joined one, a welcome
    /// from the leader to a newcomer.
    fn greeting(&mut self, obs: &Observation) -> Option<String> {
        let now = party_members(obs);
        let me = obs.me.name.to_lowercase();
        let before = std::mem::replace(&mut self.members, now.clone());
        let party = obs.party.as_ref()?;
        if now.len() < 2 {
            return None;
        }
        if !before.contains(&me) {
            return Some(hello(&obs.me.name).to_owned());
        }
        let leads = party
            .members
            .iter()
            .any(|m| m.leader && m.name.eq_ignore_ascii_case(&obs.me.name));
        let new: Vec<&str> = party
            .members
            .iter()
            .map(|m| m.name.as_str())
            .filter(|n| !before.contains(&n.to_lowercase()))
            .collect();
        (leads && !new.is_empty()).then(|| format!("welcome, {}!", new.join(" and ")))
    }

    /// Takes in what others said since the last call. The fixed party phrases are the party's business (see
    /// `party::chat`), and so is asking to join a party, on any channel.
    fn hear(&mut self, obs: &Observation) {
        for p in obs
            .players
            .iter()
            .filter(|p| dist((p.x, p.y), obs.pos()) <= SIGHT)
        {
            self.seen
                .insert(p.name.to_lowercase(), (obs.map.name.clone(), (p.x, p.y)));
        }
        let me = &obs.me.name;
        let last = self.last_seq;
        self.last_seq = last.max(obs.last_message_seq());
        for m in obs.messages.iter().filter(|m| m.seq > last) {
            let channel = match m.channel.as_str() {
                "chat" => Channel::Public,
                "whisper" => Channel::Whisper,
                "party" => Channel::Party,
                _ => continue,
            };
            let (Some(line), Some((_, text))) = (chat::parse(&m.text), m.text.split_once(" : "))
            else {
                continue;
            };
            if line.from.eq_ignore_ascii_case(me) {
                continue;
            }
            // NPCs talk on the public channel too: only players in sight count
            if channel == Channel::Public
                && !obs
                    .players
                    .iter()
                    .any(|p| p.name.eq_ignore_ascii_case(&line.from))
            {
                continue;
            }
            match line.said {
                Said::WantParty { .. } | Said::Sorry(_) => continue,
                Said::Other(_) => {}
                _ if channel == Channel::Party => continue,
                _ => {}
            }
            let text = text.trim().to_owned();
            if ends_favour(&text.to_lowercase()) {
                self.stopped_by = Some((line.from.clone(), Instant::now()));
            }
            self.talks
                .entry(line.from.to_lowercase())
                .or_default()
                .add(false, &text);
            self.heard.push_back(Heard {
                from: line.from,
                text,
                channel,
                at: Instant::now(),
            });
        }
        self.heard.retain(|h| h.at.elapsed() < HEARD_LASTS);
        while self.heard.len() > HEARD_KEPT {
            self.heard.pop_front();
        }
        self.talks
            .retain(|_, t| t.last.is_some_and(|at| at.elapsed() < TALK_LASTS));
    }

    /// Whether the bot should answer `h`: said to it, and the speaker has answers left.
    fn worth_answering(&self, h: &Heard, obs: &Observation) -> bool {
        let talk = self.talks.get(&h.from.to_lowercase());
        if talk.is_some_and(|t| !t.answers_left()) {
            return false;
        }
        let open = talk.is_some_and(Talk::open);
        let named = names(&h.text, &obs.me.name);
        match h.channel {
            Channel::Whisper => true,
            Channel::Party => named || open || h.text.contains('?'),
            Channel::Public => {
                let near = obs.player_near(&h.from, SIGHT);
                named
                    || (open && near.is_some())
                    || (obs.players.len() == 1
                        && near.is_some_and(|p| dist((p.x, p.y), obs.pos()) <= FACE_TO_FACE))
            }
        }
    }

    /// The next line to answer, taken out of those heard.
    fn next_to_answer(&mut self, obs: &Observation) -> Option<Heard> {
        if self.last_answer.is_some_and(|at| at.elapsed() < ANSWER_GAP) {
            return None;
        }
        let i = self
            .heard
            .iter()
            .position(|h| self.worth_answering(h, obs))?;
        let h = self.heard.remove(i)?;
        // Later lines of the same person are part of the same turn
        self.heard
            .retain(|o| !o.from.eq_ignore_ascii_case(&h.from) || o.channel != h.channel);
        Some(h)
    }

    /// The conversation with `name`, as lines for the prompt, oldest first.
    fn transcript(&self, name: &str) -> Vec<String> {
        self.talks
            .get(&name.to_lowercase())
            .map(|t| {
                t.lines
                    .iter()
                    .map(|(mine, text)| format!("{}: {text}", if *mine { "you" } else { name }))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn said(&mut self, to: &str, text: &str) {
        let talk = self.talks.entry(to.to_lowercase()).or_default();
        talk.add(true, text);
        talk.answers.push_back(Instant::now());
        while talk.answers.len() > BUDGET {
            talk.answers.pop_front();
        }
        self.last_answer = Some(Instant::now());
    }

    /// What the LLM is told when it answers `from`.
    fn context(
        &self,
        obs: &Observation,
        from: &str,
        channel: Channel,
        doing: &str,
        facts: Vec<String>,
        decision: Option<String>,
    ) -> ChatContext {
        let party = obs
            .party
            .as_ref()
            .map(|p| format!(", in the party {}", p.name))
            .unwrap_or_default();
        // Told "follow Tester", the model says "I follow Tester" to Tester: it reads "them"
        let doing = &them(doing, from);
        let decision = decision.map(|d| them(&d, from));
        ChatContext {
            me: obs.me.name.clone(),
            about_me: format!(
                "a level {}/{} {} on {}{party}, HP {:.0}%, {} zeny, now: {doing}",
                obs.me.base_level,
                obs.me.job_level,
                obs.me.job,
                obs.map.name,
                obs.me.hp_frac() * 100.0,
                obs.me.zeny
            ),
            from: from.to_owned(),
            channel: channel.label(),
            lines: self.transcript(from),
            facts,
            decision,
            known: Default::default(),
        }
    }

    /// Where the bot last saw `who` on the map it is on.
    pub fn last_seen(&self, who: &str, obs: &Observation) -> Option<(u16, u16)> {
        self.seen
            .get(&who.to_lowercase())
            .filter(|(map, _)| *map == obs.map.name)
            .map(|(_, pos)| *pos)
    }

    /// Whether `who` told the bot to stop since `since`.
    pub fn stopped(&self, who: &str, since: Instant) -> bool {
        self.stopped_by
            .as_ref()
            .is_some_and(|(name, at)| name.eq_ignore_ascii_case(who) && *at >= since)
    }
}

/// What the bot says when it agrees, without the LLM.
fn agree_text(ask: &Ask) -> String {
    match ask {
        Ask::Help(HelpTask::Follow) => "ok, right behind you".into(),
        Ask::Help(HelpTask::Invite) => "sure, inviting you".into(),
        Ask::Help(HelpTask::JoinParty) => "sure, invite me".into(),
        Ask::Help(HelpTask::GivePotions) => "ok, sending you some potions".into(),
        Ask::Hunt(map) => format!("ok, see you on {map}"),
    }
}

/// What doing `ask` means, for the LLM.
fn does(ask: &Ask, from: &str) -> String {
    match ask {
        Ask::Help(HelpTask::Follow) => format!("walk along with {from} for a few minutes"),
        Ask::Help(HelpTask::Invite) => format!("invite {from} to your party"),
        Ask::Help(HelpTask::JoinParty) => format!("take {from}'s party invitation"),
        Ask::Help(HelpTask::GivePotions) => format!("trade {from} a few potions"),
        Ask::Hunt(map) => format!("hunt on {map} with {from}"),
    }
}

/// `text` with the name `who` as "them".
fn them(text: &str, who: &str) -> String {
    text.split(' ')
        .map(|w| {
            let name = w.trim_end_matches(|c: char| !c.is_alphanumeric());
            if name.eq_ignore_ascii_case(who) {
                format!("them{}", &w[name.len()..])
            } else {
                w.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The members of the bot's party, lowercase; none outside a party.
fn party_members(obs: &Observation) -> Vec<String> {
    obs.party
        .as_ref()
        .map(|p| p.members.iter().map(|m| m.name.to_lowercase()).collect())
        .unwrap_or_default()
}

/// A hello for a new party, varied by the bot's name.
fn hello(me: &str) -> &'static str {
    const HELLOS: [&str; 4] = [
        "hi all!",
        "hello everyone",
        "hey, glad to join",
        "hi! let's go",
    ];
    HELLOS[me.bytes().map(usize::from).sum::<usize>() % HELLOS.len()]
}

impl Bot {
    /// Says `text` where `channel` says, to `to` for a whisper.
    pub async fn say_on(&mut self, channel: Channel, to: &str, text: &str) -> bool {
        let action = match channel {
            Channel::Public => Action::Say {
                text: text.to_owned(),
            },
            Channel::Whisper => Action::Whisper {
                to: to.to_owned(),
                text: text.to_owned(),
            },
            Channel::Party => Action::PartyChat {
                text: text.to_owned(),
            },
        };
        match self.act(action).await {
            Ok(_) => {
                tracing::info!(%to, %text, channel = channel.label(), "said");
                self.social.said(to, text);
                true
            }
            Err(error) => {
                tracing::debug!(%error, "chat refused");
                false
            }
        }
    }

    /// Hears what was said and answers one line: in words, or by taking a request to the strategist. Also says hello
    /// to a new party. Returns whether a request came in.
    pub async fn converse(&mut self, obs: &Observation, doing: &str) -> bool {
        self.talked_to
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.social.hear(obs);
        if let Some(text) = self.social.greeting(obs) {
            if self
                .act(Action::PartyChat { text: text.clone() })
                .await
                .is_ok()
            {
                tracing::info!(%text, "greeted the party");
            }
            return false;
        }
        let Some(h) = self.social.next_to_answer(obs) else {
            return false;
        };
        // The one the bot does a favour for tells it to stop: the favour ends, the bot says goodbye
        if ends_favour(&h.text.to_lowercase())
            && doing
                .to_lowercase()
                .split(' ')
                .any(|w| w == h.from.to_lowercase())
        {
            let decision = format!("you stop now, as they asked: you no longer {doing}.");
            let context =
                self.social
                    .context(obs, &h.from, h.channel, doing, Vec::new(), Some(decision));
            let reply = match self.helper.clone() {
                Some(helper) => helper
                    .chat(&context, false)
                    .await
                    .ok()
                    .and_then(|t| t.reply),
                None => None,
            };
            self.say_on(
                h.channel,
                &h.from,
                reply.as_deref().unwrap_or("ok, good luck!"),
            )
            .await;
            return false;
        }
        let (ask, reply) = match self.helper.clone() {
            Some(helper) => {
                let lore = facts::lore(&self.knowledge, &h.text).await;
                let facts: Vec<String> = lore.iter().map(facts::Lore::describe).collect();
                tracing::debug!(?facts, "facts for the answer");
                let mut context = self
                    .social
                    .context(obs, &h.from, h.channel, doing, facts, None);
                if helper.decides() {
                    context.known = self.known_for(obs, &h.from, doing, lore);
                }
                match helper.chat(&context, true).await {
                    Ok(turn)
                        if turn
                            .ask
                            .as_ref()
                            .is_some_and(|ask| !rules::plausible(ask, &h.text)) =>
                    {
                        tracing::debug!(ask = ?turn.ask, text = %h.text, "no such request in the words, answering only");
                        (
                            None,
                            helper
                                .chat(&context, false)
                                .await
                                .ok()
                                .and_then(|t| t.reply),
                        )
                    }
                    // The answer is written once the strategist decided
                    Ok(turn) if turn.ask.is_some() => (turn.ask, None),
                    Ok(turn) => (None, turn.reply),
                    Err(error) => {
                        tracing::debug!(%error, "no chat answer");
                        (rules::ask_of(&h.text), None)
                    }
                }
            }
            None => (rules::ask_of(&h.text), None),
        };
        let ask = match ask {
            // Only a map the reference knows
            Some(Ask::Hunt(map)) => self
                .knowledge
                .db
                .map(&map)
                .await
                .ok()
                .map(|_| Ask::Hunt(map)),
            other => other,
        };
        if let Some(ask) = ask {
            self.social.next_id += 1;
            let request = Request {
                id: self.social.next_id,
                from: h.from,
                channel: h.channel,
                text: h.text,
                ask,
                at: Instant::now(),
            };
            tracing::info!(from = %request.from, ask = ?request.ask, text = %request.text, "asked in chat");
            self.social.pending.push(request);
            return true;
        }
        if let Some(reply) = reply {
            self.say_on(h.channel, &h.from, &reply).await;
        }
        false
    }

    /// Answers the requests the strategist decided on: yes to the one it chose (or when its goal does what was asked),
    /// no to the others it saw, with the reason when the bot cannot. The LLM words it when there is one.
    pub async fn answer_requests(
        &mut self,
        obs: &Observation,
        asked: &[u64],
        chosen: &str,
        goal: Option<&Goal>,
        refusals: &[(u64, String)],
    ) {
        let (decided, waiting): (Vec<Request>, Vec<Request>) =
            std::mem::take(&mut self.social.pending)
                .into_iter()
                .partition(|r| asked.contains(&r.id));
        self.social.pending = waiting;
        let doing = goal
            .map(Goal::label)
            .unwrap_or_else(|| "something else".into());
        for r in decided {
            let same = match &r.ask {
                Ask::Hunt(map) => goal.is_some_and(|g| *g == Goal::Hunt { map: map.clone() }),
                Ask::Help(_) => false,
            };
            let yes = same || chosen == format!("ask:{}", r.id);
            let why = refusals
                .iter()
                .find(|(id, _)| *id == r.id)
                .map(|(_, why)| why.clone());
            let decision = match (yes, &why) {
                (true, _) => format!("yes, you will {}.", does(&r.ask, &r.from)),
                (false, Some(why)) => format!("no, because {why}."),
                (false, None) => format!("no, not now: you are busy ({doing})."),
            };
            let worded = match self.helper.clone() {
                Some(helper) => {
                    let context = self.social.context(
                        obs,
                        &r.from,
                        r.channel,
                        &doing,
                        Vec::new(),
                        Some(decision.clone()),
                    );
                    helper
                        .chat(&context, false)
                        .await
                        .ok()
                        .and_then(|t| t.reply)
                }
                None => None,
            };
            let text = worded.unwrap_or_else(|| match (yes, &why) {
                (true, _) => agree_text(&r.ask),
                (false, Some(why)) => format!(
                    "sorry, can't: {}",
                    why.replace("you are ", "I'm ")
                        .replace("you ", "I ")
                        .replace("your ", "my ")
                ),
                (false, None) => format!("sorry, can't right now ({doing})"),
            });
            tracing::info!(from = %r.from, ask = ?r.ask, yes, %decision, "answered a request");
            if yes {
                self.memory
                    .note(format!("agreed to help {}: {}", r.from, r.text));
            }
            self.say_on(r.channel, &r.from, &text).await;
        }
    }

    /// What the bot knows when it answers `from`, for the decision model.
    fn known_for(
        &self,
        obs: &Observation,
        from: &str,
        doing: &str,
        lore: Vec<facts::Lore>,
    ) -> Known {
        let sorted = |m: &std::collections::BTreeMap<String, u64>| {
            // Deaths are counted per map too: only monsters are told
            let mut v: Vec<(String, u64)> = m
                .iter()
                .filter(|(k, _)| !k.contains('_'))
                .map(|(k, n)| (k.clone(), *n))
                .collect();
            v.sort_by(|a, b| b.1.cmp(&a.1));
            v
        };
        let hunt_map = doing.strip_prefix("hunt on ").map(str::to_owned);
        Known {
            me: obs.me.name.clone(),
            level: obs.me.base_level,
            job_level: obs.me.job_level,
            job: obs.me.job.clone(),
            map: obs.map.name.clone(),
            doing: doing.to_owned(),
            hunting: hunt_map.as_deref() == Some(obs.map.name.as_str()),
            hunt_map,
            in_sight: names_in_sight(obs),
            kills: sorted(&self.memory.kills_by_mob),
            deaths: sorted(&self.memory.deaths_by_cause),
            losing_to: self.memory.losing_to(obs.me.base_level),
            kills_total: self.memory.counters.kills,
            deaths_total: self.memory.counters.deaths,
            journal: self.memory.journal.clone(),
            party: party_of(obs),
            low_potions: self.knowledge.heal_items(obs) < 3,
            other: Some(from.to_owned()),
            newcomers: Vec::new(),
            lore,
            notes: self.memory.strategy.notes.clone(),
            hp_frac: if obs.me.max_hp == 0 {
                0.0
            } else {
                obs.me.hp as f32 / obs.me.max_hp as f32
            },
            weight_frac: obs.me.weight as f32 / obs.me.max_weight.max(1) as f32,
            zeny: obs.me.zeny,
            skill_points: obs.me.skill_points,
            status_points: obs.me.status_points,
            job_target: self.memory.strategy.job_target.clone(),
            leader: obs.party.as_ref().and_then(|p| p.leader.clone()),
            can_heal: self.can_heal(obs),
        }
    }

    /// Declines the requests nobody decided on for too long.
    pub async fn decline_stale(&mut self, obs: &Observation, goal: Option<&Goal>) {
        let stale: Vec<u64> = self
            .social
            .pending
            .iter()
            .filter(|r| r.at.elapsed() >= REQUEST_LASTS)
            .map(|r| r.id)
            .collect();
        if !stale.is_empty() {
            self.answer_requests(obs, &stale, "", goal, &[]).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Message, Player};

    fn obs_with(players: &[(&str, u16)], lines: &[(&str, &str)]) -> Observation {
        let mut obs = Observation::default();
        obs.me.name = "Alfa".into();
        obs.me.x = 100;
        obs.me.y = 100;
        obs.players = players
            .iter()
            .enumerate()
            .map(|(i, (name, x))| Player {
                id: i as u64 + 10,
                name: (*name).into(),
                x: *x,
                y: 100,
                ..Default::default()
            })
            .collect();
        obs.messages = lines
            .iter()
            .enumerate()
            .map(|(i, (channel, text))| Message {
                seq: i as u64 + 1,
                channel: (*channel).into(),
                text: (*text).into(),
            })
            .collect();
        obs
    }

    fn answered(obs: &Observation) -> Vec<String> {
        let mut s = Social::default();
        s.hear(obs);
        let mut out = Vec::new();
        while let Some(h) = s.next_to_answer(obs) {
            out.push(h.text.clone());
            s.last_answer = None;
        }
        out
    }

    #[test]
    fn whispers_names_and_lone_neighbours_are_answered() {
        let obs = obs_with(
            &[("Tom", 103), ("Ann", 110)],
            &[
                ("chat", "Tom : nice weather"),
                ("chat", "Ann : hi Alfa"),
                ("whisper", "Bob : got a sec?"),
                ("chat", "Guard : Welcome to Prontera"),
                ("party", "Cid : follow me"),
                ("party", "Cid : what level are you?"),
                ("whisper", "Dee : party? level 12/9 Swordman"),
            ],
        );
        assert_eq!(
            answered(&obs),
            vec!["hi Alfa", "got a sec?", "what level are you?"]
        );
        // Alone with Tom, three cells away: he talks to the bot
        let obs = obs_with(&[("Tom", 103)], &[("chat", "Tom : nice weather")]);
        assert_eq!(answered(&obs), vec!["nice weather"]);
    }

    #[test]
    fn each_person_gets_a_few_answers_only() {
        let mut s = Social::default();
        let mut obs = obs_with(&[("Bot2", 103), ("Ann", 120)], &[]);
        for i in 0..20 {
            obs.messages.push(Message {
                seq: i + 1,
                channel: "chat".into(),
                text: format!("Bot2 : Alfa, line {i}"),
            });
            s.hear(&obs);
            if s.next_to_answer(&obs).is_some() {
                s.said("Bot2", "ok");
                s.last_answer = None;
            }
        }
        assert_eq!(s.talks["bot2"].answers.len(), BUDGET);
        assert_eq!(s.transcript("Bot2").len(), TALK_LINES);
    }

    #[test]
    fn newcomers_say_hello_and_the_leader_welcomes_them() {
        use crate::api::{Party, PartyMember};
        let party = |names: &[(&str, bool)]| Party {
            name: "P".into(),
            members: names
                .iter()
                .map(|(n, leader)| PartyMember {
                    name: (*n).into(),
                    leader: *leader,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let mut obs = obs_with(&[], &[]);
        let mut s = Social::default();
        obs.party = Some(party(&[("Alfa", true)]));
        s.start(&obs);
        assert_eq!(s.greeting(&obs), None, "alone");
        obs.party = Some(party(&[("Alfa", true), ("Tom", false)]));
        assert_eq!(s.greeting(&obs).as_deref(), Some("welcome, Tom!"));
        assert_eq!(s.greeting(&obs), None, "once");
        // Joining someone else's party
        let mut s = Social::default();
        obs.party = None;
        s.start(&obs);
        obs.party = Some(party(&[("Tom", true), ("Alfa", false)]));
        assert!(s.greeting(&obs).is_some_and(|h| !h.starts_with("welcome")));
    }

    #[test]
    fn the_one_answered_is_them() {
        assert_eq!(
            them(
                "yes, you will walk along with Tester for a few minutes.",
                "tester"
            ),
            "yes, you will walk along with them for a few minutes."
        );
        assert_eq!(them("invite Tester.", "Tester"), "invite them.");
        assert_eq!(them("hunt on prt_fild07", "Tester"), "hunt on prt_fild07");
    }

    #[test]
    fn a_stop_word_ends_a_favour() {
        let mut s = Social::default();
        let since = Instant::now();
        s.hear(&obs_with(
            &[("Tom", 103)],
            &[("whisper", "Tom : ok thanks, that's enough")],
        ));
        assert!(s.stopped("tom", since));
        assert!(!s.stopped("Ann", since));
    }
}
