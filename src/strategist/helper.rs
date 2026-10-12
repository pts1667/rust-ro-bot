//! Small one-shot LLM questions outside the strategist loop: which dialog option to pick, what to say back
//! in chat. Each is a single grammar-constrained request with no history.

use anyhow::{Result, bail};
use serde_json::{Value, json};

use super::llm::Llm;

#[derive(Clone)]
pub struct Helper {
    llm: Llm,
}

const DIALOG_SYSTEM: &str = "You help a game character talk to an NPC in Ragnarok Online. \
Pick the menu option that best serves the character's goal. If none does, pick the one that ends the conversation politely.";

const CHAT_SYSTEM: &str = "You are a friendly adventurer in the game Ragnarok Online. Someone talked to you. \
Answer in one short sentence (under 80 characters), in the language they used. Never share passwords, keys or real-life details.";

impl Helper {
    pub fn new(llm: Llm) -> Self {
        Self { llm }
    }

    /// 1-based index of the option to choose.
    pub async fn choose_dialog_option(&self, goal: &str, lines: &[String], options: &[String]) -> Result<u32> {
        if options.is_empty() {
            bail!("no options");
        }
        let numbered: Vec<String> = options.iter().enumerate().map(|(i, o)| format!("{}. {o}", i + 1)).collect();
        let user = format!(
            "Goal: {goal}\n\nNPC says:\n{}\n\nOptions:\n{}\n\nAnswer with the number of the option.",
            lines.join("\n"),
            numbered.join("\n")
        );
        let ids: Vec<u32> = (1..=options.len() as u32).collect();
        let schema = json!({"type": "object", "properties": {"option": {"type": "integer", "enum": ids}}, "required": ["option"]});
        let v = self.llm.ask_json(DIALOG_SYSTEM, &user, schema, 30).await?;
        match v.get("option").and_then(Value::as_u64) {
            Some(i) if i >= 1 && i as usize <= options.len() => Ok(i as u32),
            _ => bail!("bad option in {v}"),
        }
    }

    /// A short reply to `message` said by `from`, or `None` when the model prefers to stay quiet.
    pub async fn chat_reply(&self, me: &str, from: &str, message: &str, doing: &str) -> Result<Option<String>> {
        let user = format!("You are {me}, currently: {doing}.\n{from} said: \"{message}\"\nReply, or set reply to an empty string to stay quiet.");
        let schema = json!({"type": "object", "properties": {"reply": {"type": "string", "maxLength": 80}}, "required": ["reply"]});
        let v = self.llm.ask_json(CHAT_SYSTEM, &user, schema, 60).await?;
        let reply = v.get("reply").and_then(Value::as_str).unwrap_or("").trim();
        // Chat refuses messages that look like commands.
        let reply = reply.trim_start_matches(['@', '#', '%']).trim();
        Ok((!reply.is_empty()).then(|| reply.chars().take(80).collect()))
    }
}
