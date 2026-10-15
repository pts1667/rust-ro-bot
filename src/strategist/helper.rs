//! Small one-shot questions outside the strategist loop: which dialog option to pick, what to say back in chat. With
//! the LLM each is a single grammar-constrained request, and chat gets the last lines of the conversation in it. With
//! the decision model the bot picks among options and the catalogue's sentences (`voice`).

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::goap::goals::HelpTask;
use crate::social::Ask;
use crate::voice::known::Known;
use crate::voice::phrase::Phrase;
use crate::voice::systemone::{Question, SystemOne};
use crate::voice::{Voice, compose, state_of};

#[derive(Clone)]
pub struct Helper {
    voice: Voice,
}

/// What someone may ask for, for the decision model, with the keys of `ask_of`.
const ASKS: [(&str, &str); 7] = [
    (
        "none",
        "Nothing: small talk, a question, a greeting, a thanks",
    ),
    (
        "bot_question",
        "Nothing to do: they ask whether you are a bot, a program or a real person",
    ),
    ("follow", "Come along with them, follow them"),
    ("invite_them", "Invite them into your party"),
    ("join_their_party", "Join their party"),
    ("give_potions", "Give them healing potions"),
    ("hunt_together", "Hunt together on a map they name"),
];

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
    /// What the bot knows, for the decision model's sentences.
    pub known: Known,
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
    let reply: String = reply
        .chars()
        .filter(|c| !matches!(*c as u32, 0x2600..=0x27BF | 0x1F000..=0x1FAFF | 0xFE0F))
        .collect();
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
    pub fn new(voice: Voice) -> Self {
        Self { voice }
    }

    /// Whether the bot talks with the decision model.
    pub fn decides(&self) -> bool {
        matches!(self.voice, Voice::SystemOne(_))
    }

    /// 1-based index of the option to choose.
    pub async fn choose_dialog_option(
        &self,
        goal: &str,
        lines: &[String],
        options: &[String],
    ) -> Result<u32> {
        if options.is_empty() {
            bail!("no options");
        }
        let llm = match &self.voice {
            Voice::Llm(llm) => llm,
            Voice::SystemOne(s1) => {
                let state = format!(
                    "A game character talks to an NPC in Ragnarok Online. Its goal: {goal}

NPC says:
{}",
                    lines.join(
                        "
"
                    )
                );
                let numbered = options
                    .iter()
                    .enumerate()
                    .map(|(i, o)| ((i + 1).to_string(), o.clone()))
                    .collect();
                let pick = s1
                    .choose(&state, "Which menu option best serves the goal? If none does, the one that ends the talk politely.", numbered, 0.05)
                    .await?;
                return pick
                    .parse::<u32>()
                    .map_err(|_| anyhow::anyhow!("bad option {pick}"));
            }
        };
        let numbered: Vec<String> = options
            .iter()
            .enumerate()
            .map(|(i, o)| format!("{}. {o}", i + 1))
            .collect();
        let user = format!(
            "Goal: {goal}\n\nNPC says:\n{}\n\nOptions:\n{}\n\nAnswer with the number of the option.",
            lines.join("\n"),
            numbered.join("\n")
        );
        let ids: Vec<u32> = (1..=options.len() as u32).collect();
        let schema = json!({"type": "object", "properties": {"option": {"type": "integer", "enum": ids}}, "required": ["option"]});
        let v = llm.ask_json(DIALOG_SYSTEM, &user, schema, 30).await?;
        match v.get("option").and_then(Value::as_u64) {
            Some(i) if i >= 1 && i as usize <= options.len() => Ok(i as u32),
            _ => bail!("bad option in {v}"),
        }
    }

    /// The NPC (index in `npcs`, their lines) a character exploring should talk to first, `situation` telling who and
    /// where it is.
    pub async fn npc_to_meet(&self, situation: &str, npcs: &[String]) -> Result<usize> {
        if npcs.is_empty() {
            bail!("no NPCs");
        }
        const ASK: &str = "Which NPC should the character talk to first, to discover a quest or something worth telling other players?";
        let llm = match &self.voice {
            Voice::Llm(llm) => llm,
            Voice::SystemOne(s1) => {
                let numbered = npcs
                    .iter()
                    .enumerate()
                    .map(|(i, n)| ((i + 1).to_string(), n.clone()))
                    .collect();
                let pick = s1.choose(situation, ASK, numbered, 0.3).await?;
                return pick
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| i.checked_sub(1))
                    .ok_or_else(|| anyhow::anyhow!("bad NPC {pick}"));
            }
        };
        let numbered: Vec<String> = npcs
            .iter()
            .enumerate()
            .map(|(i, n)| format!("{}. {n}", i + 1))
            .collect();
        let user = format!(
            "{situation}\n\nNPCs it has not met:\n{}\n\n{ASK} Answer with its number.",
            numbered.join("\n")
        );
        let ids: Vec<usize> = (1..=npcs.len()).collect();
        let schema = json!({"type": "object", "properties": {"npc": {"type": "integer", "enum": ids}}, "required": ["npc"]});
        let v = llm
            .ask_json(
                "You help a game character explore Ragnarok Online.",
                &user,
                schema,
                30,
            )
            .await?;
        match v.get("npc").and_then(Value::as_u64) {
            Some(i) if i >= 1 && i as usize <= npcs.len() => Ok(i as usize - 1),
            _ => bail!("bad NPC in {v}"),
        }
    }

    /// What `context.from` asks for in its last line, if anything, and what to answer. Without `asks`, only the
    /// answer: the request the model saw was not one.
    pub async fn chat(&self, context: &ChatContext, asks: bool) -> Result<ChatTurn> {
        let llm = match &self.voice {
            Voice::Llm(llm) => llm,
            Voice::SystemOne(s1) => return decide_chat(s1, context, asks).await,
        };
        let facts = if context.facts.is_empty() {
            "none".to_owned()
        } else {
            context
                .facts
                .iter()
                .map(|f| format!("- {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let user = format!(
            "You are {}, {}.\n\nFACTS:\n{facts}\n\nConversation with {} ({}), oldest first:\n{}",
            context.me,
            context.about_me,
            context.from,
            context.channel,
            context.lines.join("\n")
        );
        let user = match &context.decision {
            Some(decision) => format!(
                "{user}\n\nAbout their request, you decided: {decision} Tell them so in your answer."
            ),
            None => user,
        };
        if !asks {
            let schema = json!({"type": "object", "properties": {"reply": {"type": "string", "maxLength": 120}}, "required": ["reply"]});
            let v = llm.ask_json(ANSWER_SYSTEM, &user, schema, 80).await?;
            return Ok(ChatTurn {
                ask: None,
                reply: clean(v.get("reply").and_then(Value::as_str).unwrap_or("")),
            });
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
        let v = llm.ask_json(CHAT_SYSTEM, &user, schema, 100).await?;
        let text = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or("");
        Ok(ChatTurn {
            ask: ask_of(text("ask"), text("map")),
            reply: clean(text("reply")),
        })
    }
}

/// `Helper::chat` with the decision model: what they ask for, then a message of the catalogue or silence. The
/// answer to a request the strategist decided on is the fixed words of the caller.
async fn decide_chat(s1: &SystemOne, context: &ChatContext, asks: bool) -> Result<ChatTurn> {
    if context.decision.is_some() {
        return Ok(ChatTurn::default());
    }
    let (me, from) = (&context.me, &context.from);
    let state = format!(
        "{}

Conversation with {from} ({}), oldest first (\"you\" is {me}):
{}",
        state_of(&context.known),
        context.channel,
        context.lines.join(
            "
"
        )
    );
    let said = context
        .lines
        .last()
        .and_then(|l| l.split_once(": "))
        .map(|(_, t)| t.to_lowercase())
        .unwrap_or_default();
    let honest = || ChatTurn {
        ask: None,
        reply: Some(Phrase::new("bot", &[]).render()),
    };
    // Asked whether it is a bot, it says so, whatever the model would draw
    if asks_if_bot(&said) {
        return Ok(honest());
    }
    if asks {
        let options = ASKS
            .iter()
            .map(|(k, d)| ((*k).to_owned(), (*d).to_owned()))
            .collect();
        let question = Question::choice(
            format!("What does {from} ask {me} to do in the last line?"),
            options,
        );
        let answers = s1.ask(&state, &[("ask", question)]).await?;
        let kind = answers
            .get("ask")
            .and_then(|a| a.choice.as_deref())
            .unwrap_or("none");
        if kind == "bot_question" {
            return Ok(honest());
        }
        let map = crate::social::rules::map_word(&said).unwrap_or_default();
        if let Some(ask) = ask_of(kind, &map) {
            return Ok(ChatTurn {
                ask: Some(ask),
                reply: None,
            });
        }
    }
    let message = compose(
        s1,
        &state,
        &format!("answer {from}'s last line"),
        &context.known.candidates(),
        None,
        true,
    )
    .await?;
    Ok(ChatTurn {
        ask: None,
        reply: message.and_then(|m| clean(&m.text)),
    })
}

/// Whether a line asks if the bot is a bot.
fn asks_if_bot(lower: &str) -> bool {
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).collect();
    words.iter().any(|w| {
        matches!(
            *w,
            "bot" | "bots" | "botting" | "ai" | "npc" | "program" | "human"
        )
    }) && lower.contains('?')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::social::facts::Lore;

    fn context(from: &str, lines: &[&str]) -> ChatContext {
        let known = Known {
            me: "Elara".into(),
            level: 9,
            job: "Novice".into(),
            map: "prt_fild08".into(),
            doing: "hunt on prt_fild08".into(),
            hunt_map: Some("prt_fild08".into()),
            hunting: true,
            in_sight: vec!["Poring".into(), "Lunatic".into()],
            kills: vec![
                ("Poring".into(), 40),
                ("Lunatic".into(), 12),
                ("Fabre".into(), 3),
            ],
            deaths: vec![("Fabre".into(), 2)],
            journal: vec!["reached base level 9 on prt_fild08".into()],
            other: Some(from.into()),
            lore: vec![Lore::Monster {
                name: "Fabre".into(),
                level: 2,
                hp: 63,
                aggressive: false,
                maps: vec![("prt_fild05".into(), 60), ("prt_fild01".into(), 20)],
                drops: vec!["Fluff".into(), "Feather".into()],
            }],
            ..Default::default()
        };
        ChatContext {
            me: "Elara".into(),
            about_me: String::new(),
            from: from.into(),
            channel: "party chat",
            lines: lines.iter().map(|l| (*l).to_owned()).collect(),
            facts: Vec::new(),
            decision: None,
            known,
        }
    }

    #[test]
    fn a_question_whether_it_is_a_bot_is_seen() {
        assert!(asks_if_bot("hi elara, are you a bot?") && asks_if_bot("u human?"));
        assert!(!asks_if_bot("the bot of the forum said hi") && !asks_if_bot("about to go?"));
    }

    /// The NPCs the decision model would meet first: `cargo test npcs_on_the_decision -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn npcs_on_the_decision_model() {
        let helper = Helper::new(Voice::SystemOne(
            SystemOne::new("http://127.0.0.1:9931").unwrap(),
        ));
        let situation = "A level 18/12 Swordman in Ragnarok Online is on prontera and may talk to one NPC there it \
has never met, to discover a quest, or news to tell other players.";
        let npcs: Vec<String> = [
            "Guard (12 cells away)",
            "Tool Dealer (30 cells away)",
            "Old Lady (45 cells away, a quest icon above it)",
            "Bard (20 cells away)",
            "Pub Master (60 cells away)",
            "Sign (8 cells away)",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut picks = std::collections::BTreeMap::new();
        for _ in 0..10 {
            let i = helper.npc_to_meet(situation, &npcs).await.unwrap();
            *picks.entry(npcs[i].clone()).or_insert(0) += 1;
        }
        println!("{picks:#?}");
    }

    /// Chat answers of the decision model: `cargo test chat_on_the_decision -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn chat_on_the_decision_model() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("voice=debug")
            .with_test_writer()
            .try_init();
        let helper = Helper::new(Voice::SystemOne(
            SystemOne::new("http://127.0.0.1:9931").unwrap(),
        ));
        let talks: [(&str, &[&str]); 8] = [
            ("Bravo", &["Bravo: anyone seen Fabres around?"]),
            ("Bravo", &["Bravo: can you follow me for a bit?"]),
            ("Ann", &["Ann: hi Elara, are you a bot?"]),
            (
                "Ann",
                &[
                    "Ann: hi Elara, are you a bot?",
                    "you: Yes, I'm a bot",
                    "Ann: ok thanks, bye!",
                ],
            ),
            ("Cid", &["Cid: wanna hunt together on prt_fild05?"]),
            ("Cid", &["Cid: how is the hunting going?"]),
            ("Dee", &["Dee: what level are you?"]),
            ("Dee", &["Dee: are you a real person"]),
        ];
        let dialogs: [(&str, &[&str], &[&str]); 2] = [
            (
                "change job to Swordman",
                &["[Swordman Guildsman] Welcome to the Swordman Guild."],
                &[
                    "Tell me about Swordmen",
                    "I want to become a Swordman",
                    "Cancel",
                ],
            ),
            (
                "buy Red Potion",
                &["[Tool Dealer] What do you need?"],
                &["Sell", "Buy", "Cancel"],
            ),
        ];
        for (goal, lines, options) in dialogs {
            let lines: Vec<String> = lines.iter().map(|l| (*l).to_owned()).collect();
            let options: Vec<String> = options.iter().map(|o| (*o).to_owned()).collect();
            let pick = helper
                .choose_dialog_option(goal, &lines, &options)
                .await
                .unwrap();
            println!("dialog {goal} -> {}", options[pick as usize - 1]);
        }
        for (from, lines) in talks {
            for _ in 0..2 {
                let turn = helper.chat(&context(from, lines), true).await.unwrap();
                println!(
                    "{} -> ask {:?}, reply {:?}",
                    lines.last().unwrap(),
                    turn.ask,
                    turn.reply
                );
            }
        }
    }
}
