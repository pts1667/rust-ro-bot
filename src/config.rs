//! Command line and settings.

use crate::strategist::prompt::Order;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

/// The bot of the commands that drive one bot, when `--name` gives none.
pub const DEFAULT_NAME: &str = "Rusty";

/// The key file of a rust-ro checked out next to this repo, when neither `--key-file` nor `--key` gives one.
pub const DEFAULT_KEY_FILE: &str = "../rust-ro/config/bots_api_key.txt";
/// The Bot API and the reference without `--url`, `--db-url` or `--server-dir`: the server's defaults.
const DEFAULT_URL: &str = "http://127.0.0.1:6902";
const DEFAULT_DB_URL: &str = "http://127.0.0.1:6904";

#[derive(Parser)]
#[command(
    about = "GOAP + small-model bots that play characters on a rust-ro server",
    long_about = concat!(
        "GOAP + small-model bots that play characters on a rust-ro server.\n\n",
        "Without a command, it runs `run`: 50 bots named from names.txt, with the decision model as their voice."
    ),
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[command(flatten)]
    pub endpoints: Endpoints,

    /// Character name of the bot. With `run`, plays that one bot instead of `--bots` bots from `--names`; the other
    /// commands use Rusty without it.
    #[arg(long, global = true)]
    pub name: Option<String>,

    /// The options of `run`, which runs when no command is given.
    #[command(flatten)]
    pub run: RunArgs,

    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Cli {
    /// The bot of a command that drives one bot.
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(DEFAULT_NAME)
    }
}

#[derive(Args, Clone)]
pub struct Endpoints {
    /// Base URL of the bot API [default: http://127.0.0.1:6902, or the one `--server-dir` sets up].
    #[arg(long = "url", id = "url", env = "BOT_API_URL", global = true)]
    url_arg: Option<String>,
    /// The bot API, once `resolve` ran.
    #[arg(skip)]
    pub url: String,

    /// Base URL of the reference (`/db` and `/wiki`) [default: http://127.0.0.1:6904, or the one `--server-dir` sets
    /// up].
    #[arg(long = "db-url", id = "db_url", env = "BOT_DB_URL", global = true)]
    db_url_arg: Option<String>,
    /// The reference, once `resolve` ran.
    #[arg(skip)]
    pub db_url: String,

    /// The folder of the rust-ro server: the bot API and the reference are where its `config.json` puts them (`bots`
    /// and `reference`, a host of 0.0.0.0 meaning this machine), and the key is its `bots.api_key_path`. `--url`,
    /// `--db-url`, `--key-file` and `--key` win over it.
    #[arg(long, env = "BOT_SERVER_DIR", global = true)]
    pub server_dir: Option<PathBuf>,

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

    /// File holding the API key (the server's `config/bots_api_key.txt`). Without it or `--key`, the key of
    /// `../rust-ro` is used.
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
    /// Play (the command when none is given): GOAP executes, the strategist picks goals.
    Run(RunArgs),
    /// Walk the bot to each map in turn the way a goal's trip goes (warps, NPC ways, `@go`), and print how it ended.
    Travel {
        #[arg(required = true)]
        to: Vec<String>,
    },
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
    /// Bots to play at once, named by the first lines of `--names`. A name without a character gets one made.
    #[arg(long, default_value_t = 50)]
    pub bots: usize,
    /// The file of bot names, one per line (`python tools/names.py <count>` adds more).
    #[arg(long, default_value = "names.txt")]
    pub names: PathBuf,
    /// Seconds between two bots joining the game.
    #[arg(long, default_value_t = 3)]
    pub join_gap: u64,
    /// Minutes a bot plays before it logs off and a resting character of the pool takes its place: `MIN-MAX` for a
    /// random length in between, or one number. 0 keeps every bot in the game.
    #[arg(long, default_value = "30-60", value_parser = Session::parse)]
    pub session: Session,
    /// Characters taking turns in the `--bots` places, the first names of `--names`. Defaults to twice `--bots`.
    #[arg(long)]
    pub pool: Option<usize>,
    /// Write each bot's log to `<dir>/<name>.log` instead of the terminal.
    #[arg(long)]
    pub log_dir: Option<PathBuf>,
    /// Stop as on Ctrl-C when a line `stop` comes in on stdin or stdin closes: for a launcher that cannot send Ctrl-C.
    /// The bots save their memory and log out; a launcher that dies closes the pipe and stops them too.
    #[arg(long)]
    pub stop_on_stdin: bool,
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
    #[arg(long, value_enum, default_value_t = crate::voice::VoiceMode::SystemOne)]
    pub voice: crate::voice::VoiceMode,
    /// Never visit the players' forum (only a bot with a voice visits it).
    #[arg(long)]
    pub no_forum: bool,
    /// Chance (0 to 1) that a strategist step goes to the model (the LLM, or the decision model with
    /// `--voice system-one`); the others use the deterministic fallback. Defaults to 0.25 with the decision model,
    /// 1 with the LLM.
    #[arg(long)]
    pub llm_share: Option<f32>,
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

/// How long a bot plays before it logs off, in minutes. `max` 0 keeps it in the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Session {
    pub min: u64,
    pub max: u64,
}

impl Session {
    fn parse(text: &str) -> Result<Self, String> {
        let number = |t: &str| {
            t.trim()
                .parse::<u64>()
                .map_err(|_| format!("{text:?} is not `MIN-MAX` or a number of minutes"))
        };
        let (min, max) = match text.split_once('-') {
            Some((min, max)) => (number(min)?, number(max)?),
            None => (number(text)?, number(text)?),
        };
        if min > max || (min == 0 && max > 0) {
            return Err(format!(
                "{text:?}: a session lasts at least a minute, and MIN comes first"
            ));
        }
        Ok(Self { min, max })
    }

    /// Whether bots log off at all.
    pub fn rotates(&self) -> bool {
        self.max > 0
    }

    /// A random length within the range.
    pub fn pick(&self) -> Duration {
        use rand::Rng;
        Duration::from_secs(rand::rng().random_range(self.min * 60..=self.max * 60))
    }
}

impl RunArgs {
    /// The characters taking turns: `--pool`, or twice `--bots`, never fewer than `--bots`.
    pub fn pool_size(&self) -> usize {
        if !self.session.rotates() {
            return self.bots;
        }
        self.pool.unwrap_or(self.bots * 2).max(self.bots)
    }

    /// `--llm-share`, or its default for the voice.
    pub fn share(&self) -> f32 {
        let default = match self.voice {
            crate::voice::VoiceMode::SystemOne => 0.25,
            crate::voice::VoiceMode::Llm => 1.0,
        };
        self.llm_share.unwrap_or(default).clamp(0.0, 1.0)
    }
}

impl Endpoints {
    /// Fills in the bot API, the reference and the key file from the options, `--server-dir` and the defaults.
    pub fn resolve(&mut self) -> Result<()> {
        let server = match &self.server_dir {
            Some(dir) => Some(ServerSetup::read(dir)?),
            None => None,
        };
        self.url = match (&self.url_arg, &server) {
            (Some(url), _) => url.clone(),
            (None, Some(server)) => server.url.clone(),
            (None, None) => DEFAULT_URL.to_owned(),
        };
        self.db_url = match (&self.db_url_arg, &server) {
            (Some(url), _) => url.clone(),
            (None, Some(server)) => server.db_url.clone(),
            (None, None) => DEFAULT_DB_URL.to_owned(),
        };
        if self.key_file.is_none()
            && self.key.is_none()
            && let Some(server) = server
        {
            self.key_file = Some(server.key_file);
        }
        Ok(())
    }

    /// Reads the key from the file or the env var. The file may carry a UTF-8 BOM (PowerShell writes one),
    /// which the server would not strip, so it goes here.
    pub fn load_key(&self) -> Result<String> {
        let raw = match (&self.key_file, &self.key) {
            (Some(path), _) => std::fs::read_to_string(path)
                .with_context(|| format!("reading key file {}", path.display()))?,
            (None, Some(key)) => key.clone(),
            (None, None) => std::fs::read_to_string(DEFAULT_KEY_FILE).with_context(|| {
                format!("no API key: pass --key-file <path> or set BOT_API_KEY_FILE ({DEFAULT_KEY_FILE} is not there)")
            })?,
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

/// Where a rust-ro server in a folder serves the bot API and the reference, and where it keeps the key.
struct ServerSetup {
    url: String,
    db_url: String,
    key_file: PathBuf,
}

impl ServerSetup {
    /// From `<dir>/config.json`. What it leaves out is the server's default, as the server reads it.
    fn read(dir: &std::path::Path) -> Result<Self> {
        let path = dir.join("config.json");
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading the server config {}", path.display()))?;
        let config: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .with_context(|| format!("parsing the server config {}", path.display()))?;
        Ok(Self::from_config(dir, &config))
    }

    fn from_config(dir: &std::path::Path, config: &serde_json::Value) -> Self {
        let address = |section: &str, port: u16| {
            let host = config[section]["host"].as_str().unwrap_or("127.0.0.1");
            let port = config[section]["port"]
                .as_u64()
                .and_then(|p| u16::try_from(p).ok())
                .unwrap_or(port);
            format!("http://{}:{port}", reachable(host))
        };
        let key = config["bots"]["api_key_path"]
            .as_str()
            .unwrap_or("config/bots_api_key.txt");
        Self {
            url: address("bots", 6902),
            db_url: address("reference", 6904),
            // The server reads it from its working directory, its own folder
            key_file: dir.join(key),
        }
    }
}

/// The host to reach a server bound to `host` at: the loopback for a bind to every address, brackets around IPv6.
fn reachable(host: &str) -> String {
    match host.trim_matches(|c| c == '[' || c == ']') {
        "" | "0.0.0.0" => "127.0.0.1".to_owned(),
        "::" => "[::1]".to_owned(),
        v6 if v6.contains(':') => format!("[{v6}]"),
        host => host.to_owned(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_lengths() {
        assert_eq!(Session::parse("30-60"), Ok(Session { min: 30, max: 60 }));
        assert_eq!(Session::parse("45"), Ok(Session { min: 45, max: 45 }));
        assert!(!Session::parse("0").unwrap().rotates());
        assert!(Session::parse("60-30").is_err());
        assert!(Session::parse("0-30").is_err());
        assert!(Session::parse("half an hour").is_err());
        let range = Session { min: 30, max: 60 };
        for _ in 0..100 {
            let length = range.pick().as_secs();
            assert!((30 * 60..=60 * 60).contains(&length));
        }
    }

    #[test]
    fn pool_defaults_to_twice_the_bots() {
        let args = |line: &[&str]| {
            Cli::try_parse_from([&["rust-ro-bot"], line].concat())
                .unwrap()
                .run
        };
        assert_eq!(args(&[]).pool_size(), 100);
        assert_eq!(args(&["--bots", "5", "--pool", "3"]).pool_size(), 5);
        assert_eq!(args(&["--bots", "5", "--session", "0"]).pool_size(), 5);
    }

    #[test]
    fn server_dir_sets_the_addresses_and_the_key() {
        let dir = std::path::Path::new("srv");
        let config = serde_json::json!({
            "bots": { "host": "0.0.0.0", "port": 7002, "api_key_path": "keys/bot.txt" },
            "reference": { "host": "192.168.1.5" }
        });
        let server = ServerSetup::from_config(dir, &config);
        assert_eq!(server.url, "http://127.0.0.1:7002");
        assert_eq!(server.db_url, "http://192.168.1.5:6904");
        assert_eq!(server.key_file, dir.join("keys/bot.txt"));
        let server = ServerSetup::from_config(dir, &serde_json::json!({ "bots": { "host": "::" } }));
        assert_eq!(server.url, "http://[::1]:6902");
        assert_eq!(server.key_file, dir.join("config/bots_api_key.txt"));
        assert_eq!(reachable("fe80::1"), "[fe80::1]");
        assert_eq!(reachable("localhost"), "localhost");
    }

    #[test]
    fn urls_given_win_over_the_server_dir() {
        let endpoints = |line: &[&str]| {
            let mut cli = Cli::try_parse_from([&["rust-ro-bot"], line].concat()).unwrap();
            cli.endpoints.resolve().map(|()| cli.endpoints)
        };
        let plain = endpoints(&[]).unwrap();
        assert_eq!(plain.url, DEFAULT_URL);
        assert_eq!(plain.db_url, DEFAULT_DB_URL);
        assert!(plain.key_file.is_none());
        let given = endpoints(&["--url", "http://h:1", "--db-url", "http://h:2"]).unwrap();
        assert_eq!((given.url.as_str(), given.db_url.as_str()), ("http://h:1", "http://h:2"));
        // A server folder without a config is an error, not the defaults
        assert!(endpoints(&["--server-dir", "no/such/dir", "--url", "http://h:1"]).is_err());
    }
}
