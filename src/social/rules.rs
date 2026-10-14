//! Requests said in plain words, read without the LLM.

use super::Ask;
use crate::goap::goals::HelpTask;

/// What `text` asks the bot to do, when it says so in the usual words.
pub fn ask_of(text: &str) -> Option<Ask> {
    let lower = text.to_lowercase();
    let has = |keys: &[&str]| keys.iter().any(|k| lower.contains(k));
    if has(&["follow me", "come with me", "come along", "walk with me"]) {
        Some(Ask::Help(HelpTask::Follow))
    } else if has(&[
        "invite me",
        "inv me",
        "add me to your party",
        "can i join your party",
        "let me join",
    ]) {
        Some(Ask::Help(HelpTask::Invite))
    } else if has(&[
        "join my party",
        "join our party",
        "join us",
        "i'll invite you",
        "i will invite you",
        "want to party",
        "wanna party",
    ]) {
        Some(Ask::Help(HelpTask::JoinParty))
    } else if has(&["potion", "pots"])
        && has(&[
            "please",
            "pls",
            "plz",
            "can i have",
            "spare",
            "give me",
            "need",
        ])
    {
        Some(Ask::Help(HelpTask::GivePotions))
    } else if has(&["hunt", "come to", "meet me", "let's go", "lets go"]) {
        map_word(&lower).map(Ask::Hunt)
    } else {
        None
    }
}

/// Whether `text` has words that go with `ask`: the small model reads requests into small talk ("how is the hunting
/// going?" taken for an invitation to hunt), so what it finds must show in the words.
pub fn plausible(ask: &Ask, text: &str) -> bool {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .collect();
    let starts = |keys: &[&str]| words.iter().any(|w| keys.iter().any(|k| w.starts_with(k)));
    match ask {
        Ask::Help(HelpTask::Follow) => {
            starts(&["follow", "come", "walk", "escort"]) || lower.contains("with me")
        }
        Ask::Help(HelpTask::Invite) | Ask::Help(HelpTask::JoinParty) => {
            starts(&["invit", "inv", "join", "party", "group"])
                || words.contains(&"pt")
                || lower.contains("add me")
        }
        Ask::Help(HelpTask::GivePotions) => starts(&["potion", "pots", "pot", "heal", "hp"]),
        Ask::Hunt(map) => words.contains(&map.as_str()),
    }
}

/// The first word shaped like a map name (`prt_fild08`).
pub fn map_word(lower: &str) -> Option<String> {
    lower
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .find(|w| {
            w.contains('_')
                && w.len() >= 5
                && w.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        })
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_requests_are_understood() {
        assert_eq!(
            ask_of("Follow me please"),
            Some(Ask::Help(HelpTask::Follow))
        );
        assert_eq!(ask_of("can you inv me?"), Some(Ask::Help(HelpTask::Invite)));
        assert_eq!(
            ask_of("wanna party? I'll invite you"),
            Some(Ask::Help(HelpTask::JoinParty))
        );
        assert_eq!(
            ask_of("I need potions"),
            Some(Ask::Help(HelpTask::GivePotions))
        );
        assert_eq!(
            ask_of("let's hunt at prt_fild08!"),
            Some(Ask::Hunt("prt_fild08".into()))
        );
        assert_eq!(ask_of("let's hunt together"), None);
        assert_eq!(ask_of("nice potion shop here"), None);
        assert_eq!(ask_of("hello"), None);
    }

    #[test]
    fn a_request_must_show_in_the_words() {
        assert!(!plausible(
            &Ask::Hunt("prt_fild08".into()),
            "hey, how is the hunting going?"
        ));
        assert!(plausible(
            &Ask::Hunt("prt_fild08".into()),
            "come hunt on prt_fild08 with me"
        ));
        assert!(plausible(
            &Ask::Help(HelpTask::Follow),
            "can you come along?"
        ));
        assert!(!plausible(
            &Ask::Help(HelpTask::Follow),
            "nice weather today"
        ));
        assert!(plausible(&Ask::Help(HelpTask::JoinParty), "wanna pt?"));
        assert!(plausible(
            &Ask::Help(HelpTask::GivePotions),
            "got any pots to spare?"
        ));
        assert!(!plausible(&Ask::Help(HelpTask::GivePotions), "what a spot"));
    }
}
