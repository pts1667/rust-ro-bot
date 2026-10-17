mod agent;
mod api;
mod behaviors;
mod bot;
mod config;
mod forum;
mod goap;
mod knowledge;
mod logging;
mod memory;
mod party;
mod quests;
mod social;
mod strategist;
mod voice;

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Parser;
use tracing::Instrument;

use api::{BotApi, Observation};
use bot::Bot;
use config::{Cli, Command, Endpoints, RunArgs};
use knowledge::Knowledge;
use knowledge::db::Db;
use memory::Memory;
use strategist::helper::Helper;
use strategist::llm::Llm;

#[tokio::main]
async fn main() -> Result<()> {
    let mut cli = Cli::parse();
    cli.endpoints.resolve()?;
    // `run` is the command when none is given
    let run_args = match &cli.command {
        None => Some(cli.run.clone()),
        Some(Command::Run(args)) => Some(args.clone()),
        Some(_) => None,
    };
    let names = match &run_args {
        Some(args) => bot_names(cli.name.as_deref(), args)?,
        None => vec![cli.name().to_owned()],
    };
    for name in &names {
        check_name(name)?;
    }
    logging::init(run_args.as_ref().and_then(|a| a.log_dir.as_deref()), &names)?;
    let db = Db::new(&cli.endpoints.db_url, Some(cli.endpoints.cache_dir()))?;
    let knowledge = Knowledge::new(db);
    let name = cli.name();

    if let Some(args) = run_args {
        return run_bots(&cli.endpoints, knowledge, args, names).await;
    }
    let Some(command) = &cli.command else {
        unreachable!("no command runs `run`")
    };
    match command {
        Command::Db { path, arg } => db_command(&cli, &knowledge, path, arg).await,
        Command::Probe => {
            let api = BotApi::new(&cli.endpoints.url, cli.endpoints.load_key()?)?;
            ensure_connected(&api, name).await?;
            let obs = wait_ready(&api, name).await?;
            print_summary(&obs);
            Ok(())
        }
        Command::Run(_) => unreachable!("handled above"),
        Command::Travel { to } => {
            let api = BotApi::new(&cli.endpoints.url, cli.endpoints.load_key()?)?;
            ensure_connected(&api, name).await?;
            wait_ready(&api, name).await?;
            let memory = Memory::load(&cli.endpoints.state_dir(), name)?;
            knowledge.graph.forbid(memory.bad_warps.iter().cloned());
            let mut bot = Bot::new(api, name.to_owned(), knowledge, memory, None);
            for map in to {
                let outcome = bot.travel(map).await?;
                let obs = bot.observe().await?;
                println!("{map}: {outcome:?}: on {} at {:?}", obs.map.name, obs.pos());
            }
            bot.memory.save()?;
            Ok(())
        }
        Command::LlmCheck { order } => llm_check(&cli, knowledge, *order).await,
        Command::HistoryCheck {
            turns,
            compact_at,
            compact_to,
            model_compaction,
        } => {
            history_check(
                &cli,
                knowledge,
                *turns,
                *compact_at,
                *compact_to,
                *model_compaction,
            )
            .await
        }
        Command::LlmEval {
            order,
            n,
            no_examples,
            no_shuffle,
        } => {
            let mut llm = Llm::new(
                &cli.endpoints.llm_url,
                &cli.endpoints.llm_model,
                cli.endpoints.llm_think,
                &cli.endpoints.sampling,
            )?;
            let orders = match order {
                Some(o) => vec![*o],
                None => vec![
                    strategist::prompt::Order::TaskFirst,
                    strategist::prompt::Order::DataFirst,
                ],
            };
            llm.examples = !no_examples;
            llm.shuffle = !no_shuffle;
            strategist::eval::run(&llm, &knowledge, &orders, *n).await
        }
    }
}

/// The bots `run` plays: the one of `--name`, or the first names of the `--names` file, `--bots` of them and the
/// resting ones of the pool (as many as the file has).
fn bot_names(name: Option<&str>, args: &RunArgs) -> Result<Vec<String>> {
    if let Some(name) = name {
        return Ok(vec![name.to_owned()]);
    }
    let text = std::fs::read_to_string(&args.names)
        .with_context(|| format!("reading the bot names in {}", args.names.display()))?;
    let mut names: Vec<String> = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(line)) {
            names.push(line.to_owned());
        }
    }
    if names.len() < args.bots {
        bail!(
            "{} has {} names for {} bots: add some with `python tools/names.py <count>`",
            args.names.display(),
            names.len(),
            args.bots
        );
    }
    names.truncate(args.pool_size());
    Ok(names)
}

/// A character name the server takes: at most 19 characters, nothing around it. A name read from a file with Windows
/// line ends would keep its `\r`, and the server would make a character of it.
fn check_name(name: &str) -> Result<()> {
    if name.trim().is_empty()
        || name.chars().any(|c| c.is_control())
        || name != name.trim()
        || name.chars().count() > 19
    {
        bail!(
            "bad character name {name:?}: at most 19 characters, no control characters or spaces around it"
        );
    }
    Ok(())
}

/// Plays `names` in `--bots` places, one bot after the other joining `--join-gap` seconds apart. With `--session`, a
/// bot logs off after its session and the character of the pool resting longest takes its place, until `--minutes`
/// are up. Ctrl-C stops them all after their current step.
async fn run_bots(
    endpoints: &Endpoints,
    knowledge: Knowledge,
    mut args: RunArgs,
    names: Vec<String>,
) -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        tokio::spawn(async move {
            let why = stop_signal().await;
            tracing::info!("{why}: stopping after the current step");
            stop.store(true, Ordering::SeqCst);
        });
    }
    if args.stop_on_stdin {
        let stop = stop.clone();
        // On a thread of its own: the read blocks
        std::thread::spawn(move || {
            let mut line = String::new();
            loop {
                line.clear();
                match std::io::stdin().read_line(&mut line) {
                    Ok(0) | Err(_) => {
                        tracing::info!("stdin closed: stopping after the current step");
                        break;
                    }
                    Ok(_) if line.trim().eq_ignore_ascii_case("stop") => {
                        tracing::info!("stop on stdin: stopping after the current step");
                        break;
                    }
                    Ok(_) => {}
                }
            }
            stop.store(true, Ordering::SeqCst);
        });
    }
    // One bot logs to the terminal as before; with several, each line says whose it is
    let named = names.len() > 1 || args.log_dir.is_some();
    if let Some(dir) = &args.log_dir {
        tracing::info!(bots = names.len(), dir = %dir.display(), "each bot logs to <dir>/<name>.log");
    }
    let places = args.bots.clamp(1, names.len());
    let rotate = args.session.rotates() && names.len() > places;
    if rotate {
        tracing::info!(
            characters = names.len(),
            places,
            minutes = format!("{}-{}", args.session.min, args.session.max),
            "the characters take turns: each logs off after a session and one resting takes its place"
        );
        // `--minutes` ends the whole run, not each session
        if let Some(minutes) = args.minutes.take() {
            let stop = stop.clone();
            tokio::spawn(async move {
                if wait_unless(&stop, Duration::from_secs(minutes * 60)).await {
                    tracing::info!("time is up: every bot logs off");
                    stop.store(true, Ordering::SeqCst);
                }
            });
        }
    } else if names.len() > 1 && args.session.rotates() {
        tracing::info!(
            bots = places,
            "no character rests to take turns (raise --pool): the bots stay in the game"
        );
    }
    release_resting(endpoints, &names[..places], &names[places..]).await;
    let launcher = Launcher {
        endpoints,
        knowledge: &knowledge,
        args: &args,
        stop: &stop,
        named,
        rotate,
    };
    let mut bots = tokio::task::JoinSet::new();
    for (i, name) in names[..places].iter().enumerate() {
        if i > 0 && !wait_unless(&stop, Duration::from_secs(args.join_gap)).await {
            break;
        }
        launcher.spawn(&mut bots, name.clone());
    }
    // The characters out of the game, the one resting longest first
    let mut resting: VecDeque<String> = names[places..].iter().cloned().collect();
    let (mut ended, mut failed) = (0, Vec::new());
    while let Some(joined) = bots.join_next().await {
        ended += 1;
        let left = match joined {
            Ok((name, Ok(()))) => Some(name),
            Ok((name, Err(error))) => {
                tracing::warn!(bot = %name, error = format!("{error:#}"), "the bot stopped on an error");
                failed.push(error);
                None
            }
            Err(error) => {
                tracing::warn!(%error, "a bot's task ended badly");
                None
            }
        };
        if !rotate || stop.load(Ordering::SeqCst) {
            continue;
        }
        // A bot that failed stays out: its name may be one the server refuses
        resting.extend(left);
        if let Some(next) = resting.pop_front() {
            tracing::info!(bot = %next, "takes a free place in the game");
            launcher.spawn(&mut bots, next);
        }
    }
    if names.len() > 1 {
        tracing::info!(sessions = ended, failed = failed.len(), "all bots stopped");
    }
    if ended == 1
        && let Some(error) = failed.pop()
    {
        return Err(error);
    }
    if ended > 0 && failed.len() == ended {
        bail!("every bot stopped on an error");
    }
    Ok(())
}

/// Logs out the characters of `resting` still in the game, left there by an earlier run that was killed rather than
/// stopped: until the server's idle timeout they would take places the bots of `playing` need. The characters of
/// `playing` play on in theirs.
async fn release_resting(endpoints: &Endpoints, playing: &[String], resting: &[String]) {
    if resting.is_empty() {
        return;
    }
    let listed = match endpoints.load_key().and_then(|key| BotApi::new(&endpoints.url, key)) {
        Ok(api) => api.list().await.map(|bots| (api, bots)),
        Err(error) => Err(error),
    };
    let (api, bots) = match listed {
        Ok(listed) => listed,
        Err(error) => {
            // Each bot reports what is wrong with the server when it joins
            tracing::debug!(error = format!("{error:#}"), "could not list the bots in the game");
            return;
        }
    };
    let left: Vec<String> = bots
        .into_iter()
        .filter(|b| b.connected)
        .map(|b| b.name)
        .filter(|name| {
            resting.iter().any(|r| r.eq_ignore_ascii_case(name))
                && !playing.iter().any(|p| p.eq_ignore_ascii_case(name))
        })
        .collect();
    if left.is_empty() {
        return;
    }
    tracing::info!(count = left.len(), "logging out the resting characters an earlier run left in the game");
    let mut out = tokio::task::JoinSet::new();
    for name in left {
        let api = api.clone();
        out.spawn(async move {
            if let Err(error) = api.disconnect(&name).await {
                tracing::warn!(bot = %name, error = format!("{error:#}"), "could not log out a resting character");
            }
        });
    }
    out.join_all().await;
}

/// What starts a bot's session in `run_bots`.
struct Launcher<'a> {
    endpoints: &'a Endpoints,
    knowledge: &'a Knowledge,
    args: &'a RunArgs,
    /// Set by Ctrl-C or the end of `--minutes`: every bot stops.
    stop: &'a Arc<AtomicBool>,
    /// Whether each log line names its bot.
    named: bool,
    /// Whether a bot logs off after a session of `--session` minutes.
    rotate: bool,
}

impl Launcher<'_> {
    fn spawn(&self, bots: &mut tokio::task::JoinSet<(String, Result<()>)>, name: String) {
        let span = if self.named {
            tracing::info_span!(logging::BOT_SPAN, name = name.as_str())
        } else {
            tracing::Span::none()
        };
        let mut stop = agent::Stop {
            now: self.stop.clone(),
            in_town: None,
        };
        if self.rotate {
            // Set when the session is over: the bot logs off in the next town
            let session = Arc::new(AtomicBool::new(false));
            let (all, flag, length) =
                (self.stop.clone(), session.clone(), self.args.session.pick());
            span.in_scope(|| {
                tracing::info!(minutes = length.as_secs() / 60, "joins for a session")
            });
            tokio::spawn(async move {
                if wait_unless(&all, length).await {
                    flag.store(true, Ordering::SeqCst);
                }
            });
            stop.in_town = Some(session);
        }
        let (endpoints, knowledge, args) = (
            self.endpoints.clone(),
            self.knowledge.for_bot(),
            self.args.clone(),
        );
        bots.spawn(async move {
            let result = run(&endpoints, &name, knowledge, args, stop)
                .instrument(span)
                .await;
            (name, result)
        });
    }
}

/// Waits for Ctrl-C or the system asking the process to end: SIGTERM, or on Windows the console closing, Ctrl-Break,
/// a log-off or a shutdown. What came, for the log.
async fn stop_signal() -> &'static str {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => return "Ctrl-C",
                _ = term.recv() => return "SIGTERM",
            }
        }
    }
    #[cfg(windows)]
    {
        use tokio::signal::windows;
        if let (Ok(mut close), Ok(mut brk), Ok(mut shutdown), Ok(mut logoff)) = (
            windows::ctrl_close(),
            windows::ctrl_break(),
            windows::ctrl_shutdown(),
            windows::ctrl_logoff(),
        ) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => return "Ctrl-C",
                _ = close.recv() => return "console closed",
                _ = brk.recv() => return "Ctrl-Break",
                _ = shutdown.recv() => return "shutdown",
                _ = logoff.recv() => return "log-off",
            }
        }
    }
    match tokio::signal::ctrl_c().await {
        Ok(()) => "Ctrl-C",
        // No signal can come: wait for the other ways to stop
        Err(_) => std::future::pending().await,
    }
}

/// Waits `time` unless `stop` is set first: whether it was not.
async fn wait_unless(stop: &AtomicBool, time: Duration) -> bool {
    let until = tokio::time::Instant::now() + time;
    while tokio::time::Instant::now() < until {
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    !stop.load(Ordering::SeqCst)
}

/// Plays the bot `name`, its character made first when it has none.
async fn run(
    endpoints: &Endpoints,
    name: &str,
    knowledge: Knowledge,
    args: RunArgs,
    stop: agent::Stop,
) -> Result<()> {
    let api = BotApi::new(&endpoints.url, endpoints.load_key()?)?;
    ensure_connected(&api, name).await?;
    wait_ready(&api, name).await?;
    let memory = Memory::load(&endpoints.state_dir(), name)?;
    knowledge.graph.forbid(memory.bad_warps.iter().cloned());

    let llm = if args.no_llm || args.voice != voice::VoiceMode::Llm {
        None
    } else {
        let mut llm = Llm::new(
            &endpoints.llm_url,
            &endpoints.llm_model,
            endpoints.llm_think,
            &endpoints.sampling,
        )?;
        llm.share = args.share();
        llm.order = args.order;
        llm.context = args.context;
        if llm.healthy().await {
            Some(llm)
        } else {
            tracing::warn!(url = %endpoints.llm_url, "the LLM does not answer: playing with the fallback strategist");
            None
        }
    };
    let voice = match args.voice {
        _ if args.no_llm => None,
        voice::VoiceMode::Llm => llm.clone().map(voice::Voice::Llm),
        voice::VoiceMode::SystemOne => {
            let s1 = voice::systemone::SystemOne::new(&endpoints.s1_url)?;
            if s1.healthy().await {
                tracing::info!(url = %endpoints.s1_url, share = args.share(), "the decision model talks and picks a share of the goals");
                Some(voice::Voice::SystemOne(s1))
            } else {
                tracing::warn!(url = %endpoints.s1_url, "the decision model does not answer: playing without a voice");
                None
            }
        }
    };
    let helper = voice.clone().map(Helper::new);
    let mut bot = Bot::new(api, name.to_owned(), knowledge.clone(), memory, helper);
    if let Some(job) = &args.job {
        let Some((path, build)) = strategist::skill_state::job_path(job) else {
            bail!("unknown job {job}")
        };
        let obs = bot.observe().await?;
        tracing::info!(?path, build, "heading for a job");
        bot.memory
            .strategy
            .pin_role(&path, Some(build), &obs.me.job);
    }
    if let Some(name) = &args.party {
        let obs = bot.observe().await?;
        let config = party::PartyConfig {
            name: name.clone(),
            friends: args.friends.clone(),
            lead: args.lead,
            role: args.role,
            auto: false,
        };
        tracing::info!(party = %name, friends = ?config.friends, lead = config.lead, role = ?config.role, "playing in a party");
        bot.memory
            .strategy
            .pin_role(args.role.jobs(), args.role.stat_build(), &obs.me.job);
        bot.party = Some(party::PartyPlay::new(config, obs.last_message_seq()));
    } else if !args.solo {
        // Finds a party on the booking board, as a player would
        let obs = bot.observe().await?;
        let config = party::PartyConfig {
            name: String::new(),
            friends: args.friends.clone(),
            lead: false,
            role: party::Role::of_job(&obs.me.job),
            auto: true,
        };
        tracing::info!(friends = ?config.friends, "finding a party on the booking board");
        bot.party = Some(party::PartyPlay::new(config, obs.last_message_seq()));
    }
    let forum_voice = voice.clone().filter(|_| !args.no_forum);
    // With the decision model talking, it also takes its share (`--llm-share`) of the strategist steps
    let decider = match &voice {
        Some(voice::Voice::SystemOne(s1)) => {
            Some(strategist::decided::Decider::new(s1.clone(), args.share()))
        }
        _ => None,
    };
    let settings = strategist::Settings {
        knowledge,
        llm,
        decider,
        period: Duration::from_secs(args.strategist_period.max(5)),
    };
    let handle = strategist::spawn(
        settings,
        bot.memory.strategy.clone(),
        bot.latest_directive.clone(),
    );
    // The forum is read and written with a model only: without one the bot has nothing to say there
    if let Some(voice) = forum_voice {
        let api = api::forum::ForumApi::new(&endpoints.url, endpoints.load_key()?, name)?;
        forum::spawn(forum::Start {
            api,
            voice,
            knowledge: bot.knowledge.clone(),
            snapshots: handle.snapshots.subscribe(),
            events: handle.events.clone(),
            state_dir: endpoints.state_dir(),
        });
    }
    agent::run(bot, handle, args, stop).await
}

/// One strategist step on the current game state, printing the prompt and the decision.
/// The strategist's view of the bot as it is now, without running it.
async fn live_snapshot(cli: &Cli, knowledge: &Knowledge) -> Result<(strategist::Snapshot, Memory)> {
    use strategist::Snapshot;
    let api = BotApi::new(&cli.endpoints.url, cli.endpoints.load_key()?)?;
    ensure_connected(&api, cli.name()).await?;
    let obs = wait_ready(&api, cli.name()).await?;
    let memory = Memory::load(&cli.endpoints.state_dir(), cli.name())?;
    knowledge.graph.forbid(memory.bad_warps.iter().cloned());
    knowledge.learn_heals(&obs).await;
    let heal_hp = knowledge.heal_stock(&obs);
    let ws = goap::WorldState::from_observation(
        &obs,
        &goap::state::Extra {
            near_shop: false,
            potion_price: 50,
            heal_hp,
            ..Default::default()
        },
    );
    let snap = Snapshot {
        heal_hp,
        heal_items: knowledge.heal_items(&obs),
        loot_items: obs
            .inventory
            .iter()
            .filter(|i| goap::state::is_loot(&i.kind, i.equipped))
            .map(|i| i.amount)
            .sum(),
        weight_frac: obs.me.weight as f32 / agent::max_weight(&obs.me.job, obs.me.stats.str) as f32,
        obs,
        ws,
        last_outcome: "llm-check".into(),
        journal: memory.journal.clone(),
        visited: memory.visited_maps.iter().cloned().collect(),
        kills: memory.counters.kills,
        deaths: memory.counters.deaths,
        ..Default::default()
    };
    Ok((snap, memory))
}

async fn llm_check(
    cli: &Cli,
    knowledge: Knowledge,
    order: strategist::prompt::Order,
) -> Result<()> {
    use strategist::{Step, llm_step, options::build_options, prompt};
    let (snap, memory) = live_snapshot(cli, &knowledge).await?;
    let options = build_options(&snap, &knowledge, &memory.strategy).await?;
    let user = prompt::user_message(order, &memory.strategy, &snap, &options);
    println!(
        "--- system ({} chars) ---
{}
--- user ({} chars) ---
{user}",
        prompt::SYSTEM.len(),
        prompt::SYSTEM,
        user.len()
    );
    let mut llm = Llm::new(
        &cli.endpoints.llm_url,
        &cli.endpoints.llm_model,
        cli.endpoints.llm_think,
        &cli.endpoints.sampling,
    )?;
    llm.order = order;
    for i in 1..=3 {
        let step = Step {
            llm: &llm,
            knowledge: &knowledge,
            snap: &snap,
            options: &options,
        };
        match llm_step(step, &memory.strategy).await {
            Ok(d) => {
                let mut state = memory.strategy.clone();
                let report = state.apply_patch(&d.patch);
                println!(
                    "--- try {i}: {} in {} ms ---
reason: {}
lookups: {:?}
patch: {}
applied {:?} rejected {:?}",
                    d.option.id,
                    d.millis,
                    d.reason,
                    d.lookups,
                    d.patch,
                    report.applied,
                    report.rejected
                );
            }
            Err(error) => println!("--- try {i}: failed: {error:#}"),
        }
    }
    let fallback = strategist::options::fallback(&snap, &options, &memory.strategy);
    println!("--- fallback would pick: {:?}", fallback.map(|o| o.id));
    Ok(())
}

/// History-mode turns on one snapshot, to see what the session and its compaction do.
async fn history_check(
    cli: &Cli,
    knowledge: Knowledge,
    turns: u32,
    compact_at: usize,
    compact_to: usize,
    model_compaction: bool,
) -> Result<()> {
    use strategist::options::build_options;
    let (mut snap, memory) = live_snapshot(cli, &knowledge).await?;
    let options = build_options(&snap, &knowledge, &memory.strategy).await?;
    let llm = Llm::new(
        &cli.endpoints.llm_url,
        &cli.endpoints.llm_model,
        cli.endpoints.llm_think,
        &cli.endpoints.sampling,
    )?;
    let settings = strategist::history::HistorySettings {
        compact_at,
        compact_to,
        model_compaction,
        trace: true,
    };
    let mut history = strategist::history::History::with(&llm, settings);
    for turn in 1..=turns {
        snap.goal_secs = 60 * turn as u64;
        let start = std::time::Instant::now();
        let step = strategist::Step {
            llm: &llm,
            knowledge: &knowledge,
            snap: &snap,
            options: &options,
        };
        let result = history.step(step, &memory.strategy).await;
        println!(
            "=== turn {turn} ({} ms, {} tokens after) ===",
            start.elapsed().as_millis(),
            history.estimated_tokens()
        );
        for line in history.drain() {
            println!("{line}");
        }
        match result {
            Ok(d) => println!("--> {} ({})", d.option.id, d.reason),
            Err(error) => println!("--> failed: {error:#}"),
        }
    }
    Ok(())
}

async fn db_command(cli: &Cli, knowledge: &Knowledge, path: &str, arg: &str) -> Result<()> {
    if path == "grounds" {
        let api = BotApi::new(&cli.endpoints.url, cli.endpoints.load_key()?)?;
        let obs = api.observation(cli.name()).await?;
        let fighter = knowledge.fighter(&obs).await?;
        let from = if arg.is_empty() {
            obs.map.name.clone()
        } else {
            arg.to_owned()
        };
        println!("{fighter:?}");
        let grounds = knowledge::grounds::Hunt::new(knowledge, &fighter, &from, 4)
            .hunting_grounds(10)
            .await?;
        for g in grounds {
            println!("{:6.3} {}", g.score, g.describe());
        }
        return Ok(());
    }
    // `db farm <item id>[@map]`: where the bot would hunt for an item, from its map or the one given.
    if path == "farm" {
        let api = BotApi::new(&cli.endpoints.url, cli.endpoints.load_key()?)?;
        let obs = api.observation(cli.name()).await?;
        let fighter = knowledge.fighter(&obs).await?;
        let (item, from) = arg.split_once('@').unwrap_or((arg, obs.map.name.as_str()));
        let item = knowledge.db.item(item.parse()?).await?;
        let farm = knowledge::grounds::Hunt::new(knowledge, &fighter, from, 6)
            .farm_ground(&item)
            .await?;
        match farm {
            Some((g, mob)) => println!(
                "{} from {from}: hunt {mob} on {}",
                item.name_english,
                g.describe()
            ),
            None => println!("{} from {from}: nowhere", item.name_english),
        }
        return Ok(());
    }
    if path == "gear" {
        let api = BotApi::new(&cli.endpoints.url, cli.endpoints.load_key()?)?;
        let obs = api.observation(cli.name()).await?;
        let budget = behaviors::gear::gear_budget(obs.me.zeny, 50);
        println!("{:?}", knowledge::gear::Worn::of(&obs));
        println!(
            "budget {budget}: {:#?}",
            knowledge
                .best_upgrade(
                    &obs,
                    &knowledge::gear::Shopping {
                        budget,
                        max_hops: 10,
                        ..Default::default()
                    }
                )
                .await?
        );
        return Ok(());
    }
    if path == "go" {
        let town = knowledge
            .graph
            .nearest_upstream(arg, behaviors::travel::GO_TOWNS, 6)
            .await?;
        println!("{town:?}");
        return Ok(());
    }
    if path == "route" {
        let (from, to) = arg.split_once(':').unwrap_or(("", arg));
        let route = knowledge.graph.route(from, to, &HashSet::new()).await?;
        println!("{route:#?}");
        return Ok(());
    }
    // Git Bash rewrites a leading `/db/...` into a Windows path, so also accept `maps/prt_fild08`.
    let path = match path.find("/db/") {
        Some(i) => path[i..].to_owned(),
        None => format!("/db/{}", path.trim_start_matches('/')),
    };
    let v = knowledge.db.get(&path).await?;
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(())
}

/// Connects the character, creating it first if it does not exist yet.
pub async fn ensure_connected(api: &BotApi, name: &str) -> Result<()> {
    let bots = api.list().await?;
    if bots
        .iter()
        .any(|b| b.name.eq_ignore_ascii_case(name) && b.connected)
    {
        tracing::info!(name, "already connected");
        return Ok(());
    }
    match api.connect(name).await {
        Ok(_) => tracing::info!(name, "connected"),
        Err(error) => {
            tracing::info!(name, %error, "connect failed, creating the character");
            api.create(name).await?;
        }
    }
    Ok(())
}

/// The first observation after connecting can be incomplete, so poll until `ready`.
pub async fn wait_ready(api: &BotApi, name: &str) -> Result<Observation> {
    for _ in 0..40 {
        let obs = api.observation(name).await?;
        if obs.ready {
            return Ok(obs);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    bail!("bot {name} did not become ready within 20 s")
}

fn print_summary(obs: &Observation) {
    let me = &obs.me;
    let s = &me.stats;
    println!("== {} (char {}) ==", me.name, me.char_id);
    println!(
        "job {} | base lv {} | job lv {} | hp {}/{} | sp {}/{} | zeny {} | weight {}",
        me.job, me.base_level, me.job_level, me.hp, me.max_hp, me.sp, me.max_sp, me.zeny, me.weight
    );
    println!(
        "stats str {} agi {} vit {} int {} dex {} luk {} | status points {} | skill points {}",
        s.str, s.agi, s.vit, s.int, s.dex, s.luk, me.status_points, me.skill_points
    );
    println!(
        "map {} at ({}, {}) | action {} | dead {}",
        obs.map.name, me.x, me.y, me.action, me.dead
    );
    println!(
        "mobs {}, npcs {}, items {}, players {}",
        obs.mobs.len(),
        obs.npcs.len(),
        obs.items.len(),
        obs.players.len()
    );
    for w in &obs.warps {
        println!(
            "  warp #{} at ({}, {}) -> {} ({}, {})",
            w.id, w.x, w.y, w.to_map, w.to_x, w.to_y
        );
    }
    for n in obs.npcs.iter().take(20) {
        println!("  npc #{} {} at ({}, {})", n.id, n.name, n.x, n.y);
    }
    for i in &obs.inventory {
        println!(
            "  [{}] {} x{} kind={}{}",
            i.index,
            i.name,
            i.amount,
            i.kind,
            if i.equipped { " (equipped)" } else { "" }
        );
    }
    for sk in &obs.skills {
        println!("  skill {} lv{} aim={}", sk.name, sk.level, sk.aim);
    }
}
