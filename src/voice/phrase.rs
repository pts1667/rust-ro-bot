//! What a bot can say with the decision model, as in the Dark Souls messages: a fixed catalogue of templates with
//! slots (`Watch out for {monster}`), in categories, and conjunctions that join two of them. The bot fills the slots
//! only with what it knows (`known`), so what it says is true; the model picks among the sentences.
//!
//! The catalogue is shared by every bot, so a sentence one bot wrote parses back into its template and values: another
//! bot reads it as data (`parse`).

/// The kinds of templates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Greeting,
    Reply,
    /// What the game reference or the bot's eyes say about monsters, items and maps.
    Lore,
    Warning,
    /// What the bot did and does.
    News,
    Request,
    Farewell,
    /// What the bot keeps for its strategy (Σ notes); never said.
    Lesson,
}

/// The kinds as the model saw them before the topics; kept for the comparison in `voice/eval.rs`.
#[cfg(test)]
impl Category {
    pub const SPOKEN: [Category; 7] = [
        Category::Greeting,
        Category::Reply,
        Category::Lore,
        Category::Warning,
        Category::News,
        Category::Request,
        Category::Farewell,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Category::Greeting => "greeting",
            Category::Reply => "reply",
            Category::Lore => "lore",
            Category::Warning => "warning",
            Category::News => "news",
            Category::Request => "request",
            Category::Farewell => "farewell",
            Category::Lesson => "lesson",
        }
    }

    /// For the model.
    pub fn description(self) -> &'static str {
        match self {
            Category::Greeting => "Greet someone",
            Category::Reply => "A short answer: yes, no, thanks, no idea, good luck",
            Category::Lore => {
                "A fact about a monster, an item or a map: where it lives, what it drops, where it is sold"
            }
            Category::Warning => {
                "Warn about a danger: a monster that kills, a shop that does not work"
            }
            Category::News => {
                "Tell what you did or do: your level, where you hunt, what you killed, a new job, a quest"
            }
            Category::Request => {
                "Ask for something: a monster's place, potions, a party, help with a quest"
            }
            Category::Farewell => "Say goodbye",
            Category::Lesson => "A lesson to keep",
        }
    }
}

pub struct Template {
    pub id: &'static str,
    pub category: Category,
    /// Slots are `{monster}`, `{map}`, `{item}`, `{job}`, `{level}`, `{count}`, `{player}`, `{quest}`.
    pub text: &'static str,
}

const fn t(id: &'static str, category: Category, text: &'static str) -> Template {
    Template { id, category, text }
}

use Category::*;

pub const TEMPLATES: &[Template] = &[
    t("hello", Greeting, "Hello!"),
    t("hi_player", Greeting, "Hi {player}!"),
    t("welcome", Greeting, "Welcome, {player}!"),
    t("hey_all", Greeting, "Hey everyone!"),
    t("good_to_see", Greeting, "Good to see you, {player}"),
    t("yes", Reply, "Yes"),
    t("no", Reply, "No"),
    t("sure", Reply, "Sure"),
    t("thanks", Reply, "Thanks!"),
    t("no_idea", Reply, "No idea, sorry"),
    t("nice", Reply, "Nice!"),
    t("congrats", Reply, "Congrats!"),
    t("me_too", Reply, "Me too"),
    t("sounds_good", Reply, "Sounds good"),
    t("good_luck", Reply, "Good luck!"),
    t("careful", Reply, "Be careful"),
    t("bot", Reply, "Yes, I'm a bot"),
    t("not_yet", Reply, "Not yet"),
    t("maybe_later", Reply, "Maybe later"),
    t("you_too", Reply, "You too!"),
    t("no_problem", Reply, "No problem"),
    t("got_it", Reply, "Got it"),
    t("same_here", Reply, "Same here"),
    t("sorry", Reply, "Sorry"),
    t("haha", Reply, "Haha"),
    t("thank_you", Reply, "Thank you!"),
    t("on_my_way", Reply, "On my way"),
    t("here_now", Lore, "{monster} is here on {map}"),
    t("lives_on", Lore, "{monster} lives on {map}"),
    t("drops", Lore, "{monster} drops {item}"),
    t("dropped_by", Lore, "{item} drops from {monster}"),
    t("sold_on", Lore, "{item} is sold on {map}"),
    t("aggressive", Lore, "{monster} attacks on sight"),
    t("not_seen", Lore, "Haven't seen {monster}"),
    t("mob_level", Lore, "{monster} is level {level}"),
    t("mob_hp", Lore, "{monster} has {count} HP"),
    t("passive", Lore, "{monster} is passive"),
    t("lots_on", Lore, "Lots of {monster} on {map}"),
    t("price_on", Lore, "{item} costs {count} zeny on {map}"),
    t("watch_out", Warning, "Watch out for {monster}"),
    t("killed_me", Warning, "{monster} killed me {count} times"),
    t(
        "too_strong",
        Warning,
        "{monster} is too strong for a level {level}",
    ),
    t("shop_closed", Warning, "The shop on {map} won't open"),
    t("hits_hard", Warning, "{monster} hits hard"),
    t("not_alone", Warning, "Don't fight {monster} alone"),
    t("careful_on", Warning, "Careful on {map}"),
    t("died_on", Warning, "Died to {monster} on {map}"),
    t("me_level", News, "I'm a level {level} {job}"),
    t("hunting_on", News, "Hunting on {map}"),
    t("heading_to", News, "Heading to {map}"),
    t("easy", News, "{monster} is easy at level {level}"),
    t("killed_many", News, "Killed {count} {monster}"),
    t("new_job", News, "New job: {job}!"),
    t("leveled_on", News, "Leveled to {level} on {map}"),
    t("good_spot", News, "Good spot for level {level}: {map}"),
    t("finished", News, "Finished {quest}"),
    t("stuck", News, "Stuck on {quest}"),
    t("level_now", News, "Level {level} now!"),
    t("job_level_now", News, "My job level is {level}"),
    t("first_visit", News, "First time on {map}"),
    t("selling", News, "Selling my loot"),
    t("buying_potions", News, "Buying potions"),
    t("resting", News, "Resting a bit"),
    t("exploring", News, "Exploring {map}"),
    t("heading_for", News, "Heading for {job}"),
    t("bought", News, "Bought {item}"),
    t("wearing", News, "Wearing {item} now"),
    t("total_kills", News, "Killed {count} monsters so far"),
    t("total_deaths", News, "Died {count} times so far"),
    t("party_with", News, "In a party with {player}"),
    t("anyone_seen", Request, "Anyone seen {monster}?"),
    t("need_potions", Request, "Need potions"),
    t("party_for", Request, "Party for {map}?"),
    t("help_quest", Request, "Help with {quest}?"),
    t("tips_job", Request, "Any {job} tips?"),
    t("where_item", Request, "Where can I get {item}?"),
    t("want_party", Request, "Anyone want to party?"),
    t("anyone_on", Request, "Anyone hunting on {map}?"),
    t("map_good", Request, "Is {map} good for a level {level}?"),
    t("someone_help", Request, "Can someone help?"),
    t("wait_me", Request, "Wait for me"),
    t("bye", Farewell, "Bye!"),
    t("see_you", Farewell, "See you!"),
    t("gotta_go", Farewell, "Gotta go"),
    t("take_care", Farewell, "Take care!"),
    t("time_to_hunt", Farewell, "Time to hunt"),
    t("good_night", Farewell, "Good night"),
    t("too_bad", Reply, "Too bad"),
    t("ouch", Reply, "Ouch!"),
    t("hang_in", Reply, "Hang in there"),
    t("well_done", Reply, "Well done!"),
    t("welcome_back", Reply, "Welcome back!"),
    t("what_about_you", Reply, "What about you?"),
    t("not_sure", Reply, "Not sure"),
    t("cant_now", Reply, "Can't right now"),
    t("right_away", Reply, "Right away"),
    t("busy_on", Reply, "Busy hunting on {map}"),
    t("which_map", Reply, "Which map?"),
    t("what_mean", Reply, "What do you mean?"),
    t("say_again", Reply, "Say again?"),
    t("no_here", Lore, "No {monster} here"),
    t("about_my_level", Lore, "{monster} is about my level"),
    t("above_me", Lore, "{monster} is way above me"),
    t("no_deaths", News, "No deaths so far"),
    t("killing_now", News, "Killing {monster} now"),
    t("low_potions", News, "Low on potions"),
    t("going_town", News, "Going back to town"),
    t("low_hp", News, "Low on HP"),
    t("too_heavy", News, "Too heavy, need to sell"),
    t("my_zeny", News, "I have {count} zeny"),
    t("skill_points", News, "Got {count} skill points to spend"),
    t("stat_points", News, "Got {count} stat points to spend"),
    t("next_job", News, "{job} is my next job"),
    t("need_job_level", News, "Need job level {level} for {job}"),
    t("going_buy", News, "Going to buy {item}"),
    t("follow_you", News, "I'll follow you"),
    t("hunting_with", News, "Hunting with {player}"),
    t("wait_resting", News, "Wait, I'm resting"),
    t("can_heal", News, "I can heal"),
    t("note_avoid", Lesson, "Avoid {monster} while weak"),
    t("note_try", Lesson, "Try hunting on {map}"),
    t("note_job", Lesson, "Aim for {job}"),
    t("note_shop", Lesson, "The shop on {map} won't open"),
    t("note_drop", Lesson, "{monster} drops {item}"),
    t("note_careful", Lesson, "Be careful on {map}"),
    t("note_easy", Lesson, "{monster} is an easy kill"),
];

/// Conjunctions joining two sentences, as (key, text between them). `none` ends the message after one.
pub const CONJUNCTIONS: &[(&str, &str)] = &[
    ("none", ""),
    ("comma", ", "),
    ("and", ", and "),
    ("but", ", but "),
    ("so", ", so "),
    ("or", ", or "),
    ("dots", "... "),
];

/// For the model.
pub fn conjunction_description(key: &str) -> &'static str {
    match key {
        "none" => "Nothing more: the message is complete",
        "comma" => "A comma: the second sentence goes on from the first",
        "and" => "\"and\": the second sentence adds to the first",
        "but" => "\"but\": the second sentence contrasts with the first",
        "so" => "\"so\": the second sentence follows from the first",
        "or" => "\"or\": the second sentence is an alternative to the first",
        _ => "A pause: the second sentence is an afterthought",
    }
}

/// What the decision model picks first in a message: a topic, finer than the kinds, then a sentence of it. In a live
/// comparison (`voice/eval.rs`, PLAN.md) this beat one choice over every sentence and the kinds first. Every spoken
/// template is in exactly one topic; lessons are in none.
pub struct Topic {
    pub key: &'static str,
    /// For the model.
    pub description: &'static str,
    pub templates: &'static [&'static str],
}

pub const TOPICS: &[Topic] = &[
    Topic {
        key: "greet",
        description: "Greet someone",
        templates: &["hello", "hi_player", "welcome", "hey_all", "good_to_see"],
    },
    Topic {
        key: "accept",
        description: "Say yes or agree to do it: yes, sure, got it, right away, on my way, sounds good",
        templates: &[
            "yes",
            "sure",
            "got_it",
            "right_away",
            "on_my_way",
            "sounds_good",
            "bot",
        ],
    },
    Topic {
        key: "decline",
        description: "Say no, not now, or that you do not know: no, not yet, maybe later, can't right now, busy, not sure, no idea",
        templates: &[
            "no",
            "not_yet",
            "maybe_later",
            "cant_now",
            "busy_on",
            "not_sure",
            "no_idea",
        ],
    },
    Topic {
        key: "cheer",
        description: "Share their joy or agree: congrats, well done, nice, haha, me too, welcome back",
        templates: &[
            "congrats",
            "well_done",
            "nice",
            "haha",
            "me_too",
            "same_here",
            "welcome_back",
        ],
    },
    Topic {
        key: "comfort",
        description: "Comfort them after a death or bad luck: sorry, too bad, ouch, hang in there, be careful",
        templates: &["sorry", "too_bad", "ouch", "hang_in", "careful"],
    },
    Topic {
        key: "thank",
        description: "Thanks and wishes: thank you, you too, no problem, good luck, what about you",
        templates: &[
            "thanks",
            "thank_you",
            "you_too",
            "no_problem",
            "good_luck",
            "what_about_you",
        ],
    },
    Topic {
        key: "monster_where",
        description: "Where monsters are: the ones around you now, the map a monster lives on, where there are lots",
        templates: &["here_now", "lives_on", "lots_on", "not_seen", "no_here"],
    },
    Topic {
        key: "monster_strength",
        description: "How strong a monster is: its level, its HP, compared to you, whether it attacks on sight or is passive",
        templates: &[
            "mob_level",
            "mob_hp",
            "aggressive",
            "passive",
            "about_my_level",
            "above_me",
        ],
    },
    Topic {
        key: "items",
        description: "Items: which monster drops what, where an item is sold and its price",
        templates: &["drops", "dropped_by", "sold_on", "price_on"],
    },
    Topic {
        key: "danger",
        description: "Warn about a danger: a monster that hits hard or killed you, a risky map, a shop that does not open",
        templates: &[
            "watch_out",
            "killed_me",
            "too_strong",
            "shop_closed",
            "hits_hard",
            "not_alone",
            "careful_on",
            "died_on",
        ],
    },
    Topic {
        key: "my_level",
        description: "Your base level, job and job level now, a level-up",
        templates: &["me_level", "level_now", "job_level_now", "new_job"],
    },
    Topic {
        key: "my_maps",
        description: "Where you hunt or go: your hunting map, good spots, maps you visit",
        templates: &[
            "hunting_on",
            "heading_to",
            "first_visit",
            "exploring",
            "leveled_on",
            "good_spot",
        ],
    },
    Topic {
        key: "my_fights",
        description: "How your fights go: what you fight now, what and how many you killed, how often you died, what is easy",
        templates: &[
            "easy",
            "killed_many",
            "total_kills",
            "total_deaths",
            "no_deaths",
            "killing_now",
        ],
    },
    Topic {
        key: "shopping",
        description: "Shopping and gear: what you bought, wear, sell, or buy",
        templates: &["bought", "wearing", "selling", "buying_potions"],
    },
    Topic {
        key: "errands",
        description: "Errands besides fighting: resting, quests, going back to town, running low on potions",
        templates: &["resting", "finished", "stuck", "going_town", "low_potions"],
    },
    Topic {
        key: "ask_info",
        description: "Ask a question of your own, not a reply to theirs: where a monster or item is, whether a map is good, job tips, who hunts where",
        templates: &[
            "anyone_seen",
            "where_item",
            "map_good",
            "tips_job",
            "anyone_on",
        ],
    },
    Topic {
        key: "ask_help",
        description: "Make a request of your own, not a reply to theirs: potions, help with a quest, a party, to wait for you",
        templates: &[
            "need_potions",
            "help_quest",
            "someone_help",
            "wait_me",
            "party_for",
            "want_party",
        ],
    },
    Topic {
        key: "clarify",
        description: "Ask them to make it clear: you do not understand what they said or which map they mean",
        templates: &["which_map", "what_mean", "say_again"],
    },
    Topic {
        key: "status",
        description: "How you are right now: HP, weight, zeny, points to spend",
        templates: &[
            "low_hp",
            "too_heavy",
            "my_zeny",
            "skill_points",
            "stat_points",
        ],
    },
    Topic {
        key: "plans",
        description: "What you will do next: your next job and the job level it needs, what you go to buy",
        templates: &["heading_for", "next_job", "need_job_level", "going_buy"],
    },
    Topic {
        key: "party",
        description: "Your party: who you are with, following the leader, healing, waiting",
        templates: &[
            "party_with",
            "hunting_with",
            "follow_you",
            "wait_resting",
            "can_heal",
        ],
    },
    Topic {
        key: "bye",
        description: "Say goodbye",
        templates: &[
            "bye",
            "see_you",
            "gotta_go",
            "take_care",
            "time_to_hunt",
            "good_night",
        ],
    },
];

pub fn template(id: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.id == id)
}

/// A template with its slots filled.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Phrase {
    pub template: &'static str,
    pub values: Vec<String>,
}

impl Phrase {
    /// `values` fill the slots in order. Panics on an unknown template or a wrong number of values: both are bugs.
    pub fn new(id: &str, values: &[&str]) -> Phrase {
        let t = template(id).unwrap_or_else(|| panic!("no template {id}"));
        assert_eq!(slots(t.text).len(), values.len(), "values for {id}");
        Phrase {
            template: t.id,
            values: values.iter().map(|v| (*v).to_owned()).collect(),
        }
    }

    pub fn category(&self) -> Category {
        template(self.template).map(|t| t.category).unwrap_or(Reply)
    }

    /// The value of a slot, by name.
    pub fn value(&self, slot: &str) -> Option<&str> {
        let t = template(self.template)?;
        slots(t.text)
            .iter()
            .position(|s| *s == slot)
            .and_then(|i| self.values.get(i))
            .map(String::as_str)
    }

    pub fn render(&self) -> String {
        let Some(t) = template(self.template) else {
            return String::new();
        };
        let mut out = String::new();
        let mut values = self.values.iter();
        for piece in pieces(t.text) {
            match piece {
                Piece::Text(text) => out.push_str(text),
                Piece::Slot(_) => out.push_str(values.next().map(String::as_str).unwrap_or("")),
            }
        }
        out
    }
}

/// Two sentences joined by the conjunction `key`, or the first alone for `none` or an unknown key.
pub fn join(first: &Phrase, key: &str, second: Option<&Phrase>) -> String {
    let between = CONJUNCTIONS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, t)| *t)
        .unwrap_or("");
    match second {
        Some(second) if !between.is_empty() => {
            let first = first.render();
            let first = first.trim_end_matches(['.', '!']);
            let text = second.render();
            // "Watch out for Spore, and haven't seen Fabre"; a name and "I" keep their capital
            let keeps = template(second.template).is_some_and(|t| t.text.starts_with('{'))
                || text.starts_with("I ")
                || text.starts_with("I'");
            let text = if keeps { text } else { lower_first(&text) };
            format!("{first}{between}{text}")
        }
        _ => first.render(),
    }
}

fn lower_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

enum Piece<'a> {
    Text(&'a str),
    Slot(&'a str),
}

fn pieces(text: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        if open > 0 {
            out.push(Piece::Text(&rest[..open]));
        }
        let close = rest[open..]
            .find('}')
            .map(|c| open + c)
            .unwrap_or(rest.len() - 1);
        out.push(Piece::Slot(&rest[open + 1..close]));
        rest = &rest[close + 1..];
    }
    if !rest.is_empty() {
        out.push(Piece::Text(rest));
    }
    out
}

/// The slot names of a template text, in order.
pub fn slots(text: &str) -> Vec<&str> {
    pieces(text)
        .into_iter()
        .filter_map(|p| match p {
            Piece::Slot(s) => Some(s),
            Piece::Text(_) => None,
        })
        .collect()
}

/// The phrase a sentence is, when it is one of the catalogue's (case and end punctuation aside).
pub fn parse_sentence(sentence: &str) -> Option<Phrase> {
    let sentence = sentence.trim().trim_end_matches(['.', '!', '?', ' ']);
    if sentence.is_empty() {
        return None;
    }
    // The most specific template: "X drops from Y" is not "X drops Y"
    TEMPLATES
        .iter()
        .filter(|t| t.category != Lesson)
        .filter_map(|t| match_template(t, sentence).map(|p| (literal_len(t.text), p)))
        .max_by_key(|(len, _)| *len)
        .map(|(_, p)| p)
}

fn literal_len(text: &str) -> usize {
    pieces(text)
        .iter()
        .map(|p| match p {
            Piece::Text(t) => t.len(),
            Piece::Slot(_) => 0,
        })
        .sum()
}

fn match_template(t: &'static Template, sentence: &str) -> Option<Phrase> {
    let text = t.text.trim_end_matches(['.', '!', '?']);
    let pieces = pieces(text);
    let lower = sentence.to_lowercase();
    // Positions in `lower` are used in `sentence`
    if lower.len() != sentence.len() {
        return None;
    }
    let mut at = 0;
    let mut values = Vec::new();
    for (i, piece) in pieces.iter().enumerate() {
        match piece {
            Piece::Text(literal) => {
                let literal = literal.to_lowercase();
                if !lower[at..].starts_with(&literal) {
                    return None;
                }
                at += literal.len();
            }
            Piece::Slot(slot) => {
                // A slot runs to the next literal, or to the end
                let end = match pieces.get(i + 1) {
                    Some(Piece::Text(next)) => at + lower[at..].find(&next.to_lowercase())?,
                    _ => sentence.len(),
                };
                let value = sentence[at..end].trim();
                if !valid(slot, value) {
                    return None;
                }
                values.push(value.to_owned());
                at = end;
            }
        }
    }
    (at == sentence.len()).then_some(Phrase {
        template: t.id,
        values,
    })
}

/// Whether a parsed value can fill a slot: numbers for numbers, one map name for a map, and no sentence for a name.
fn valid(slot: &str, value: &str) -> bool {
    if value.is_empty() || value.len() > 40 {
        return false;
    }
    match slot {
        "level" | "count" => value.chars().all(|c| c.is_ascii_digit()),
        "map" => {
            !value.contains(' ') && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => value.split_whitespace().count() <= 4 && !value.contains([',', '.', '!', '?']),
    }
}

/// The catalogue sentences in a text: each sentence, or each part between conjunctions. Free text gives nothing.
pub fn parse(text: &str) -> Vec<Phrase> {
    let mut out = Vec::new();
    for sentence in text.split_inclusive(['.', '!', '?', '\n']) {
        if let Some(p) = parse_sentence(sentence) {
            out.push(p);
            continue;
        }
        let mut parts = vec![sentence.to_owned()];
        for (key, between) in CONJUNCTIONS.iter().rev() {
            if *key == "none" {
                continue;
            }
            parts = parts
                .iter()
                .flat_map(|p| p.split(between).map(str::to_owned).collect::<Vec<_>>())
                .collect();
        }
        out.extend(parts.iter().filter_map(|p| parse_sentence(p)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spoken_template_has_one_topic() {
        for t in TEMPLATES {
            let n = TOPICS
                .iter()
                .filter(|topic| topic.templates.contains(&t.id))
                .count();
            assert_eq!(n, (t.category != Lesson) as usize, "{}", t.id);
        }
        for topic in TOPICS {
            assert!(
                topic.templates.iter().all(|id| template(id).is_some()),
                "{}",
                topic.key
            );
        }
    }

    #[test]
    fn every_template_has_known_slots_and_renders_back() {
        const SLOTS: [&str; 8] = [
            "monster", "map", "item", "job", "level", "count", "player", "quest",
        ];
        for t in TEMPLATES {
            let values: Vec<String> = slots(t.text)
                .iter()
                .map(|s| {
                    assert!(SLOTS.contains(s), "{}: slot {s}", t.id);
                    match *s {
                        "level" | "count" => "12".to_owned(),
                        "map" => "prt_fild05".to_owned(),
                        other => format!("Some {other}"),
                    }
                })
                .collect();
            let p = Phrase {
                template: t.id,
                values,
            };
            if t.category != Lesson {
                assert_eq!(
                    parse_sentence(&p.render()).map(|b| b.template),
                    Some(t.id),
                    "{} parses back",
                    t.id
                );
            }
        }
        assert_eq!(
            TEMPLATES
                .iter()
                .map(|t| t.id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            TEMPLATES.len()
        );
    }

    #[test]
    fn sentences_render_join_and_parse_back() {
        let warn = Phrase::new("watch_out", &["Poison Spore"]);
        let died = Phrase::new("killed_me", &["Poison Spore", "4"]);
        let seen = Phrase::new("not_seen", &["Fabre"]);
        assert_eq!(
            join(&warn, "none", Some(&died)),
            "Watch out for Poison Spore"
        );
        assert_eq!(
            join(&warn, "so", Some(&seen)),
            "Watch out for Poison Spore, so haven't seen Fabre"
        );
        assert_eq!(
            join(&warn, "and", Some(&died)),
            "Watch out for Poison Spore, and Poison Spore killed me 4 times"
        );
        let me = Phrase::new("me_level", &["9", "Novice"]);
        assert_eq!(
            join(&Phrase::new("hello", &[]), "comma", Some(&me)),
            "Hello, I'm a level 9 Novice"
        );
        assert_eq!(
            parse("Watch out for Poison Spore, and Poison Spore killed me 4 times."),
            vec![warn.clone(), died]
        );
        assert_eq!(
            parse("Hello, I'm a level 9 Novice"),
            vec![Phrase::new("hello", &[]), me]
        );
        assert_eq!(
            parse("watch out for poison spore!"),
            vec![Phrase::new("watch_out", &["poison spore"])]
        );
        assert_eq!(
            parse("Good spot for level 12: prt_fild08"),
            vec![Phrase::new("good_spot", &["12", "prt_fild08"])]
        );
        assert!(
            parse("Did you try the fields around Payon instead?").is_empty(),
            "free text is no phrase"
        );
        assert!(
            parse("Killed lots of Porings").is_empty(),
            "a count is a number"
        );
        assert_eq!(warn.value("monster"), Some("Poison Spore"));
    }
}
