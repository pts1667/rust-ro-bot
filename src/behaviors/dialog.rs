//! Driving an NPC conversation towards an intent: `next` pages are skipped, text prompts get the bot's name,
//! menus are answered by keyword, then by the LLM helper, then by the safest-looking option.

use anyhow::Result;
use serde_json::Value;

use crate::api::Action;
use crate::api::observation::Dialog;
use crate::bot::Bot;
use crate::quests::normalize;

const MAX_DIALOG_STEPS: usize = 60;

/// What the bot wants from a conversation.
#[derive(Debug, Clone, Default)]
pub struct Intent {
    /// What the bot wants, in words, for the LLM helper.
    pub goal: String,
    /// Menu entries containing one of these (case-insensitive) are picked first, in this order.
    pub keywords: Vec<String>,
    /// Answer to text prompts; the bot's name when empty.
    pub text_answer: Option<String>,
    /// Quiz answers: when a line said since the last menu contains the first text, the entry matching the second
    /// is picked, or the second is typed into a text or number prompt (`{name}`: the bot's name).
    pub answers: Vec<(String, String)>,
    /// Menus ask to say back what the NPC said before the first one, line by line (a pledge to recite).
    pub recite: bool,
    /// Talking to an unknown NPC to see what it does: menus get the entries that ask for more (`curious_option`), a
    /// number prompt (an amount to pay or buy) ends the talk.
    pub curious: bool,
}

/// The entry that says the next line of what was heard (`heard[from..]`), and where the heard lines go on after it.
pub fn recite_next(heard: &[String], from: usize, options: &[String]) -> Option<(u32, usize)> {
    let clean = |s: &str| {
        normalize(s)
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_owned()
    };
    heard.iter().enumerate().skip(from).find_map(|(i, line)| {
        let line = clean(line);
        options
            .iter()
            .position(|o| !line.is_empty() && clean(o) == line)
            .map(|o| (o as u32 + 1, i + 1))
    })
}

#[derive(Debug)]
pub enum DialogEnd {
    /// The NPC closed the conversation.
    Closed,
    /// The NPC opened a shop: the caller handles it.
    Shop,
    /// The conversation went in circles or too long and was closed.
    GaveUp(String),
}

pub fn dialog_of(result: &Value) -> Option<Dialog> {
    result
        .get("dialog")
        .filter(|d| !d.is_null())
        .and_then(|d| serde_json::from_value(d.clone()).ok())
}

/// Index (1-based) of the first option matching a keyword, keywords in priority order.
pub fn match_option(options: &[String], keywords: &[String]) -> Option<u32> {
    keywords.iter().find_map(|k| {
        let k = k.to_lowercase();
        options
            .iter()
            .position(|o| o.to_lowercase().contains(&k))
            .map(|i| i as u32 + 1)
    })
}

/// The answer of `answers` whose question is in `lines` (a question may span several lines), the one asked last
/// when several are.
#[cfg(test)]
pub fn known_answer<'a>(lines: &[String], answers: &'a [(String, String)]) -> Option<&'a str> {
    known_answers(lines, answers).into_iter().next()
}

/// Every answer whose question is in `lines`, the question asked last first: quizzes ask alike questions with
/// other choices, so the right one is the first among the choices.
pub fn known_answers<'a>(lines: &[String], answers: &'a [(String, String)]) -> Vec<&'a str> {
    let text = normalize(&lines.join(" "));
    let mut found: Vec<(usize, &str)> = answers
        .iter()
        .filter_map(|(q, a)| text.rfind(&normalize(q)).map(|at| (at, a.as_str())))
        .collect();
    found.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    found.into_iter().map(|(_, a)| a).collect()
}

/// The value of the last sum asked (`(47 * 28) - (1376 / 4) = ?`), worked out as anyone would.
pub fn sum_asked(lines: &[String]) -> Option<i64> {
    lines.iter().rev().find_map(|line| {
        let (expr, rest) = line.split_once('=')?;
        if !rest.trim().starts_with('?') {
            return None;
        }
        let tokens: Vec<char> = expr.chars().filter(|c| !c.is_whitespace()).collect();
        let mut at = 0;
        let value = sum(&tokens, &mut at)?;
        (at == tokens.len()).then_some(value)
    })
}

fn sum(t: &[char], at: &mut usize) -> Option<i64> {
    let mut value = product(t, at)?;
    while let Some(&op) = t.get(*at).filter(|c| matches!(c, '+' | '-')) {
        *at += 1;
        let next = product(t, at)?;
        value = if op == '+' {
            value + next
        } else {
            value - next
        };
    }
    Some(value)
}

fn product(t: &[char], at: &mut usize) -> Option<i64> {
    let mut value = factor(t, at)?;
    while let Some(&op) = t.get(*at).filter(|c| matches!(c, '*' | 'x' | '/')) {
        *at += 1;
        let next = factor(t, at)?;
        value = if op == '/' {
            value.checked_div(next)?
        } else {
            value * next
        };
    }
    Some(value)
}

fn factor(t: &[char], at: &mut usize) -> Option<i64> {
    if t.get(*at) == Some(&'(') {
        *at += 1;
        let value = sum(t, at)?;
        (t.get(*at) == Some(&')')).then(|| *at += 1)?;
        return Some(value);
    }
    let start = *at;
    while t.get(*at).is_some_and(|c| c.is_ascii_digit() || *c == ',') {
        *at += 1;
    }
    t[start..*at]
        .iter()
        .filter(|c| **c != ',')
        .collect::<String>()
        .parse()
        .ok()
}

/// The one entry (1-based) whose word can be spelled with the letters of the last puzzle line (`t m y a n y e o b`).
pub fn hidden_word(lines: &[String], options: &[String]) -> Option<u32> {
    let letters: Vec<char> = lines.iter().rev().find_map(|line| {
        let parts: Vec<&str> = line.split_whitespace().collect();
        (parts.len() >= 8
            && parts
                .iter()
                .all(|p| p.len() == 1 && p.chars().all(|c| c.is_ascii_alphabetic())))
        .then(|| {
            parts
                .iter()
                .flat_map(|p| p.chars())
                .map(|c| c.to_ascii_lowercase())
                .collect()
        })
    })?;
    let fits = |word: &str| {
        let mut left = letters.clone();
        word.chars()
            .filter(|c| c.is_ascii_alphabetic())
            .map(|c| c.to_ascii_lowercase())
            .all(|c| match left.iter().position(|l| *l == c) {
                Some(i) => {
                    left.swap_remove(i);
                    true
                }
                None => false,
            })
    };
    let found: Vec<usize> = (0..options.len()).filter(|&i| fits(&options[i])).collect();
    (found.len() == 1).then(|| found[0] as u32 + 1)
}

/// The line where the highlight (`^3377FF...^000000`) of the page ends, as written: a song to sing along to, the NPC
/// colouring how far it got.
pub fn highlighted_line(lines: &[String]) -> Option<String> {
    let code = |s: &str, at: usize| {
        s.get(at + 1..at + 7)
            .is_some_and(|h| h.chars().all(|c| c.is_ascii_hexdigit()))
    };
    let mut on = false;
    let mut last = None;
    for line in lines {
        let mut text = String::new();
        let mut rest = line.as_str();
        while let Some(at) = rest.find('^') {
            if !code(rest, at) {
                text.push_str(&rest[..=at]);
                rest = &rest[at + 1..];
                continue;
            }
            text.push_str(&rest[..at]);
            let closing = &rest[at + 1..at + 7] == "000000";
            if on && closing {
                last = Some(text.trim().to_owned());
            }
            on = !closing;
            rest = &rest[at + 7..];
        }
    }
    last.filter(|l| !l.is_empty())
}

/// The entry (1-based) that is the answer: equal to it, else containing it.
pub fn option_for(options: &[String], answer: &str) -> Option<u32> {
    let answer = normalize(answer);
    let clean = |o: &String| {
        normalize(o)
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_owned()
    };
    options
        .iter()
        .position(|o| clean(o) == answer.trim_matches(|c: char| !c.is_alphanumeric()))
        .or_else(|| options.iter().position(|o| normalize(o).contains(&answer)))
        .map(|i| i as u32 + 1)
}

/// The option that most likely ends the conversation without commitment.
pub fn safe_option(options: &[String]) -> u32 {
    exit_option(options).unwrap_or(options.len() as u32)
}

/// The entry (1-based) that says no or leaves, if one does.
pub fn exit_option(options: &[String]) -> Option<u32> {
    const EXITS: [&str; 7] = ["cancel", "quit", "end", "no", "leave", "exit", "nothing"];
    EXITS
        .iter()
        .find_map(|k| options.iter().position(|o| has_word(o, k)))
        .map(|i| i as u32 + 1)
}

/// Whether `text` holds the word or words `key`, whole.
fn has_word(text: &str, key: &str) -> bool {
    let words = |s: &str| {
        let w: Vec<String> = s
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_owned)
            .collect();
        format!(" {} ", w.join(" "))
    };
    words(text).contains(&words(key))
}

/// Words of an entry or of the page before a menu that commit the bot to something: money, gear, stats, a job, a
/// trip, its items.
const RISKY: [&str; 44] = [
    "refine",
    "upgrade",
    "reset",
    "job",
    "become",
    "change",
    "warp",
    "teleport",
    "transport",
    "move",
    "go to",
    "send",
    "save",
    "storage",
    "buy",
    "sell",
    "purchase",
    "exchange",
    "trade",
    "pay",
    "zeny",
    "z",
    "donate",
    "gamble",
    "bet",
    "lottery",
    "card",
    "enchant",
    "socket",
    "marry",
    "wedding",
    "divorce",
    "rent",
    "hire",
    "register",
    "guild",
    "delete",
    "repair",
    "identify",
    "appraise",
    "fee",
    "cost",
    "price",
    "give",
];
/// Words of an entry that asks to hear more.
const CURIOUS: [&str; 24] = [
    "quest",
    "who",
    "what",
    "about",
    "ask",
    "talk",
    "tell",
    "story",
    "hear",
    "listen",
    "information",
    "info",
    "explain",
    "how",
    "why",
    "help",
    "hello",
    "hi",
    "chat",
    "rumor",
    "rumour",
    "news",
    "sure",
    "yes",
];
/// Menus answered in one curious talk before leaving.
const CURIOUS_MENUS: usize = 3;

/// The entry (1-based) a curious talk picks, `answered` menus in: one that asks to hear more, unless the page or the
/// entry speaks of money, gear, a job or a trip; else the one that leaves. `None`: no entry leaves, close the talk.
pub fn curious_option(page: &[String], options: &[String], answered: usize) -> Option<u32> {
    let risky = |text: &str| RISKY.iter().any(|k| has_word(text, k));
    if answered < CURIOUS_MENUS
        && !risky(&page.join(" "))
        && let Some(i) = options
            .iter()
            .position(|o| !risky(o) && CURIOUS.iter().any(|k| has_word(o, k)))
    {
        return Some(i as u32 + 1);
    }
    exit_option(options)
}

impl Bot {
    /// Answers the conversation until it ends, `first` being the dialog the `use` call returned.
    pub async fn run_dialog(
        &mut self,
        first: Option<Dialog>,
        intent: &Intent,
    ) -> Result<DialogEnd> {
        let mut dialog = first;
        let mut seen_menus: Vec<String> = Vec::new();
        let mut transcript: Vec<String> = Vec::new();
        // Where the lines said since the last menu answer start: the question of a quiz is among them.
        let mut since_menu = 0;
        // For a recitation: what was said before the first menu, and how far it was said back.
        let mut heard: Option<Vec<String>> = None;
        let mut recited = 0;
        // Menus a curious talk answered.
        let mut curious_menus = 0;
        self.last_dialog.clear();
        self.first_menu.clear();
        for _ in 0..MAX_DIALOG_STEPS {
            let Some(d) = dialog.take() else {
                let obs = self.observe().await?;
                match obs.dialog {
                    Some(d) if d.prompt.is_some() => {
                        dialog = Some(d);
                        continue;
                    }
                    _ => return Ok(DialogEnd::Closed),
                }
            };
            // A page may come as one text of several lines.
            let lines: Vec<String> = d
                .lines
                .iter()
                .flat_map(|l| l.split('\n'))
                .map(|l| l.trim_end().to_owned())
                .filter(|l| !l.is_empty())
                .collect();
            transcript.extend(lines.iter().cloned());
            self.last_dialog.extend(lines);
            let kind = d
                .prompt
                .as_ref()
                .map(|p| p.kind.clone())
                .unwrap_or_default();
            tracing::debug!(lines = ?d.lines, %kind, "dialog page");
            let candidates: Vec<String> = known_answers(&transcript[since_menu..], &intent.answers)
                .iter()
                .map(|a| a.replace("{name}", &self.name))
                .collect();
            let known = candidates.first().cloned();
            let action = match kind.as_str() {
                "next" => Action::DialogNext,
                "text" => {
                    since_menu = transcript.len();
                    let sung = if intent.recite {
                        highlighted_line(&d.lines)
                    } else {
                        None
                    };
                    let text = known
                        .or(sung)
                        .or_else(|| intent.text_answer.clone())
                        .unwrap_or_else(|| self.name.clone());
                    tracing::info!(%text, "dialog text");
                    Action::DialogText { text }
                }
                "number" if intent.curious => {
                    self.act(Action::DialogClose).await?;
                    return Ok(DialogEnd::GaveUp("asked for an amount".into()));
                }
                "number" => {
                    let value = known
                        .and_then(|a| a.trim().parse::<i64>().ok())
                        .or_else(|| sum_asked(&transcript[since_menu..]))
                        .unwrap_or(1);
                    since_menu = transcript.len();
                    tracing::info!(value, "dialog number");
                    Action::DialogNumber { value }
                }
                "running" => {
                    Bot::pause(300).await;
                    continue;
                }
                "shop" | "buy" | "sell" => return Ok(DialogEnd::Shop),
                "menu" => {
                    let options = d
                        .prompt
                        .as_ref()
                        .map(|p| p.options.clone())
                        .unwrap_or_default();
                    if self.first_menu.is_empty() {
                        self.first_menu = options.clone();
                    }
                    let key = format!("{:?}{:?}", d.lines, options);
                    let repeats = seen_menus.iter().filter(|k| **k == key).count();
                    seen_menus.push(key);
                    if repeats >= 2 || options.is_empty() {
                        self.act(Action::DialogClose).await?;
                        return Ok(DialogEnd::GaveUp(format!("menu repeats: {options:?}")));
                    }
                    if intent.curious {
                        let Some(option) =
                            curious_option(&transcript[since_menu..], &options, curious_menus)
                        else {
                            self.act(Action::DialogClose).await?;
                            return Ok(DialogEnd::GaveUp(format!("no way out of {options:?}")));
                        };
                        curious_menus += 1;
                        since_menu = transcript.len();
                        tracing::info!(option, choice = %options[option as usize - 1], "curious dialog menu");
                        match self.act(Action::DialogChoose { option }).await {
                            Ok(result) => dialog = dialog_of(&result),
                            Err(error) => {
                                self.act(Action::DialogClose).await.ok();
                                return Ok(DialogEnd::GaveUp(error.to_string()));
                            }
                        }
                        continue;
                    }
                    let heard = heard.get_or_insert_with(|| transcript.clone());
                    let recital = if intent.recite {
                        recite_next(heard, recited, &options)
                    } else {
                        None
                    };
                    let option = match (
                        recital,
                        candidates.iter().find_map(|a| option_for(&options, a)),
                    ) {
                        (Some((i, next)), _) => {
                            recited = next;
                            i
                        }
                        (None, Some(i)) => i,
                        (None, None) => match hidden_word(&transcript[since_menu..], &options) {
                            Some(i) => i,
                            None => {
                                self.choose_option(&transcript, &options, intent, repeats)
                                    .await
                            }
                        },
                    };
                    since_menu = transcript.len();
                    tracing::info!(option, choice = %options[option as usize - 1], "dialog menu");
                    Action::DialogChoose { option }
                }
                _ => {
                    self.act(Action::DialogClose).await?;
                    return Ok(DialogEnd::Closed);
                }
            };
            match self.act(action).await {
                Ok(result) => dialog = dialog_of(&result),
                Err(error) => {
                    tracing::warn!(%error, "dialog answer refused");
                    self.act(Action::DialogClose).await.ok();
                    return Ok(DialogEnd::GaveUp(error.to_string()));
                }
            }
        }
        self.act(Action::DialogClose).await.ok();
        Ok(DialogEnd::GaveUp("dialogue too long".into()))
    }

    async fn choose_option(
        &mut self,
        transcript: &[String],
        options: &[String],
        intent: &Intent,
        repeats: usize,
    ) -> u32 {
        if repeats == 0
            && let Some(i) = match_option(options, &intent.keywords)
        {
            return i;
        }
        if let Some(helper) = &self.helper {
            let recent: Vec<String> = transcript.iter().rev().take(12).rev().cloned().collect();
            match helper
                .choose_dialog_option(&intent.goal, &recent, options)
                .await
            {
                Ok(i) => return i,
                Err(error) => tracing::warn!(%error, "LLM helper could not pick a dialog option"),
            }
        }
        safe_option(options)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_song_line_sung_last_is_the_highlighted_one() {
        let page = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            highlighted_line(&page(&[
                "^3377FFThere was a man^000000",
                "who was said to be immortal."
            ])),
            Some("There was a man".into())
        );
        assert_eq!(
            highlighted_line(&page(&[
                "^3377FFThere was a man",
                "who was said to be immortal.^000000",
                "His name Jichfreid,"
            ])),
            Some("who was said to be immortal.".into())
        );
        assert_eq!(highlighted_line(&page(&["Nothing coloured here."])), None);
    }

    #[test]
    fn sums_are_worked_out() {
        let ask = |s: &str| sum_asked(&["[Raspuchin Gregory]".to_owned(), s.to_owned()]);
        assert_eq!(ask("12 + 23 + 34 + 45 = ?"), Some(114));
        assert_eq!(ask("1000 - 36 - 227 - 348 = ?"), Some(389));
        assert_eq!(ask("9765 / 3 / 5 / 7 = ?"), Some(93));
        assert_eq!(ask("(47 * 28) - (1376 / 4) = ?"), Some(972));
        assert_eq!(ask("(2646 / 7) + (13 * 28) = ?"), Some(742));
        assert_eq!(ask("What is the total weight?"), None);
    }

    #[test]
    fn the_hidden_word_is_the_one_the_letters_spell() {
        let lines = |s: &str| vec!["[Nicholas Flamel]".to_owned(), s.to_owned()];
        assert_eq!(
            hidden_word(
                &lines("t m y a n y e o b n e g p r i"),
                &opts(&["Brake", "Brass", "Bug", "Broken", "Brigan?"])
            ),
            Some(5)
        );
        assert_eq!(
            hidden_word(
                &lines("g b n o p r e f a r e t a s k"),
                &opts(&["purple", "smoker", "ragnarok", "bolt", "burnt wood"])
            ),
            Some(3)
        );
        assert_eq!(
            hidden_word(
                &lines("r o e h n r o m c a i n p t t"),
                &opts(&[
                    "forgemerchant",
                    "potionmerchant",
                    "dcmerchant",
                    "vendingmerchant",
                    "battlemerchant"
                ])
            ),
            Some(2)
        );
        assert_eq!(hidden_word(&lines("Pick one."), &opts(&["a", "b"])), None);
    }

    use super::*;

    fn opts(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn keywords_pick_in_priority_order() {
        let options = opts(&[
            "Training Grounds.",
            "Direct access to Ragnarok Online.",
            "Cancel",
        ]);
        assert_eq!(match_option(&options, &["direct access".into()]), Some(2));
        assert_eq!(
            match_option(&options, &["nope".into(), "cancel".into()]),
            Some(3)
        );
        assert_eq!(match_option(&options, &["nope".into()]), None);
    }

    #[test]
    fn quiz_answers_follow_the_question() {
        let answers = vec![
            (
                "not affected by the Two Hand Quicken".to_owned(),
                "Flamberge".to_owned(),
            ),
            ("Bowling Bash".to_owned(), "Provoke".to_owned()),
        ];
        let lines = vec!["[Sir Siracuse]".to_owned(), "Which of the following weapons are ^0000FFnot affected by the Two Hand Quicken^000000 skill?".to_owned()];
        let a = known_answer(&lines, &answers).unwrap();
        assert_eq!(
            option_for(&opts(&["Katana", "Slayer", "Broadsword", "Flamberge"]), a),
            Some(4)
        );
        assert_eq!(
            option_for(
                &opts(&["Two Handed Sword Mastery Lv.5", "Provoke Lv.10"]),
                "Provoke"
            ),
            Some(2)
        );
        // An exact entry beats one that merely contains the answer.
        assert_eq!(
            option_for(&opts(&["Level 30", "Level 3"]), "Level 3"),
            Some(2)
        );
        assert_eq!(known_answer(&["Hello".to_owned()], &answers), None);
        // Split over lines, and the latest question wins.
        let answers = vec![
            ("how should you act within".to_owned(), "Protect".to_owned()),
            ("Novice".to_owned(), "hunting".to_owned()),
        ];
        let lines = vec![
            "A Novice? Fine.".to_owned(),
            "Now, how should".to_owned(),
            "you act within".to_owned(),
            "a party?".to_owned(),
        ];
        assert_eq!(known_answer(&lines, &answers), Some("Protect"));
        // Alike questions: the answer that is among the choices.
        let alike = vec![
            (
                "different type than the others".to_owned(),
                "Aster".to_owned(),
            ),
            (
                "different type than the others".to_owned(),
                "Marin".to_owned(),
            ),
        ];
        let asked =
            vec!["4. Choose the monster that is a different type than the others.".to_owned()];
        let options = opts(&["Aster", "Marc", "Marse", "Marin"]);
        assert_eq!(
            known_answers(&asked, &alike)
                .iter()
                .find_map(|a| option_for(&options[1..], a)),
            Some(3)
        );
    }

    #[test]
    fn a_pledge_is_said_back_in_order() {
        let heard: Vec<String> = [
            "[Touha]",
            "Now, pay attention",
            "I seek the path",
            "[Touha]",
            "of enlightenment.",
            "We monks",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let options = opts(&["We monks", "of enlightenment.", "I seek the path"]);
        assert_eq!(recite_next(&heard, 0, &options), Some((3, 3)));
        assert_eq!(recite_next(&heard, 3, &options), Some((2, 5)));
        assert_eq!(recite_next(&heard, 5, &options), Some((1, 6)));
    }

    #[test]
    fn a_curious_talk_asks_for_more_but_commits_to_nothing() {
        let page = |s: &str| vec![s.to_owned()];
        let hello = page("Hello there, traveller.");
        assert_eq!(
            curious_option(&hello, &opts(&["Refine", "Tell me about it", "Cancel"]), 0),
            Some(2)
        );
        // An entry to hear more that would also cost something is not taken
        assert_eq!(
            curious_option(&hello, &opts(&["Ask to warp", "Quit"]), 0),
            Some(2)
        );
        // A page about money or a job: leave
        assert_eq!(
            curious_option(&page("It costs 500 zeny."), &opts(&["Yes", "No"]), 0),
            Some(2)
        );
        assert_eq!(
            curious_option(
                &page("Do you want to become a Swordman?"),
                &opts(&["Yes", "No"]),
                0
            ),
            Some(2)
        );
        // Enough questions: leave
        assert_eq!(
            curious_option(&hello, &opts(&["Who are you?", "End"]), 3),
            Some(2)
        );
        // No entry leaves: the talk is closed
        assert_eq!(
            curious_option(&hello, &opts(&["Prontera", "Geffen"]), 0),
            None
        );
        // Words are whole: "hi" is not in "this", "z" is not in "zone"
        assert_eq!(
            curious_option(
                &page("This zone is quiet."),
                &opts(&["this", "Hi!", "Quit"]),
                0
            ),
            Some(2)
        );
    }

    #[test]
    fn safe_option_prefers_cancel_words() {
        assert_eq!(safe_option(&opts(&["Buy", "Sell", "Cancel"])), 3);
        assert_eq!(safe_option(&opts(&["Save", "No thanks", "Teleport"])), 2);
        assert_eq!(safe_option(&opts(&["A", "B"])), 2);
        // "Endless Tower" must not count as "end".
        assert_eq!(safe_option(&opts(&["Endless Tower", "Prontera"])), 2);
    }
}
