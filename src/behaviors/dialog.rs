//! Driving an NPC conversation towards an intent: `next` pages are skipped, text prompts get the bot's name,
//! menus are answered by keyword, then by the LLM helper, then by the safest-looking option.

use anyhow::Result;
use serde_json::Value;

use crate::api::Action;
use crate::api::observation::Dialog;
use crate::bot::Bot;

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
    result.get("dialog").filter(|d| !d.is_null()).and_then(|d| serde_json::from_value(d.clone()).ok())
}

/// Index (1-based) of the first option matching a keyword, keywords in priority order.
pub fn match_option(options: &[String], keywords: &[String]) -> Option<u32> {
    keywords.iter().find_map(|k| {
        let k = k.to_lowercase();
        options.iter().position(|o| o.to_lowercase().contains(&k)).map(|i| i as u32 + 1)
    })
}

/// The option that most likely ends the conversation without commitment.
pub fn safe_option(options: &[String]) -> u32 {
    const EXITS: [&str; 7] = ["cancel", "quit", "end", "no", "leave", "exit", "nothing"];
    EXITS
        .iter()
        .find_map(|k| options.iter().position(|o| o.to_lowercase().split(|c: char| !c.is_alphanumeric()).any(|w| w == *k)))
        .map(|i| i as u32 + 1)
        .unwrap_or(options.len() as u32)
}

impl Bot {
    /// Answers the conversation until it ends, `first` being the dialog the `use` call returned.
    pub async fn run_dialog(&mut self, first: Option<Dialog>, intent: &Intent) -> Result<DialogEnd> {
        let mut dialog = first;
        let mut seen_menus: Vec<String> = Vec::new();
        let mut transcript: Vec<String> = Vec::new();
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
            transcript.extend(d.lines.iter().cloned());
            let kind = d.prompt.as_ref().map(|p| p.kind.clone()).unwrap_or_default();
            let action = match kind.as_str() {
                "next" => Action::DialogNext,
                "text" => Action::DialogText { text: intent.text_answer.clone().unwrap_or_else(|| self.name.clone()) },
                "number" => Action::DialogNumber { value: 1 },
                "running" => {
                    Bot::pause(300).await;
                    continue;
                }
                "shop" | "buy" | "sell" => return Ok(DialogEnd::Shop),
                "menu" => {
                    let options = d.prompt.as_ref().map(|p| p.options.clone()).unwrap_or_default();
                    let key = format!("{:?}{:?}", d.lines, options);
                    let repeats = seen_menus.iter().filter(|k| **k == key).count();
                    seen_menus.push(key);
                    if repeats >= 2 || options.is_empty() {
                        self.act(Action::DialogClose).await?;
                        return Ok(DialogEnd::GaveUp(format!("menu repeats: {options:?}")));
                    }
                    let option = self.choose_option(&transcript, &options, intent, repeats).await;
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

    async fn choose_option(&mut self, transcript: &[String], options: &[String], intent: &Intent, repeats: usize) -> u32 {
        if repeats == 0
            && let Some(i) = match_option(options, &intent.keywords)
        {
            return i;
        }
        if let Some(helper) = &self.helper {
            let recent: Vec<String> = transcript.iter().rev().take(12).rev().cloned().collect();
            match helper.choose_dialog_option(&intent.goal, &recent, options).await {
                Ok(i) => return i,
                Err(error) => tracing::warn!(%error, "LLM helper could not pick a dialog option"),
            }
        }
        safe_option(options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn keywords_pick_in_priority_order() {
        let options = opts(&["Training Grounds.", "Direct access to Ragnarok Online.", "Cancel"]);
        assert_eq!(match_option(&options, &["direct access".into()]), Some(2));
        assert_eq!(match_option(&options, &["nope".into(), "cancel".into()]), Some(3));
        assert_eq!(match_option(&options, &["nope".into()]), None);
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
