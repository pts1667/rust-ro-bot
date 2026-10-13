//! Small one-shot LLM questions outside the strategist loop: which dialog option to pick, what to say back
//! in chat. Each is a single grammar-constrained request; chat gets the last lines of the conversation in it.

use anyhow::{Result, bail};
use serde_json::{Value, json};

use super::llm::Llm;
use crate::goap::goals::HelpTask;
use crate::social::Ask;

#[derive(Clone)]
pub struct Helper {
    llm: Llm,
}

const DIALOG_SYSTEM: &str = "You help a game character talk to an NPC in Ragnarok Online. \
Pick the menu option that best serves the character's goal. If none does, pick the one that ends the conversation politely.";

/// How the bot talks, for both chat prompts.
macro_rules! chat_style {
    () => {
        "Reply to them directly (\"you\"), in one short sentence of at most 15 words in their language, or with an empty \
string when nothing needs saying (a goodbye, a thanks, talk that is over). About the game, say only what FACTS or your \
own state tell, and say you don't know otherwise. If asked whether you are a bot, answer honestly. Never share \
passwords, keys or real-life details."
    };
}

/// Reads what they ask for; the answer to a request comes once the strategist decided (`ANSWER_SYSTEM`).
const CHAT_SYSTEM: &str = concat!(
    "You play a character in the game Ragnarok Online and chat like a friendly player. \
Answer the last line of the conversation.
Set ask to what they want you to do, only when they clearly ask for it: follow (come along with them), invite_them \
(invite them to your party), join_their_party (join their party), give_potions (give them healing potions), \
hunt_together (hunt on a map with them: set map to its name, like prt_fild08). Otherwise ask is none and map is empty. \
When ask is not none, leave reply empty: you answer once you decided.
",
    chat_style!()
);

const ANSWER_SYSTEM: &str = concat!(
    "You play a character in the game Ragnarok Online and chat like a friendly player. \
Answer the last line of the conversation.
",
    chat_style!()
);

/// What the bot knows when it answers someone in chat.
pub struct ChatContext {
    pub me: String,
    /// The bot's level, job, map and what it does, in a few words.
    pub about_me: String,
    pub from: String,
    pub channel: &'static str,
    /// The conversation, oldest first, as `name: text` (`you: text` for the bot).
    pub lines: Vec<String>,
    pub facts: Vec<String>,
    /// What the bot decided about their request, to tell them.
    pub decision: Option<String>,
}

/// The answer: what they ask for, if anything, and what to say.
#[derive(Debug, Default)]
pub struct ChatTurn {
    pub ask: Option<Ask>,
    pub reply: Option<String>,
}

/// The ask of the model's answer.
fn ask_of(kind: &str, map: &str) -> Option<Ask> {
    Some(match kind {
        "follow" => Ask::Help(HelpTask::Follow),
        "invite_them" => Ask::Help(HelpTask::Invite),
        "join_their_party" => Ask::Help(HelpTask::JoinParty),
        "give_potions" => Ask::Help(HelpTask::GivePotions),
        "hunt_together" if !map.trim().is_empty() => Ask::Hunt(map.trim().to_lowercase()),
        _ => return None,
    })
}

/// Longest reply, in characters.
const REPLY_MAX: usize = 120;

/// A line the bot can say: no command prefix (chat refuses them), not empty. A reply the length limit cut ends at its
/// last full sentence.
fn clean(reply: &str) -> Option<String> {
    // The game client shows no emoji
    let reply: String = reply.chars().filter(|c| !matches!(*c as u32, 0x2600..=0x27BF | 0x1F000..=0x1FAFF | 0xFE0F)).collect();
    let reply = reply.trim().trim_start_matches(['@', '#', '%']).trim();
    let reply: String = reply.chars().take(REPLY_MAX).collect();
    let reply = if reply.chars().count() >= REPLY_MAX - 1 {
        match reply.rfind(['.', '!', '?']) {
            Some(end) => reply[..=end].to_owned(),
            None => reply,
        }
    } else {
        reply
    };
    (!reply.is_empty()).then_some(reply)
}

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

    /// What `context.from` asks for in its last line, if anything, and what to answer. Without `asks`, only the
    /// answer: the request the model saw was not one.
    pub async fn chat(&self, context: &ChatContext, asks: bool) -> Result<ChatTurn> {
        let facts = if context.facts.is_empty() { "none".to_owned() } else { context.facts.iter().map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n") };
        let user = format!(
            "You are {}, {}.\n\nFACTS:\n{facts}\n\nConversation with {} ({}), oldest first:\n{}",
            context.me,
            context.about_me,
            context.from,
            context.channel,
            context.lines.join("\n")
        );
        let user = match &context.decision {
            Some(decision) => format!("{user}\n\nAbout their request, you decided: {decision} Tell them so in your answer."),
            None => user,
        };
        if !asks {
            let schema = json!({"type": "object", "properties": {"reply": {"type": "string", "maxLength": 120}}, "required": ["reply"]});
            let v = self.llm.ask_json(ANSWER_SYSTEM, &user, schema, 80).await?;
            return Ok(ChatTurn { ask: None, reply: clean(v.get("reply").and_then(Value::as_str).unwrap_or("")) });
        }
        // Keys come out in alphabetical order: what they ask for is settled before the reply is written.
        let schema = json!({
            "type": "object",
            "properties": {
                "ask": {"type": "string", "enum": ["none", "follow", "invite_them", "join_their_party", "give_potions", "hunt_together"]},
                "map": {"type": "string", "maxLength": 24},
                "reply": {"type": "string", "maxLength": 120}
            },
            "required": ["ask", "map", "reply"]
        });
        let v = self.llm.ask_json(CHAT_SYSTEM, &user, schema, 100).await?;
        let text = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or("");
        Ok(ChatTurn { ask: ask_of(text("ask"), text("map")), reply: clean(text("reply")) })
    }
}
