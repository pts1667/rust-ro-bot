//! How the bot talks: with the LLM, in its own words, or with the decision model (`systemone`), which picks
//! sentences of a fixed catalogue (`phrase`) filled with what the bot knows (`known`). One of them is used per run
//! (`--voice`).

#[cfg(test)]
mod eval;
pub mod known;
pub mod menu;
pub mod phrase;
pub mod systemone;

use anyhow::Result;

use crate::strategist::llm::Llm;
use known::Known;
use menu::{Menu, Picked, topic_options, topics_of};
use phrase::{CONJUNCTIONS, Category, Phrase, conjunction_description, join};
use systemone::{Question, SystemOne};

/// Which model talks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum VoiceMode {
    /// The chat LLM: free text, and the LLM strategist.
    #[default]
    Llm,
    /// The decision model: sentences from the catalogue; the strategist uses its deterministic choice.
    SystemOne,
}

/// The model the bot talks with.
#[derive(Clone)]
pub enum Voice {
    Llm(Llm),
    SystemOne(SystemOne),
}

/// How sharp the draws are: the second sentence's topic and the conjunction stay close to the model's pick.
const TOPIC_TEMPERATURE: f64 = 0.5;
const CONJUNCTION_TEMPERATURE: f64 = 0.5;
/// Saying nothing, a choice next to the topics.
pub const SILENCE: &str =
    "Say nothing: nothing needs an answer, or the talk is over (you already said goodbye)";
/// The decision model's chance of "needs a second sentence" above which the message gets one.
const MORE_GATE: f64 = 0.5;
/// Whether `Other...` (typing letter by letter) is offered: off, this model cannot spell (see PLAN.md).
const TYPING: bool = false;
/// Typing goes close to the model's best letter: a letter drawn by luck spoils the word.
const LETTER_TEMPERATURE: f64 = 0.2;
/// Longest typed message, in characters: one request per character.
const TYPED_MAX: usize = 60;
/// The keys a typed character is picked by, with the character, and the key that ends the message.
const LETTERS: &str = "abcdefghijklmnopqrstuvwxyz0123456789";
const MARKS: [(&str, char); 6] = [
    ("space", ' '),
    ("comma", ','),
    ("period", '.'),
    ("question", '?'),
    ("exclamation", '!'),
    ("apostrophe", '\''),
];
const END: &str = "end";

/// What the bot decided to say.
#[derive(Debug, Clone)]
pub struct Message {
    pub text: String,
    /// Typed letter by letter (`Other...`), not built from the catalogue: nothing makes it true.
    pub typed: bool,
}

impl Message {
    fn built(text: String) -> Self {
        Message { text, typed: false }
    }
}

/// Builds a message from `candidates`, for the situation in `state`: the model goes through the menu (`menu`: a
/// topic, a sentence of it, Back to the topics) for the first sentence, then says whether to add a second one and
/// about what, goes through the menu again from there, and last picks the conjunction that joins them. `purpose` says
/// what the message is for ("answer Bravo"). `only` keeps the candidates to some categories. With `quiet`, saying
/// nothing is a choice next to the topics. As a last resort, when no sentence fits, the model may pick `Other...` and
/// type the message itself (`type_message`). `None` when there is nothing to say.
pub async fn compose(
    s1: &SystemOne,
    state: &str,
    purpose: &str,
    candidates: &[Phrase],
    only: Option<&[Category]>,
    quiet: bool,
) -> Result<Option<Message>> {
    let offered: Vec<&Phrase> = candidates
        .iter()
        .filter(|p| {
            p.category() != Category::Lesson && only.is_none_or(|only| only.contains(&p.category()))
        })
        .collect();
    let mut extra = Vec::new();
    if quiet {
        extra.push(("silence".to_owned(), SILENCE.to_owned()));
    }
    if TYPING {
        extra.push((
            "other".to_owned(),
            "Other...: none of these kinds fits, so type your own words letter by letter. Very slow: only as a last resort"
                .to_owned(),
        ));
    }
    let menu = Menu {
        phrases: offered.clone(),
        extra,
        ask_topic: format!("What kind of message fits best to {purpose}?"),
        ask_sentence: format!("Which message should be sent to {purpose}?"),
        best: false,
    };
    let walk = menu.walk(s1, state, None).await?;
    tracing::debug!(target: "voice", steps = %walk.steps.join(" > "), "first sentence");
    let first = match walk.picked {
        None => return Ok(None),
        Some(Picked::Extra(key)) if key == "other" => {
            return type_message(s1, state, purpose).await;
        }
        Some(Picked::Extra(_)) => return Ok(None),
        Some(Picked::Phrase(p)) => p.clone(),
    };
    // A second sentence that says something (no greeting, short reply or goodbye), then the conjunction that joins them
    let rest: Vec<&Phrase> = offered
        .iter()
        .copied()
        .filter(|p| {
            **p != first
                && !matches!(
                    p.category(),
                    Category::Greeting | Category::Reply | Category::Farewell
                )
        })
        .collect();
    let rest_topics = topics_of(&rest);
    if rest_topics.is_empty() || first.category() == Category::Farewell {
        return Ok(Some(Message::built(first.render())));
    }
    let so_far = format!("{state}\nThe message so far: \"{}\"", first.render());
    let answers = s1
        .ask(
            &so_far,
            &[
                (
                    "more",
                    Question::noul(format!(
                        "Does the message need a second sentence, to {purpose}?"
                    )),
                ),
                (
                    "topic",
                    Question::choice(
                        format!("What would a second sentence be about, to {purpose}?"),
                        topic_options(&rest_topics),
                    ),
                ),
            ],
        )
        .await?;
    let more = answers.get("more").map(|a| a.yes()).unwrap_or(0.0);
    let topic = answers
        .get("topic")
        .and_then(|a| a.sample(TOPIC_TEMPERATURE))
        .and_then(|k| rest_topics.iter().find(|t| t.key == k).copied());
    let Some(topic) = topic.filter(|_| more >= MORE_GATE) else {
        return Ok(Some(Message::built(first.render())));
    };
    let menu = Menu {
        phrases: rest,
        extra: Vec::new(),
        ask_topic: format!("What would a second sentence be about, to {purpose}?"),
        ask_sentence: format!("Which sentence would come next, to {purpose}?"),
        best: false,
    };
    let walk = menu.walk(s1, &so_far, Some(topic)).await?;
    tracing::debug!(target: "voice", steps = %walk.steps.join(" > "), "second sentence");
    let Some(Picked::Phrase(second)) = walk.picked else {
        return Ok(Some(Message::built(first.render())));
    };
    let both = format!(
        "{state}\nThe message: \"{}\", then \"{}\"",
        first.render(),
        second.render()
    );
    let conjunctions = CONJUNCTIONS
        .iter()
        .filter(|(k, _)| *k != "none")
        .map(|(k, _)| ((*k).to_owned(), conjunction_description(k).to_owned()))
        .collect();
    let conjunction = s1
        .choose(
            &both,
            "Which word joins the two sentences best?",
            conjunctions,
            CONJUNCTION_TEMPERATURE,
        )
        .await?;
    Ok(Some(Message::built(join(
        &first,
        &conjunction,
        Some(second),
    ))))
}

/// The `Other...` way: the model types the message one character per request, from the letters, digits and a few
/// marks, until it picks the end (or `TYPED_MAX` characters). Slow: a request per character.
async fn type_message(s1: &SystemOne, state: &str, purpose: &str) -> Result<Option<Message>> {
    let started = std::time::Instant::now();
    let mut text = String::new();
    while text.chars().count() < TYPED_MAX {
        // Only what can come next: a mark after a letter or a digit, a space after a mark, the end once a word is there
        let last = text.chars().last();
        let mut options: Vec<(String, String)> = LETTERS
            .chars()
            .map(|c| (c.to_string(), format!("the character {c}")))
            .collect();
        for (key, c) in MARKS {
            if add_character(&text, key).is_some() {
                options.push((key.to_owned(), format!("the character {c:?}")));
            }
        }
        if last.is_some_and(|l| l != ' ') {
            options.push((END.into(), "The message is complete: stop typing".into()));
        }
        // The unfinished text goes in the question itself: the model continues a word better than when the state alone
        // holds it (5 of 6 next characters right in a probe, against 4)
        let typing = format!(
            "{state}\nNo ready-made sentence fits, so you type your own short message to {purpose}, one character at a time. It is not finished yet."
        );
        let question = if text.is_empty() {
            "Which character does your message start with?".to_owned()
        } else {
            format!("Your unfinished message so far is \"{text}\". Which character comes next?")
        };
        let key = s1
            .choose(&typing, &question, options, LETTER_TEMPERATURE)
            .await?;
        match add_character(&text, &key) {
            Some(next) => text = next,
            None => break,
        }
    }
    let text = text.trim().to_owned();
    let ok = readable(&text, state);
    tracing::info!(target: "voice", %text, ok, millis = started.elapsed().as_millis() as u64, "typed a message");
    Ok((ok && !text.is_empty()).then_some(Message { text, typed: true }))
}

/// Words a typed message may use besides those of the state and the catalogue.
const COMMON_WORDS: &str = "i you we they he she it is am are was be not no yes my your our like love the a an and but so or to of in on at for with too also don't dont know think maybe sure thanks thank sorry ok okay good nice cool great what where how why who when here there now later me this that hi hello bye lol";

/// Whether a typed message reads as words, not letters strung by luck: most of its words are known (from the state,
/// the catalogue or common words), and no word comes more than twice in a row.
fn readable(text: &str, state: &str) -> bool {
    let apart = |c: char| !(c.is_alphanumeric() || c == '\'' || c == '_');
    let words: Vec<String> = text
        .split(apart)
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    if words.is_empty() {
        return false;
    }
    if words.windows(3).any(|w| w[0] == w[1] && w[1] == w[2]) {
        return false;
    }
    let state = state.to_lowercase();
    let known = |w: &str| {
        COMMON_WORDS.split_whitespace().any(|c| c == w)
            || state.split(apart).any(|s| s == w)
            || phrase::TEMPLATES.iter().any(|t| {
                t.text
                    .to_lowercase()
                    .split(|c: char| !c.is_alphanumeric())
                    .any(|s| s == w)
            })
            || w.chars().all(|c| c.is_ascii_digit())
    };
    let known_words = words.iter().filter(|w| known(w)).count();
    known_words * 3 >= words.len() * 2
}

/// `text` with the character of `key` added, capitalised at the start of a sentence; `None` for the end key, an unknown
/// key, or a mark that cannot come there (first, or after a space or another mark; a space may follow a mark).
fn add_character(text: &str, key: &str) -> Option<String> {
    let c = match MARKS.iter().find(|(k, _)| *k == key) {
        Some((_, c)) => *c,
        None if key.chars().count() == 1 && LETTERS.contains(key) => key.chars().next()?,
        None => return None,
    };
    let last = text.chars().last();
    if !c.is_alphanumeric() && !last.is_some_and(|l| l.is_alphanumeric() || (c == ' ' && l != ' '))
    {
        return None;
    }
    let sentence_start = text.trim_end().is_empty() || text.trim_end().ends_with(['.', '?', '!']);
    let c = if sentence_start && c.is_alphabetic() {
        c.to_ascii_uppercase()
    } else {
        c
    };
    Some(format!("{text}{c}"))
}

/// The state the decision model reads about the bot: who it is and what it has done.
pub fn state_of(known: &Known) -> String {
    let experience = known.experience();
    if experience.is_empty() {
        known.about()
    } else {
        format!(
            "{}
{experience}",
            known.about()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_characters_make_words_and_marks_go_after_them() {
        let mut text = String::new();
        for key in [
            "h", "i", "space", "a", "n", "n", "period", "space", "o", "k", "question",
        ] {
            text = add_character(&text, key).unwrap();
        }
        assert_eq!(text, "Hi ann. Ok?");
        assert_eq!(add_character("", "space"), None, "no space first");
        assert_eq!(add_character("ok ", "space"), None, "no two spaces");
        assert_eq!(add_character("ok?", "period"), None, "no two marks");
        assert_eq!(add_character("ok", END), None);
        assert_eq!(add_character("ok", "ä"), None);
        assert_eq!(add_character("lvl", "1").as_deref(), Some("lvl1"));
    }

    #[test]
    fn typed_gibberish_is_not_sent() {
        let state = "Bravo: what's your favourite color?";
        assert!(readable("Blue, I think", state));
        assert!(readable("No idea, sorry", state));
        assert!(
            !readable("A a a a a a b b c a a aa", state),
            "a letter strung on"
        );
        assert!(!readable("Xqzt brlk", state), "no words");
        assert!(!readable("", state));
    }

    /// What the model types when it must: `cargo test typing_on_the_decision -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn typing_on_the_decision_model() {
        let s1 = SystemOne::new("http://127.0.0.1:9931").unwrap();
        let state = "Elara is a level 9 Novice on prt_fild08, now: hunt on prt_fild08.

Conversation with Bravo (party chat):
Bravo: what's your favourite color?";
        for _ in 0..2 {
            let started = std::time::Instant::now();
            let m = type_message(&s1, state, "answer Bravo's last line")
                .await
                .unwrap();
            println!(
                "typed {:?} in {} ms",
                m.map(|m| m.text),
                started.elapsed().as_millis()
            );
        }
    }
}
