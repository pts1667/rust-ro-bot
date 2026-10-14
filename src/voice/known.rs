//! What the bot knows, and the sentences of the catalogue it may say with it: each template is offered only with values
//! the bot has (a monster it saw, a map it hunted on, what the reference says about a name someone said), so whatever
//! the model picks is true.

use super::phrase::{Category, Phrase};
use crate::social::facts::Lore;
use crate::strategist::Snapshot;
use crate::strategist::skill_state::job_path;

/// Sentences offered per category, at most.
const PER_CATEGORY: usize = 40;
/// A monster this many levels from the bot's is "about my level", this many above "way above me".
const SAME_LEVELS: u32 = 3;
const ABOVE_LEVELS: u32 = 10;
/// HP share under which the bot is low on HP, weight share from which it is too heavy (no more regeneration).
const LOW_HP: f32 = 0.4;
const HEAVY: f32 = 0.5;
/// Spawns of a monster on a map that make "lots of" it there.
const LOTS: u32 = 20;
/// Kills of a monster without a death to it that make it easy.
const EASY_KILLS: u64 = 20;
/// A lesson about a hunting spot is for bots within this many levels of the one who wrote it.
const SPOT_LEVELS: u32 = 5;
/// A monster another player warns about is left alone for this many levels.
const CAUTION_LEVELS: u32 = 5;

#[derive(Debug, Clone, Default)]
pub struct Known {
    pub me: String,
    pub level: u32,
    pub job_level: u32,
    pub job: String,
    pub map: String,
    /// What the bot does now, in a few words.
    pub doing: String,
    /// The map the bot hunts on or heads for.
    pub hunt_map: Option<String>,
    /// The bot is on `hunt_map`.
    pub hunting: bool,
    /// Monsters in sight, by name.
    pub in_sight: Vec<String>,
    /// Kills and deaths per monster, most first.
    pub kills: Vec<(String, u64)>,
    pub deaths: Vec<(String, u64)>,
    /// Monsters that keep killing the bot at its level.
    pub losing_to: Vec<String>,
    /// All kills and deaths.
    pub kills_total: u64,
    pub deaths_total: u64,
    pub journal: Vec<String>,
    /// The other members of the bot's party; `None` out of a party.
    pub party: Option<Vec<String>>,
    pub low_potions: bool,
    /// Who the bot talks to.
    pub other: Option<String>,
    /// New members of the bot's party.
    pub newcomers: Vec<String>,
    /// What the reference says about the names in what the bot read or heard.
    pub lore: Vec<Lore>,
    pub notes: Vec<String>,
    /// HP and weight, as shares of their maximum.
    pub hp_frac: f32,
    pub weight_frac: f32,
    pub zeny: u64,
    pub skill_points: u32,
    pub status_points: u32,
    /// The job the strategist aims for (Σ).
    pub job_target: Option<String>,
    /// The leader of the bot's party.
    pub leader: Option<String>,
    /// The bot heals its party (the healer's role, as an Acolyte or Priest).
    pub can_heal: bool,
}

impl Known {
    pub fn from_snapshot(snap: &Snapshot) -> Self {
        let me = &snap.obs.me;
        let hunt_map = match &snap.goal {
            Some(crate::goap::Goal::Hunt { map }) => Some(map.clone()),
            _ => None,
        };
        Known {
            me: me.name.clone(),
            level: me.base_level,
            job_level: me.job_level,
            job: me.job.clone(),
            map: snap.obs.map.name.clone(),
            doing: snap.goal.as_ref().map(|g| g.label()).unwrap_or_default(),
            hunting: hunt_map.as_deref() == Some(snap.obs.map.name.as_str()),
            hunt_map,
            in_sight: names_in_sight(&snap.obs),
            kills: snap.kills_by_mob.clone(),
            // Deaths are counted per map too: only monsters are told
            deaths: snap
                .deaths_by_cause
                .iter()
                .filter(|(k, _)| !k.contains('_'))
                .cloned()
                .collect(),
            losing_to: snap.losing_to.clone(),
            kills_total: snap.kills,
            deaths_total: snap.deaths,
            journal: snap.journal.clone(),
            party: party_of(&snap.obs),
            low_potions: snap.heal_items < 3,
            notes: snap.notes.clone(),
            hp_frac: frac(me.hp, me.max_hp),
            weight_frac: snap.weight_frac,
            zeny: me.zeny,
            skill_points: me.skill_points,
            status_points: me.status_points,
            job_target: snap.job_target.clone(),
            leader: snap.obs.party.as_ref().and_then(|p| p.leader.clone()),
            can_heal: snap.can_heal,
            ..Default::default()
        }
    }

    /// The bot in a line, for the decision model's state.
    pub fn about(&self) -> String {
        let doing = if self.doing.is_empty() {
            String::new()
        } else {
            format!(", now: {}", self.doing)
        };
        let job_level = if self.job_level > 0 {
            format!(" (job level {})", self.job_level)
        } else {
            String::new()
        };
        format!(
            "{} is a level {} {}{job_level} on {}{doing}.",
            self.me, self.level, self.job, self.map
        )
    }

    /// What the bot has done, for the decision model's state.
    pub fn experience(&self) -> String {
        let list = |l: &[(String, u64)]| {
            l.iter()
                .take(5)
                .map(|(k, n)| format!("{k} {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut lines = Vec::new();
        if !self.kills.is_empty() {
            lines.push(format!("Killed: {}.", list(&self.kills)));
        }
        if !self.deaths.is_empty() {
            lines.push(format!("Killed by: {}.", list(&self.deaths)));
        }
        if !self.in_sight.is_empty() {
            lines.push(format!("Monsters in sight: {}.", self.in_sight.join(", ")));
        }
        lines.push(format!(
            "HP {:.0}%, weight {:.0}%, {} zeny, {} skill points and {} stat points to spend{}.",
            self.hp_frac * 100.0,
            self.weight_frac * 100.0,
            self.zeny,
            self.skill_points,
            self.status_points,
            if self.low_potions {
                ", low on potions"
            } else {
                ""
            }
        ));
        if let Some(job) = &self.job_target {
            lines.push(format!("Job target: {job}."));
        }
        if let Some(members) = &self.party {
            let leader = self
                .leader
                .as_deref()
                .map(|l| format!(" (leader: {l})"))
                .unwrap_or_default();
            lines.push(format!("Party: {}{leader}.", members.join(", ")));
        }
        lines.extend(
            self.journal
                .iter()
                .rev()
                .take(5)
                .rev()
                .map(|l| format!("Lately: {l}.")),
        );
        lines.extend(self.lore.iter().map(Lore::describe));
        lines.join("\n")
    }

    pub fn killed(&self, monster: &str) -> u64 {
        self.kills
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(monster))
            .map(|(_, n)| *n)
            .unwrap_or(0)
    }

    pub fn died_to(&self, monster: &str) -> u64 {
        self.deaths
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(monster))
            .map(|(_, n)| *n)
            .unwrap_or(0)
    }

    /// Every sentence the bot may say, at most `PER_CATEGORY` of each category.
    pub fn candidates(&self) -> Vec<Phrase> {
        let mut out: Vec<Phrase> = Vec::new();
        let p = Phrase::new;
        let level = self.level.to_string();
        // Greeting, reply and farewell: what anyone may say
        out.push(p("hello", &[]));
        out.push(p("hey_all", &[]));
        if let Some(other) = &self.other {
            out.push(p("hi_player", &[other]));
            out.push(p("good_to_see", &[other]));
        }
        out.extend(self.newcomers.iter().map(|n| p("welcome", &[n])));
        for id in [
            "yes",
            "no",
            "sure",
            "thanks",
            "no_idea",
            "nice",
            "congrats",
            "me_too",
            "sounds_good",
            "good_luck",
            "careful",
            "not_yet",
            "maybe_later",
            "you_too",
            "no_problem",
            "got_it",
            "same_here",
            "sorry",
            "haha",
            "thank_you",
            "on_my_way",
            "too_bad",
            "ouch",
            "hang_in",
            "well_done",
            "welcome_back",
            "what_about_you",
            "not_sure",
            "cant_now",
            "right_away",
            "which_map",
            "what_mean",
            "say_again",
        ] {
            out.push(p(id, &[]));
        }
        for id in [
            "bye",
            "see_you",
            "gotta_go",
            "take_care",
            "time_to_hunt",
            "good_night",
        ] {
            out.push(p(id, &[]));
        }
        // Lore: the eyes and the reference
        for m in &self.in_sight {
            out.push(p("here_now", &[m, &self.map]));
        }
        for lore in &self.lore {
            match lore {
                Lore::Monster {
                    name,
                    level,
                    hp,
                    aggressive,
                    maps,
                    drops,
                } => {
                    out.extend(
                        maps.iter()
                            .take(3)
                            .map(|(map, _)| p("lives_on", &[name, map])),
                    );
                    out.extend(
                        maps.iter()
                            .filter(|(_, n)| *n >= LOTS)
                            .take(2)
                            .map(|(map, _)| p("lots_on", &[name, map])),
                    );
                    out.extend(drops.iter().take(3).map(|item| p("drops", &[name, item])));
                    out.push(p(
                        if *aggressive { "aggressive" } else { "passive" },
                        &[name],
                    ));
                    out.push(p("mob_level", &[name, &level.to_string()]));
                    if !self.in_sight.iter().any(|m| m.eq_ignore_ascii_case(name)) {
                        out.push(p("no_here", &[name]));
                    }
                    if level.abs_diff(self.level) <= SAME_LEVELS {
                        out.push(p("about_my_level", &[name]));
                    } else if *level >= self.level + ABOVE_LEVELS {
                        out.push(p("above_me", &[name]));
                    }
                    out.push(p("mob_hp", &[name, &hp.to_string()]));
                    if self.killed(name) == 0
                        && !self.in_sight.iter().any(|m| m.eq_ignore_ascii_case(name))
                    {
                        out.push(p("not_seen", &[name]));
                        out.push(p("anyone_seen", &[name]));
                    }
                }
                Lore::Item {
                    name,
                    droppers,
                    shops,
                } => {
                    out.extend(droppers.iter().take(3).map(|m| p("dropped_by", &[name, m])));
                    out.extend(
                        shops
                            .iter()
                            .take(2)
                            .map(|(map, _)| p("sold_on", &[name, map])),
                    );
                    out.extend(
                        shops
                            .iter()
                            .take(2)
                            .map(|(map, price)| p("price_on", &[name, &price.to_string(), map])),
                    );
                    if shops.is_empty() && droppers.is_empty() {
                        out.push(p("where_item", &[name]));
                    }
                }
            }
        }
        // Warnings: what killed the bot
        for (m, n) in &self.deaths {
            if *n >= 2 || self.losing_to.contains(m) {
                out.push(p("watch_out", &[m]));
            }
            out.push(p("killed_me", &[m, &n.to_string()]));
            out.push(p("hits_hard", &[m]));
        }
        for m in &self.losing_to {
            out.push(p("too_strong", &[m, &level]));
            out.push(p("not_alone", &[m]));
        }
        // News: the bot itself, and its journal
        out.push(p("me_level", &[&level, &self.job]));
        out.push(p("level_now", &[&level]));
        if self.job_level > 1 {
            out.push(p("job_level_now", &[&self.job_level.to_string()]));
        }
        if let Some(map) = &self.hunt_map {
            out.push(p(
                if self.hunting {
                    "hunting_on"
                } else {
                    "heading_to"
                },
                &[map],
            ));
            out.push(p("party_for", &[map]));
            out.push(p("anyone_on", &[map]));
            out.push(p("map_good", &[map, &level]));
        }
        match self.doing.as_str() {
            "sell loot" => out.push(p("selling", &[])),
            "buy potions" => out.push(p("buying_potions", &[])),
            "rest" => out.push(p("resting", &[])),
            doing => {
                if let Some(map) = doing.strip_prefix("explore ") {
                    out.push(p("exploring", &[map]));
                }
                if let Some(job) = doing.strip_prefix("change job to ") {
                    out.push(p("heading_for", &[job]));
                }
            }
        }
        if self.kills_total > 0 {
            out.push(p("total_kills", &[&self.kills_total.to_string()]));
            if self.deaths_total == 0 {
                out.push(p("no_deaths", &[]));
            }
        }
        if let Some(map) = self.hunt_map.as_ref().filter(|_| self.hunting) {
            out.push(p("busy_on", &[map]));
            if let Some(m) = self.in_sight.iter().find(|m| self.killed(m) > 0) {
                out.push(p("killing_now", &[m]));
            }
        }
        if self.low_potions {
            out.push(p("low_potions", &[]));
        }
        if matches!(self.doing.as_str(), "sell loot" | "buy potions") {
            out.push(p("going_town", &[]));
        }
        if let Some(item) = self.doing.strip_prefix("buy ").filter(|i| *i != "potions") {
            // "buy 3 Red Potion": the item without the amount
            let item = item
                .split_once(' ')
                .filter(|(n, _)| n.parse::<u32>().is_ok())
                .map_or(item, |(_, i)| i);
            out.push(p("going_buy", &[item]));
        }
        if self.hp_frac > 0.0 && self.hp_frac < LOW_HP {
            out.push(p("low_hp", &[]));
        }
        if self.weight_frac >= HEAVY {
            out.push(p("too_heavy", &[]));
        }
        out.push(p("my_zeny", &[&self.zeny.to_string()]));
        if self.skill_points > 0 {
            out.push(p("skill_points", &[&self.skill_points.to_string()]));
        }
        if self.status_points > 0 {
            out.push(p("stat_points", &[&self.status_points.to_string()]));
        }
        if let Some(job) = self
            .job_target
            .as_ref()
            .filter(|j| !j.eq_ignore_ascii_case(&self.job))
        {
            out.push(p("next_job", &[job]));
            // A Novice changes at job level 10, a first job at 40
            let needed = if self.job == "Novice" { 10 } else { 40 };
            if self.job_level < needed {
                out.push(p("need_job_level", &[&needed.to_string(), job]));
            }
        }
        if self.deaths_total > 0 {
            out.push(p("total_deaths", &[&self.deaths_total.to_string()]));
        }
        match &self.party {
            Some(members) => {
                out.extend(members.iter().map(|m| p("party_with", &[m])));
                if self.hunting {
                    out.extend(members.iter().map(|m| p("hunting_with", &[m])));
                }
                out.push(p("wait_me", &[]));
                if self.leader.is_some()
                    && self.leader == self.other
                    && self.leader.as_deref() != Some(self.me.as_str())
                {
                    out.push(p("follow_you", &[]));
                }
                if self.doing == "rest" {
                    out.push(p("wait_resting", &[]));
                }
                if self.can_heal {
                    out.push(p("can_heal", &[]));
                }
            }
            None => out.push(p("want_party", &[])),
        }
        for (m, n) in self.kills.iter().take(5) {
            out.push(p("killed_many", &[&n.to_string(), m]));
            if *n >= EASY_KILLS && self.died_to(m) == 0 {
                out.push(p("easy", &[m, &level]));
            }
        }
        for line in &self.journal {
            let phrases = journal_phrases(line);
            if phrases.iter().any(|ph| ph.template == "help_quest") {
                out.push(p("someone_help", &[]));
            }
            out.extend(phrases);
        }
        if self.low_potions {
            out.push(p("need_potions", &[]));
        }
        if self.job != "Novice" {
            out.push(p("tips_job", &[&self.job]));
        }
        // No sentence twice, and a few of each category
        let mut seen = std::collections::HashSet::new();
        out.retain(|ph| seen.insert(ph.render().to_lowercase()));
        let mut per: std::collections::HashMap<Category, usize> = Default::default();
        out.retain(|ph| {
            let n = per.entry(ph.category()).or_default();
            *n += 1;
            *n <= PER_CATEGORY
        });
        out
    }
}

fn frac(a: u32, b: u32) -> f32 {
    if b == 0 { 0.0 } else { a as f32 / b as f32 }
}

/// The other members of the bot's party, `None` out of one.
pub fn party_of(obs: &crate::api::Observation) -> Option<Vec<String>> {
    let party = obs.party.as_ref()?;
    Some(
        party
            .members
            .iter()
            .map(|m| m.name.clone())
            .filter(|n| !n.eq_ignore_ascii_case(&obs.me.name))
            .collect(),
    )
}

/// The monsters in sight, each once.
pub fn names_in_sight(obs: &crate::api::Observation) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for m in &obs.mobs {
        if !m.name.is_empty() && !names.contains(&m.name) {
            names.push(m.name.clone());
        }
    }
    names
}

/// What a journal line lets the bot tell.
pub fn journal_phrases(line: &str) -> Vec<Phrase> {
    let p = Phrase::new;
    if let Some(rest) = line.strip_prefix("changed job: ")
        && let Some(job) = rest.rsplit(" -> ").next()
    {
        return vec![p("new_job", &[job])];
    }
    if let Some(quest) = line.strip_suffix(" finished") {
        return vec![p("finished", &[quest])];
    }
    if let Some((quest, _)) = line.split_once(": stuck at '") {
        return vec![p("stuck", &[quest]), p("help_quest", &[quest])];
    }
    if let Some(rest) = line.strip_prefix("could not open the shop of ")
        && let Some((_, rest)) = rest.split_once(" on ")
        && let Some(map) = rest.split([',', ' ']).next()
    {
        return vec![p("shop_closed", &[map])];
    }
    if let Some(rest) = line.strip_prefix("reached base level ")
        && let Some((level, map)) = rest.split_once(" on ")
        && level.parse::<u32>().is_ok()
        && (map.contains("_fild") || map.contains("_dun"))
    {
        return vec![
            p("leveled_on", &[level, map]),
            p("good_spot", &[level, map]),
        ];
    }
    if let Some(mob) = line.strip_suffix(" keeps killing me: leaving it alone for a few levels") {
        return vec![p("watch_out", &[mob])];
    }
    if let Some(map) = line.strip_prefix("first visit to ") {
        return vec![p("first_visit", &[map])];
    }
    if let Some(rest) = line.strip_prefix("died fighting ")
        && let Some((mob, map)) = rest.rsplit_once(" on ")
    {
        return vec![p("died_on", &[mob, map]), p("careful_on", &[map])];
    }
    if let Some(rest) = line.strip_prefix("died to ")
        && let Some((_, map)) = rest.rsplit_once(" on the way through ")
    {
        return vec![p("careful_on", &[map])];
    }
    if let Some(rest) = line.strip_prefix("now wearing ")
        && let Some((item, _)) = rest.rsplit_once(" (")
    {
        return vec![p("wearing", &[item])];
    }
    if let Some(rest) = line.strip_prefix("bought ")
        && let Some((what, _)) = rest.rsplit_once(" for ")
    {
        // "bought 5 Red Potion": the item without the amount
        let item = what
            .split_once(' ')
            .filter(|(n, _)| n.parse::<u32>().is_ok())
            .map(|(_, item)| item)
            .unwrap_or(what);
        return vec![p("bought", &[item])];
    }
    Vec::new()
}

impl Known {
    /// The lessons the bot's own experience holds: monsters that killed it, maps where it died, shops that would not
    /// open, maps where it levelled near its level now, monsters it kills easily. Each once.
    pub fn own_lessons(&self) -> Vec<Lesson> {
        let mut out: Vec<Lesson> = Vec::new();
        let mut add = |note: Phrase, effect: Effect| {
            if !out.iter().any(|l| l.note == note) {
                out.push(Lesson { note, effect });
            }
        };
        for (m, n) in &self.deaths {
            if *n >= 2 || self.losing_to.contains(m) {
                add(
                    Phrase::new("note_avoid", &[m]),
                    Effect::Avoid {
                        monster: m.clone(),
                        until: self.level + CAUTION_LEVELS,
                    },
                );
            }
        }
        for line in &self.journal {
            for p in journal_phrases(line) {
                match p.template {
                    "careful_on" => add(
                        Phrase::new("note_careful", &[p.value("map").unwrap_or("")]),
                        Effect::None,
                    ),
                    "shop_closed" => add(
                        Phrase::new("note_shop", &[p.value("map").unwrap_or("")]),
                        Effect::None,
                    ),
                    "leveled_on" => {
                        let at = p
                            .value("level")
                            .and_then(|l| l.parse::<u32>().ok())
                            .unwrap_or(0);
                        if self.level.abs_diff(at) <= SPOT_LEVELS {
                            add(
                                Phrase::new("note_try", &[p.value("map").unwrap_or("")]),
                                Effect::None,
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
        for (m, n) in self.kills.iter().take(5) {
            if *n >= EASY_KILLS && self.died_to(m) == 0 {
                add(Phrase::new("note_easy", &[m]), Effect::None);
            }
        }
        out
    }
}

/// Whether `notes` already hold the lesson, by itself or as heard on the forum.
pub fn noted(notes: &[String], lesson: &Lesson) -> bool {
    let text = lesson.note.render().to_lowercase();
    notes
        .iter()
        .any(|n| n.to_lowercase().trim_start_matches("forum: ") == text)
}

/// What a lesson does to the strategy, besides the note.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    None,
    /// Leave the monster alone below this base level.
    Avoid {
        monster: String,
        until: u32,
    },
    /// Head for this job.
    Job(String),
}

/// A lesson the bot may keep from what another player wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct Lesson {
    pub note: Phrase,
    pub effect: Effect,
}

/// The lessons a sentence of another player holds for a bot at `level` and `job`.
pub fn lessons_of(phrase: &Phrase, level: u32, job: &str) -> Vec<Lesson> {
    let v = |slot: &str| phrase.value(slot).unwrap_or("").to_owned();
    let lesson = |note: Phrase, effect: Effect| vec![Lesson { note, effect }];
    let num = |slot: &str| phrase.value(slot).and_then(|s| s.parse::<u32>().ok());
    match phrase.template {
        "watch_out" | "killed_me" | "hits_hard" | "not_alone" | "died_on" => {
            let m = v("monster");
            lesson(
                Phrase::new("note_avoid", &[&m]),
                Effect::Avoid {
                    monster: m.clone(),
                    until: level + CAUTION_LEVELS,
                },
            )
        }
        "too_strong" => match num("level") {
            Some(at) if level <= at => {
                let m = v("monster");
                lesson(
                    Phrase::new("note_avoid", &[&m]),
                    Effect::Avoid {
                        monster: m.clone(),
                        until: at + CAUTION_LEVELS,
                    },
                )
            }
            _ => Vec::new(),
        },
        "leveled_on" | "good_spot" => match num("level") {
            Some(at) if level.abs_diff(at) <= SPOT_LEVELS => {
                lesson(Phrase::new("note_try", &[&v("map")]), Effect::None)
            }
            _ => Vec::new(),
        },
        "new_job" => {
            let wanted = v("job");
            match job_path(&wanted) {
                // A first job for a Novice, a second job on the first job it comes from
                Some((path, _))
                    if (job == "Novice" && path.len() == 1)
                        || (path.len() == 2 && path[0].eq_ignore_ascii_case(job)) =>
                {
                    let name = path.last().copied().unwrap_or_default().to_owned();
                    lesson(Phrase::new("note_job", &[&name]), Effect::Job(name))
                }
                _ => Vec::new(),
            }
        }
        "shop_closed" => lesson(Phrase::new("note_shop", &[&v("map")]), Effect::None),
        "careful_on" => lesson(Phrase::new("note_careful", &[&v("map")]), Effect::None),
        "drops" => lesson(
            Phrase::new("note_drop", &[&v("monster"), &v("item")]),
            Effect::None,
        ),
        "dropped_by" => lesson(
            Phrase::new("note_drop", &[&v("monster"), &v("item")]),
            Effect::None,
        ),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bot() -> Known {
        Known {
            me: "Elara".into(),
            level: 9,
            job: "Novice".into(),
            map: "prt_fild08".into(),
            hunt_map: Some("prt_fild08".into()),
            hunting: true,
            in_sight: vec!["Poring".into()],
            kills: vec![("Poring".into(), 40), ("Fabre".into(), 3)],
            deaths: vec![("Fabre".into(), 2)],
            journal: vec![
                "reached base level 9 on prt_fild08".into(),
                "changed job: Novice -> Acolyte".into(),
                "bought Knife for 50 z".into(),
                "died fighting Fabre on prt_fild05".into(),
            ],
            lore: vec![Lore::Monster {
                name: "Poison Spore".into(),
                level: 19,
                hp: 665,
                aggressive: false,
                maps: vec![("prt_fild05".into(), 30)],
                drops: vec!["Poison Spore".into()],
            }],
            ..Default::default()
        }
    }

    #[test]
    fn the_bot_offers_only_what_it_knows() {
        let said: Vec<String> = bot().candidates().iter().map(Phrase::render).collect();
        for want in [
            "Poring is here on prt_fild08",
            "Poison Spore lives on prt_fild05",
            "Haven't seen Poison Spore",
            "Watch out for Fabre",
            "Fabre killed me 2 times",
            "Killed 40 Poring",
            "Poring is easy at level 9",
            "Hunting on prt_fild08",
            "Good spot for level 9: prt_fild08",
            "New job: Acolyte!",
            "Poison Spore is level 19",
            "Lots of Poison Spore on prt_fild05",
            "Fabre hits hard",
            "Level 9 now!",
            "Anyone want to party?",
            "Bought Knife",
            "Died to Fabre on prt_fild05",
            "Careful on prt_fild05",
        ] {
            assert!(said.iter().any(|s| s == want), "{want} in {said:?}");
        }
        for never in [
            "Haven't seen Poring",
            "Haven't seen Fabre",
            "Fabre is easy at level 9",
            "Need potions",
            "Wait for me",
            "Selling my loot",
        ] {
            assert!(!said.iter().any(|s| s == never), "{never}");
        }
    }

    #[test]
    fn the_bot_learns_from_its_own_deaths_spots_and_easy_kills() {
        let mut k = bot();
        k.deaths = vec![("Fabre".into(), 3)];
        let notes: Vec<String> = k.own_lessons().iter().map(|l| l.note.render()).collect();
        assert_eq!(
            notes,
            vec![
                "Avoid Fabre while weak",
                "Try hunting on prt_fild08",
                "Be careful on prt_fild05",
                "Poring is an easy kill"
            ]
        );
        let avoid = &k.own_lessons()[0];
        assert_eq!(
            avoid.effect,
            Effect::Avoid {
                monster: "Fabre".into(),
                until: 14
            }
        );
        assert!(
            noted(&["forum: Avoid Fabre while weak".into()], avoid)
                && noted(&["avoid fabre while weak".into()], avoid)
        );
        assert!(!noted(&["Poring is an easy kill".into()], avoid));
    }

    #[test]
    fn what_others_write_becomes_lessons_that_fit_the_bot() {
        let warn = Phrase::new("too_strong", &["Spore", "15"]);
        assert_eq!(
            lessons_of(&warn, 10, "Novice")[0].effect,
            Effect::Avoid {
                monster: "Spore".into(),
                until: 20
            }
        );
        assert!(
            lessons_of(&warn, 30, "Swordman").is_empty(),
            "a bot past that level has nothing to fear"
        );
        assert_eq!(
            lessons_of(
                &Phrase::new("good_spot", &["12", "prt_fild08"]),
                10,
                "Novice"
            )[0]
            .note
            .render(),
            "Try hunting on prt_fild08"
        );
        assert!(
            lessons_of(
                &Phrase::new("good_spot", &["40", "gef_dun01"]),
                10,
                "Novice"
            )
            .is_empty()
        );
        assert_eq!(
            lessons_of(&Phrase::new("new_job", &["Mage"]), 9, "Novice")[0].effect,
            Effect::Job("Mage".into())
        );
        assert_eq!(
            lessons_of(&Phrase::new("new_job", &["Wizard"]), 30, "Mage")[0].effect,
            Effect::Job("Wizard".into())
        );
        assert!(lessons_of(&Phrase::new("new_job", &["Wizard"]), 9, "Novice").is_empty());
        assert!(lessons_of(&Phrase::new("new_job", &["Knight"]), 30, "Mage").is_empty());
    }
}
