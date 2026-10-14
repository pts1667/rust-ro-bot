//! Picking a sentence the way a player goes through a menu: the topics, then the sentences of one, with Back to the
//! topics (at most `MAX_BACKS` times per message). The model sees the whole menu at each step: the topics while it is
//! in one (not as choices), and what it saw in the topics it went back from.

use anyhow::{Result, anyhow};

use super::phrase::{Phrase, TOPICS, Topic};
use super::systemone::{Question, SystemOne};

/// Most times the model may go back to the topics in one walk.
pub const MAX_BACKS: usize = 2;
/// How sharp the draws are: the topic stays close to the model's pick, the sentence varies more.
const TOPIC_TEMPERATURE: f64 = 0.5;
const SENTENCE_TEMPERATURE: f64 = 0.8;
const BACK: &str = "back";

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
    /// Walks the menu from the topics, or from inside `open` (already picked by the caller).
    pub async fn walk(
        &self,
        s1: &SystemOne,
        state: &str,
        mut open: Option<&'static Topic>,
    ) -> Result<Walk<'a>> {
        let topics = topics_of(&self.phrases);
        let mut walk = Walk {
            picked: None,
            steps: Vec::new(),
            backs: 0,
        };
        if topics.is_empty() {
            return Ok(walk);
        }
        // The topics gone back from, with their sentences
        let mut left: Vec<(&Topic, Vec<&'a Phrase>)> = Vec::new();
        loop {
            let topic = match open.take() {
                Some(t) => t,
                None => {
                    let mut options = topic_options(&topics);
                    options.extend(self.extra.iter().cloned());
                    let view = format!("{state}\n\n{}", view(&topics, &left, None));
                    let key = self
                        .pick(s1, &view, &self.ask_topic, options, TOPIC_TEMPERATURE)
                        .await?;
                    match topics.iter().find(|t| t.key == key) {
                        Some(t) => *t,
                        None if self.extra.iter().any(|(k, _)| *k == key) => {
                            walk.steps.push(key.clone());
                            walk.picked = Some(Picked::Extra(key));
                            return Ok(walk);
                        }
                        None => topics[0],
                    }
                }
            };
            walk.steps.push(topic.key.to_owned());
            let of = of_topic(&self.phrases, topic);
            let can_back = left.len() < MAX_BACKS && topics.len() > 1;
            if of.len() == 1 && !can_back {
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
                    "Back: none of these fits, return to the topics".into(),
                ));
            }
            let view = format!("{state}\n\n{}", view(&topics, &left, Some(topic)));
            let key = self
                .pick(s1, &view, &self.ask_sentence, options, SENTENCE_TEMPERATURE)
                .await?;
            if key == BACK {
                walk.steps.push(BACK.into());
                walk.backs += 1;
                left.push((topic, of));
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
fn view(topics: &[&Topic], left: &[(&Topic, Vec<&Phrase>)], open: Option<&Topic>) -> String {
    let mut out = String::from("Menu of messages. Topics:");
    for t in topics {
        let mark = match open {
            Some(o) if o.key == t.key => " <- open now",
            _ if left.iter().any(|(l, _)| l.key == t.key) => " (you selected Back in it)",
            _ => "",
        };
        out += &format!(
            "
- {}: {}{mark}",
            t.key, t.description
        );
    }
    out += "
Your history in this menu, oldest first:";
    let mut n = 0;
    let mut step = |out: &mut String, text: String| {
        n += 1;
        *out += &format!(
            "
{n}. {text}"
        );
    };
    for (t, sentences) in left {
        let seen: Vec<String> = sentences
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
    match open {
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

/// The topics that have a sentence among `phrases`, in catalogue order.
pub fn topics_of(phrases: &[&Phrase]) -> Vec<&'static Topic> {
    TOPICS
        .iter()
        .filter(|t| phrases.iter().any(|p| t.templates.contains(&p.template)))
        .collect()
}

pub fn topic_options(topics: &[&Topic]) -> Vec<(String, String)> {
    topics
        .iter()
        .map(|t| (t.key.to_owned(), t.description.to_owned()))
        .collect()
}

fn of_topic<'a>(phrases: &[&'a Phrase], topic: &Topic) -> Vec<&'a Phrase> {
    phrases
        .iter()
        .copied()
        .filter(|p| topic.templates.contains(&p.template))
        .collect()
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
        let topics = topics_of(&phrases);
        assert_eq!(
            topics.iter().map(|t| t.key).collect::<Vec<_>>(),
            ["greet", "answer", "bye"]
        );
        let answer = topics[1];
        let text = view(
            &topics,
            &[(answer, of_topic(&phrases, answer))],
            Some(topics[2]),
        );
        assert!(
            text.contains(
                "- greet: Greet someone
"
            ),
            "{text}"
        );
        assert!(text.contains("- answer: Answer a question or a request: yes, no, sure, not yet, maybe later, no idea, got it (you selected Back in it)"), "{text}");
        assert!(text.contains("- bye: Say goodbye <- open now"), "{text}");
        assert!(
            text.contains("1. Opened the topic answer, which offered: \"Yes\", \"No\"."),
            "{text}"
        );
        assert!(
            text.contains(
                "2. Selected Back: none of the sentences in answer fit. Returned to the topics."
            ),
            "{text}"
        );
        assert!(text.ends_with("3. Opened the topic bye <- you are here. Pick one of its sentences, or Back to return to the topics (1 Back left)."), "{text}");
        let fresh = view(&topics, &[], None);
        assert!(
            fresh.ends_with(
                "Your history in this menu, oldest first:
1. At the topics <- you are here. Pick a topic."
            ),
            "{fresh}"
        );
    }

    /// A decision model that selects Back whenever it may, else the first option (keys come sorted); the requests it got go to `seen`.
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
            ["answer", "back", "answer", "back", "answer", "\"Yes\""]
        );
        assert_eq!(walk.picked, Some(Picked::Phrase(&p[1])));
        let seen = seen.lock().unwrap();
        // The last request: in answer (the first key) with no Back left, and both Backs in its history
        let last = seen.last().unwrap();
        let state = last["state"].as_str().unwrap();
        assert!(
            state.contains("2. Selected Back: none of the sentences in answer fit."),
            "{state}"
        );
        assert!(
            state.contains("4. Selected Back: none of the sentences in answer fit."),
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
