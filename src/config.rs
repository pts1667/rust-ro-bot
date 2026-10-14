//! Command line and settings.

use crate::strategist::prompt::Order;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(about = "A GOAP + small-LLM bot that plays a character on a rust-ro server")]
pub struct Cli {
    #[command(flatten)]
    pub endpoints: Endpoints,

    /// Character name of the bot.
    #[arg(long, default_value = "Rusty", global = true)]
    pub name: String,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Args, Clone)]
pub struct Endpoints {
    /// Base URL of the bot API.
    #[arg(
        long,
        env = "BOT_API_URL",
        default_value = "http://127.0.0.1:6902",
        global = true
    )]
    pub url: String,

    /// Base URL of the reference (`/db` and `/wiki`).
    #[arg(
        long,
        env = "BOT_DB_URL",
        default_value = "http://127.0.0.1:6904",
        global = true
    )]
    pub db_url: String,

    /// Base URL of the llama.cpp server.
    #[arg(
        long,
        env = "BOT_LLM_URL",
        default_value = "http://127.0.0.1:8080",
        global = true
    )]
    pub llm_url: String,

    /// Base URL of the llama.cpp server of the decision model (`/v1/systemone`), for `--voice system-one`.
    #[arg(
        long,
        env = "BOT_S1_URL",
        default_value = "http://127.0.0.1:9931",
        global = true
    )]
    pub s1_url: String,

    /// Model name sent to the llama.cpp server (it serves one model and ignores the name).
    #[arg(
        long,
        env = "BOT_LLM_MODEL",
        default_value = "local-model",
        global = true
    )]
    pub llm_model: String,

    /// Tokens the model may think before a strategist answer or a forum turn (llama.cpp `thinking_budget_tokens`). 0
    /// turns thinking off.
    #[arg(long, env = "BOT_LLM_THINK", default_value_t = DEFAULT_THINK, global = true)]
    pub llm_think: u32,

    #[command(flatten)]
    pub sampling: Sampling,

    /// File holding the API key (the server's `config/bots_api_key.txt`).
    #[arg(long, env = "BOT_API_KEY_FILE", conflicts_with = "key", global = true)]
    pub key_file: Option<PathBuf>,

    /// The API key itself. Prefer `--key-file`, so the key stays out of shell history.
    #[arg(long, env = "BOT_API_KEY", global = true)]
    pub key: Option<String>,

    /// Directory for the `/db` cache and the bot memory.
    #[arg(long, default_value = ".", global = true)]
    pub data_dir: PathBuf,
}

#[derive(Subcommand)]
pub enum Command {
    /// Connect the bot (creating it if needed) and print what it sees, without acting.
    Probe,
    /// Play: GOAP executes, the LLM strategist picks goals.
    Run(RunArgs),
    /// Print a `/db` path, e.g. `db /db/maps/prt_fild08`, or the hunting grounds with `db grounds <map>`.
    Db {
        path: String,
        #[arg(default_value = "")]
        arg: String,
    },
    /// Run one strategist step on the bot's current observation and print the prompt and the answer.
    LlmCheck {
        #[arg(long, value_enum, default_value_t = Order::TaskFirst)]
        order: Order,
    },
    /// Run history-mode turns on the bot's current observation, printing every request, response and compaction.
    HistoryCheck {
        /// Turns to run.
        #[arg(long, default_value_t = 12)]
        turns: u32,
        /// Estimated tokens that trigger compaction.
        #[arg(long, default_value_t = 12_000)]
        compact_at: usize,
        /// Estimated tokens compaction aims for.
        #[arg(long, default_value_t = 6_000)]
        compact_to: usize,
        /// Ask the model to compact first, before the oldest turns are dropped.
        #[arg(long)]
        model_compaction: bool,
    },
    /// Score the strategist on fixed scenarios, for each turn layout (or only `--order`).
    LlmEval {
        #[arg(long, value_enum)]
        order: Option<Order>,
        /// Tries per scenario and layout.
        #[arg(long, default_value_t = 5)]
        n: usize,
        /// Leave out the few-shot example turns.
        #[arg(long)]
        no_examples: bool,
        /// Keep the options in their built order.
        #[arg(long)]
        no_shuffle: bool,
    },
}

#[derive(Args, Clone)]
pub struct RunArgs {
    /// Stop when the bot reaches this base level.
    #[arg(long)]
    pub until_level: Option<u32>,
    /// Stop after this many minutes.
    #[arg(long)]
    pub minutes: Option<u64>,
    /// Use no model at all: the fallback strategist picks the goals, and the bot does not chat or visit the forum.
    #[arg(long)]
    pub no_llm: bool,
    /// Seconds between two strategist steps when nothing happens.
    #[arg(long, default_value_t = 45)]
    pub strategist_period: u64,
    /// Who talks in chat and on the forum and takes the model's share of the goal choices: the LLM in its own words,
    /// or the decision model with sentences of a catalogue.
    #[arg(long, value_enum, default_value_t = crate::voice::VoiceMode::Llm)]
    pub voice: crate::voice::VoiceMode,
    /// Never visit the players' forum (only a bot with a voice visits it).
    #[arg(long)]
    pub no_forum: bool,
    /// Chance (0 to 1) that a strategist step goes to the model (the LLM, or the decision model with
    /// `--voice system-one`); the others use the deterministic fallback.
    #[arg(long, default_value_t = 1.0)]
    pub llm_share: f32,
    /// Layout of the strategist turns.
    #[arg(long, value_enum, default_value_t = Order::TaskFirst)]
    pub order: Order,
    /// How the strategist keeps context between steps.
    #[arg(long, value_enum, default_value_t = crate::strategist::llm::ContextMode::SkillState)]
    pub context: crate::strategist::llm::ContextMode,
    /// Play in a party: the name of the party to create when the bot leads it.
    #[arg(long)]
    pub party: Option<String>,
    /// The characters to play with, comma separated: invited when the bot leads, the only invitations it takes.
    #[arg(long, value_delimiter = ',')]
    pub friends: Vec<String>,
    /// Lead the party: create it, invite the friends, choose where to hunt.
    #[arg(long)]
    pub lead: bool,
    /// Play alone: no party, not even one found on the booking board.
    #[arg(long, conflicts_with = "party")]
    pub solo: bool,
    /// The bot's part in the party; healer and tank also set the job it heads for.
    #[arg(long, value_enum, default_value_t = crate::party::Role::Fighter)]
    pub role: crate::party::Role,
    /// The job to head for, a first or a second job (`Hunter` goes through Archer); it also sets the stat build.
    #[arg(long)]
    pub job: Option<String>,
}

impl Endpoints {
    /// Reads the key from the file or the env var. The file may carry a UTF-8 BOM (PowerShell writes one),
    /// which the server would not strip, so it goes here.
    pub fn load_key(&self) -> Result<String> {
        let raw = match (&self.key_file, &self.key) {
            (Some(path), _) => std::fs::read_to_string(path)
                .with_context(|| format!("reading key file {}", path.display()))?,
            (None, Some(key)) => key.clone(),
            (None, None) => bail!("no API key: pass --key-file <path> or set BOT_API_KEY_FILE"),
        };
        let key = raw.trim_start_matches('\u{feff}').trim().to_owned();
        if key.is_empty() {
            bail!("the API key is empty");
        }
        Ok(key)
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.data_dir.join("cache").join("db")
    }

    pub fn state_dir(&self) -> PathBuf {
        self.data_dir.join("state")
    }
}

/// Default thinking budget of the strategist and the forum, in tokens.
pub const DEFAULT_THINK: u32 = 2048;

/// llama.cpp sampling parameters of the strategist. Unset ones keep the server's defaults.
#[derive(Args, Clone, Debug)]
pub struct Sampling {
    #[arg(long = "llm-temp", default_value_t = 0.3, global = true)]
    pub temperature: f32,
    #[arg(long = "llm-top-p", global = true)]
    pub top_p: Option<f32>,
    #[arg(long = "llm-top-k", global = true)]
    pub top_k: Option<u32>,
    #[arg(long = "llm-min-p", global = true)]
    pub min_p: Option<f32>,
    /// Penalty on tokens repeated within the last `repeat_last_n` tokens (llama.cpp `repeat_penalty`, 1 = off).
    #[arg(long = "llm-repeat-penalty", global = true)]
    pub repeat_penalty: Option<f32>,
    #[arg(long = "llm-presence-penalty", global = true)]
    pub presence_penalty: Option<f32>,
    #[arg(long = "llm-frequency-penalty", global = true)]
    pub frequency_penalty: Option<f32>,
}

impl Sampling {
    /// The parameters as request fields.
    pub fn fields(&self) -> Vec<(&'static str, serde_json::Value)> {
        let mut out = vec![("temperature", serde_json::json!(self.temperature))];
        let mut opt = |key, value: Option<serde_json::Value>| {
            if let Some(v) = value {
                out.push((key, v));
            }
        };
        opt("top_p", self.top_p.map(Into::into));
        opt("top_k", self.top_k.map(Into::into));
        opt("min_p", self.min_p.map(Into::into));
        opt("repeat_penalty", self.repeat_penalty.map(Into::into));
        opt("presence_penalty", self.presence_penalty.map(Into::into));
        opt("frequency_penalty", self.frequency_penalty.map(Into::into));
        out
    }
}
