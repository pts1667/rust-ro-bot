//! Picking a sentence the way a player goes through a menu: the topics, then the sentences of one, with Back to the
//! topics (at most `MAX_BACKS` times per message). The model sees the whole menu at each step: the topics while it is
//! in one (not as choices), and what it saw in the topics it went back from.
//!
//! The topics are the catalogue's (`phrase::TOPICS`), except that what the bot knows about one monster or item makes
//! a topic of its own ("about_fabre") once there are `SUBJECT_MIN` sentences about it: lore grows with what the bot
//! knows, and questions are asked about a monster ("is Fabre strong?").

use anyhow::{Result, anyhow};

use super::phrase::{Category, Phrase, TOPICS};
use super::systemone::{Question, SystemOne};

/// Most times the model may go back to the topics in one walk.
pub const MAX_BACKS: usize = 2;
/// How sharp the draws are: the topic stays close to the model's pick, the sentence varies more.
const TOPIC_TEMPERATURE: f64 = 0.5;
const SENTENCE_TEMPERATURE: f64 = 0.8;
const BACK: &str = "back";
/// Sentences about one monster or item from which they make a topic of their own.
const SUBJECT_MIN: usize = 2;
/// The catalogue topics whose sentences go to subjects; the subjects come where the first of them was.
const SUBJECT_TOPICS: [&str; 4] = ["monster_where", "monster_strength", "items", "danger"];
/// Templates that are about an item even with a monster in them ("{item} drops from {monster}").
const ITEM_TEMPLATES: [&str; 3] = ["dropped_by", "sold_on", "price_on"];

/// A topic of the menu, with its sentences.
#[derive(Debug, Clone)]
pub struct Group<'a> {
    pub key: String,
    /// For the model.
    pub description: String,
    pub phrases: Vec<&'a Phrase>,
}

/// The menu's topics for `phrases`, in catalogue order, each with its sentences; empty topics are left out.
pub fn groups<'a>(phrases: &[&'a Phrase]) -> Vec<Group<'a>> {
    // Monsters and items with enough sentences about them
    let mut subjects: Vec<Group<'a>> = Vec::new();
    for p in phrases {
        let Some((kind, name)) = subject(p) else {
            continue;
        };
        let key = subject_key(name);
        match subjects.iter_mut().find(|g| g.key == key) {
            Some(g) => g.phrases.push(p),
            None => subjects.push(Group {
                key,
                description: subject_description(kind, name),
                phrases: vec![p],
            }),
        }
    }
    subjects.retain(|g| g.phrases.len() >= SUBJECT_MIN);
    let in_subject = |p: &Phrase| subjects.iter().any(|g| g.phrases.iter().any(|q| *q == p));
    let mut out = Vec::new();
    let mut placed = false;
    for t in TOPICS {
        if !placed && SUBJECT_TOPICS.contains(&t.key) {
            placed = true;
            out.extend(subjects.iter().cloned());
        }
        let of: Vec<&Phrase> = phrases
            .iter()
            .copied()
            .filter(|p| t.templates.contains(&p.template) && !in_subject(p))
            .collect();
        if !of.is_empty() {
            out.push(Group {
                key: t.key.to_owned(),
                description: t.description.to_owned(),
                phrases: of,
            });
        }
    }
    out
}

/// What a lore sentence or a warning is about: ("monster" or "item", the name).
fn subject(p: &Phrase) -> Option<(&'static str, &str)> {
    if ITEM_TEMPLATES.contains(&p.template) {
        return p.value("item").map(|i| ("item", i));
    }
    if matches!(p.category(), Category::Lore | Category::Warning) {
        return p.value("monster").map(|m| ("monster", m));
    }
    None
}

fn subject_key(name: &str) -> String {
    let words: Vec<String> = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    format!("about_{}", words.join("_"))
}

fn subject_description(kind: &str, name: &str) -> String {
    match kind {
        "item" => format!(
            "About {name} (an item): which monster drops it, where it is sold and for how much"
        ),
        _ => format!(
            "About {name} (a monster): where it is, how strong it is, what it drops, how dangerous it is"
        ),
    }
}

pub fn group_options(groups: &[Group]) -> Vec<(String, String)> {
    groups
        .iter()
        .map(|g| (g.key.clone(), g.description.clone()))
        .collect()
}

/// A menu of sentences.
pub struct Menu<'a> {
    pub phrases: Vec<&'a Phrase>,
    /// Choices next to the topics, such as saying nothing: (key, description).
    pub extra: Vec<(String, String)>,
    /// The questions at the topics and in a topic.
    pub ask_topic: String,
    pub ask_sentence: String,
    /// Take the model's best pick at each step instead of drawing (for the comparison in `eval`).
    pub best: bool,
}

/// What a walk through the menu ended on.
#[derive(Debug, Clone, PartialEq)]
pub enum Picked<'a> {
    Phrase(&'a Phrase),
    /// One of `Menu::extra`, by key.
    Extra(String),
}

/// A walk: what it ended on (`None` when the menu is empty), and the steps taken, for the log.
#[derive(Debug)]
pub struct Walk<'a> {
    pub picked: Option<Picked<'a>>,
    pub steps: Vec<String>,
    pub backs: usize,
}

impl<'a> Menu<'a> {
    /// Walks the menu from the topics, or from inside the topic `open` (already picked by the caller).
    pub async fn walk(&self, s1: &SystemOne, state: &str, open: Option<&str>) -> Result<Walk<'a>> {
        let topics = groups(&self.phrases);
        let mut walk = Walk {
            picked: None,
            steps: Vec::new(),
            backs: 0,
        };
        if topics.is_empty() {
            return Ok(walk);
        }
        let mut open = open.and_then(|k| topics.iter().position(|g| g.key == k));
        // The topics gone back from, by index
        let mut left: Vec<usize> = Vec::new();
        loop {
            let at = match open.take() {
                Some(i) => i,
                None => {
                    let mut options = group_options(&topics);
                    options.extend(self.extra.iter().cloned());
                    let view = format!("{state}\n\n{}", view(&topics, &left, None));
                    let key = self
                        .pick(s1, &view, &self.ask_topic, options, TOPIC_TEMPERATURE)
                        .await?;
                    match topics.iter().position(|g| g.key == key) {
                        Some(i) => i,
                        None if self.extra.iter().any(|(k, _)| *k == key) => {
                            walk.steps.push(key.clone());
                            walk.picked = Some(Picked::Extra(key));
                            return Ok(walk);
                        }
                        None => 0,
                    }
                }
            };
            let topic = &topics[at];
            walk.steps.push(topic.key.clone());
            let of = &topic.phrases;
            let can_back = left.len() < MAX_BACKS && topics.len() > 1;
            if of.len() == 1 && !can_back {
                walk.steps.push(format!("\"{}\"", of[0].render()));
                walk.picked = Some(Picked::Phrase(of[0]));
                return Ok(walk);
            }
            let mut options: Vec<(String, String)> = of
                .iter()
                .enumerate()
                .map(|(i, p)| (format!("s{i}"), p.render()))
                .collect();
            if can_back {
                options.push((
                    BACK.into(),
                    "Back: none of these sentences fits the conversation; return to the topics and pick another".into(),
                ));
            }
            let view = format!("{state}\n\n{}", view(&topics, &left, Some(at)));
            let key = self
                .pick(s1, &view, &self.ask_sentence, options, SENTENCE_TEMPERATURE)
                .await?;
            if key == BACK {
                walk.steps.push(BACK.into());
                walk.backs += 1;
                left.push(at);
                continue;
            }
            let phrase = key
                .strip_prefix('s')
                .and_then(|i| i.parse::<usize>().ok())
                .and_then(|i| of.get(i))
                .copied()
                .unwrap_or(of[0]);
            walk.steps.push(format!("\"{}\"", phrase.render()));
            walk.picked = Some(Picked::Phrase(phrase));
            return Ok(walk);
        }
    }

    async fn pick(
        &self,
        s1: &SystemOne,
        state: &str,
        ask: &str,
        options: Vec<(String, String)>,
        temperature: f64,
    ) -> Result<String> {
        if !self.best {
            return s1.choose(state, ask, options, temperature).await;
        }
        let answers = s1
            .ask(state, &[("pick", Question::choice(ask, options))])
            .await?;
        answers
            .get("pick")
            .and_then(|a| a.choice.clone())
            .ok_or_else(|| anyhow!("decision model: no choice"))
    }
}

/// The menu as the model reads it: the topics (the open one marked), then its history in this menu, numbered, with
/// every Back it selected spelled out, and where it is now. The sentences of the open topic are the choices themselves.
fn view(topics: &[Group], left: &[usize], open: Option<usize>) -> String {
    let mut out = String::from("Menu of messages. Topics:");
    for (i, t) in topics.iter().enumerate() {
        let mark = if open == Some(i) {
            " <- open now"
        } else if left.contains(&i) {
            " (you selected Back in it)"
        } else {
            ""
        };
        out += &format!("\n- {}: {}{mark}", t.key, t.description);
    }
    out += "\nYour history in this menu, oldest first:";
    let mut n = 0;
    let mut step = |out: &mut String, text: String| {
        n += 1;
        *out += &format!("\n{n}. {text}");
    };
    for &i in left {
        let t = &topics[i];
        let seen: Vec<String> = t
            .phrases
            .iter()
            .map(|p| format!("\"{}\"", p.render()))
            .collect();
        step(
            &mut out,
            format!(
                "Opened the topic {}, which offered: {}.",
                t.key,
                seen.join(", ")
            ),
        );
        step(
            &mut out,
            format!(
                "Selected Back: none of the sentences in {} fit. Returned to the topics.",
                t.key
            ),
        );
    }
    let backs_left = MAX_BACKS.saturating_sub(left.len());
    match open.map(|i| &topics[i]) {
        Some(t) if backs_left > 0 && topics.len() > 1 => step(
            &mut out,
            format!(
                "Opened the topic {} <- you are here. Pick one of its sentences, or Back to return to the topics ({backs_left} Back left).",
                t.key
            ),
        ),
        Some(t) => step(
            &mut out,
            format!(
                "Opened the topic {} <- you are here. No Back left: pick one of its sentences.",
                t.key
            ),
        ),
        None => step(
            &mut out,
            "At the topics <- you are here. Pick a topic.".to_owned(),
        ),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_shows_the_topics_and_what_was_left() {
        let p = [
            Phrase::new("hello", &[]),
            Phrase::new("yes", &[]),
            Phrase::new("no", &[]),
            Phrase::new("bye", &[]),
        ];
        let phrases: Vec<&Phrase> = p.iter().collect();
        let topics = groups(&phrases);
        assert_eq!(
            topics.iter().map(|t| t.key.as_str()).collect::<Vec<_>>(),
            ["greet", "accept", "decline", "bye"]
        );
        let text = view(&topics, &[1], Some(3));
        assert!(text.contains("- greet: Greet someone\n"), "{text}");
        assert!(
            text.lines()
                .any(|l| l.starts_with("- accept: ") && l.ends_with(" (you selected Back in it)")),
            "{text}"
        );
        assert!(text.contains("- bye: Say goodbye <- open now"), "{text}");
        assert!(
            text.contains("1. Opened the topic accept, which offered: \"Yes\"."),
            "{text}"
        );
        assert!(
            text.contains(
                "2. Selected Back: none of the sentences in accept fit. Returned to the topics."
            ),
            "{text}"
        );
        assert!(
            text.ends_with("3. Opened the topic bye <- you are here. Pick one of its sentences, or Back to return to the topics (1 Back left)."),
            "{text}"
        );
        let fresh = view(&topics, &[], None);
        assert!(
            fresh.ends_with("Your history in this menu, oldest first:\n1. At the topics <- you are here. Pick a topic."),
            "{fresh}"
        );
    }

    #[test]
    fn what_the_bot_knows_about_a_monster_or_an_item_is_a_topic() {
        let p = [
            Phrase::new("mob_level", &["Fabre", "2"]),
            Phrase::new("hits_hard", &["Fabre"]),
            Phrase::new("lives_on", &["Fabre", "prt_fild05"]),
            Phrase::new("here_now", &["Poring", "prt_fild08"]),
            Phrase::new("price_on", &["Red Potion", "50", "prontera"]),
            Phrase::new("dropped_by", &["Red Potion", "Poring"]),
            Phrase::new("careful_on", &["prt_fild05"]),
            Phrase::new("hello", &[]),
        ];
        let phrases: Vec<&Phrase> = p.iter().collect();
        let topics = groups(&phrases);
        let keys: Vec<&str> = topics.iter().map(|t| t.key.as_str()).collect();
        // Poring has one sentence: it stays in monster_where
        assert_eq!(
            keys,
            [
                "greet",
                "about_fabre",
                "about_red_potion",
                "monster_where",
                "danger"
            ]
        );
        assert_eq!(
            topics[1].phrases.len(),
            3,
            "level, hits hard and where, all about Fabre"
        );
        assert!(topics[1].description.starts_with("About Fabre (a monster)"));
        assert!(
            topics[2]
                .description
                .starts_with("About Red Potion (an item)")
        );
        assert_eq!(topics[4].phrases[0].render(), "Careful on prt_fild05");
    }

    /// A decision model that selects Back whenever it may, else the first option (keys come sorted); the requests it
    /// got go to `seen`.
    async fn backward_model(
        seen: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    ) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let body = loop {
                    let n = socket.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break None;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    let Some((head, body)) = text.split_once("\r\n\r\n") else {
                        continue;
                    };
                    let length = head
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if body.len() >= length {
                        break Some(body.to_owned());
                    }
                };
                let Some(body) = body else { continue };
                let request: serde_json::Value = serde_json::from_str(&body).unwrap();
                let mut answers = serde_json::Map::new();
                for (name, q) in request["questions"].as_object().unwrap() {
                    let keys: Vec<&String> = q["criteria"].as_object().unwrap().keys().collect();
                    let pick = if keys.iter().any(|k| *k == BACK) {
                        BACK
                    } else {
                        keys[0].as_str()
                    };
                    answers.insert(
                        name.clone(),
                        serde_json::json!({"choice": pick, "probabilities": {pick: 1.0}}),
                    );
                }
                seen.lock().unwrap().push(request);
                let out = serde_json::json!({"answers": answers}).to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{out}",
                    out.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        url
    }

    #[tokio::test]
    async fn the_model_goes_back_at_most_twice_and_sees_it_did() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let s1 = SystemOne::new(&backward_model(seen.clone()).await).unwrap();
        let p = [
            Phrase::new("hello", &[]),
            Phrase::new("yes", &[]),
            Phrase::new("sure", &[]),
            Phrase::new("no", &[]),
            Phrase::new("bye", &[]),
        ];
        let menu = Menu {
            phrases: p.iter().collect(),
            extra: Vec::new(),
            ask_topic: "Which topic?".into(),
            ask_sentence: "Which sentence?".into(),
            best: true,
        };
        let walk = menu.walk(&s1, "Bravo: hi", None).await.unwrap();
        assert_eq!(walk.backs, MAX_BACKS);
        assert_eq!(
            walk.steps,
            ["accept", "back", "accept", "back", "accept", "\"Yes\""]
        );
        assert_eq!(walk.picked, Some(Picked::Phrase(&p[1])));
        let seen = seen.lock().unwrap();
        // The last request: in accept (the first key) with no Back left, and both Backs in its history
        let last = seen.last().unwrap();
        let state = last["state"].as_str().unwrap();
        assert!(
            state.contains("2. Selected Back: none of the sentences in accept fit."),
            "{state}"
        );
        assert!(
            state.contains("4. Selected Back: none of the sentences in accept fit."),
            "{state}"
        );
        assert!(state.contains("No Back left"), "{state}");
        let keys: Vec<&String> = last["questions"]["pick"]["criteria"]
            .as_object()
            .unwrap()
            .keys()
            .collect();
        assert!(!keys.iter().any(|k| *k == BACK), "{keys:?}");
    }
}
