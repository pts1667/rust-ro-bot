mod agent;
mod api;
mod behaviors;
mod bot;
mod config;
mod forum;
mod goap;
mod knowledge;
mod memory;
mod party;
mod quests;
mod social;
mod strategist;
mod voice;

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::Parser;
use tracing_subscriber::EnvFilter;

use api::{BotApi, Observation};
use bot::Bot;
use config::{Cli, Command, RunArgs};
use knowledge::Knowledge;
use knowledge::db::Db;
use memory::Memory;
use strategist::helper::Helper;
use strategist::llm::Llm;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    // A name read from a file with Windows line ends keeps its `\r`, and the server makes a character of it
    if cli.name.trim().is_empty()
        || cli.name.chars().any(|c| c.is_control())
        || cli.name != cli.name.trim()
    {
        bail!(
            "bad character name {:?}: no control characters or spaces around it",
            cli.name
        );
    }
    let db = Db::new(&cli.endpoints.db_url, Some(cli.endpoints.cache_dir()))?;
    let knowledge = Knowledge::new(db);

    match &cli.command {
        Command::Db { path, arg } => db_command(&cli, &knowledge, path, arg).await,
        Command::Probe => {
            let api = BotApi::new(&cli.endpoints.url, cli.endpoints.load_key()?)?;
            ensure_connected(&api, &cli.name).await?;
            let obs = wait_ready(&api, &cli.name).await?;
            print_summary(&obs);
            Ok(())
        }
        Command::Run(args) => run(&cli, knowledge, args.clone()).await,
        Command::Travel { to } => {
            let api = BotApi::new(&cli.endpoints.url, cli.endpoints.load_key()?)?;
            ensure_connected(&api, &cli.name).await?;
            wait_ready(&api, &cli.name).await?;
            let memory = Memory::load(&cli.endpoints.state_dir(), &cli.name)?;
            knowledge.graph.forbid(memory.bad_warps.iter().cloned());
            let mut bot = Bot::new(api, cli.name.clone(), knowledge, memory, None);
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

async fn run(cli: &Cli, knowledge: Knowledge, args: RunArgs) -> Result<()> {
    let api = BotApi::new(&cli.endpoints.url, cli.endpoints.load_key()?)?;
    ensure_connected(&api, &cli.name).await?;
    wait_ready(&api, &cli.name).await?;
    let memory = Memory::load(&cli.endpoints.state_dir(), &cli.name)?;
    knowledge.graph.forbid(memory.bad_warps.iter().cloned());

    let llm = if args.no_llm || args.voice != voice::VoiceMode::Llm {
        None
    } else {
        let mut llm = Llm::new(
            &cli.endpoints.llm_url,
            &cli.endpoints.llm_model,
            cli.endpoints.llm_think,
            &cli.endpoints.sampling,
        )?;
        llm.share = args.llm_share.clamp(0.0, 1.0);
        llm.order = args.order;
        llm.context = args.context;
        if llm.healthy().await {
            Some(llm)
        } else {
            tracing::warn!(url = %cli.endpoints.llm_url, "the LLM does not answer: playing with the fallback strategist");
            None
        }
    };
    let voice = match args.voice {
        _ if args.no_llm => None,
        voice::VoiceMode::Llm => llm.clone().map(voice::Voice::Llm),
        voice::VoiceMode::SystemOne => {
            let s1 = voice::systemone::SystemOne::new(&cli.endpoints.s1_url)?;
            if s1.healthy().await {
                tracing::info!(url = %cli.endpoints.s1_url, share = args.llm_share, "the decision model talks and picks a share of the goals");
                Some(voice::Voice::SystemOne(s1))
            } else {
                tracing::warn!(url = %cli.endpoints.s1_url, "the decision model does not answer: playing without a voice");
                None
            }
        }
    };
    let helper = voice.clone().map(Helper::new);
    let mut bot = Bot::new(api, cli.name.clone(), knowledge.clone(), memory, helper);
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
        Some(voice::Voice::SystemOne(s1)) => Some(strategist::decided::Decider::new(
            s1.clone(),
            args.llm_share.clamp(0.0, 1.0),
        )),
        _ => None,
    };
    let handle = strategist::spawn(
        knowledge,
        llm,
        decider,
        bot.memory.strategy.clone(),
        Duration::from_secs(args.strategist_period.max(5)),
        bot.latest_directive.clone(),
    );
    // The forum is read and written with a model only: without one the bot has nothing to say there
    if let Some(voice) = forum_voice {
        let api =
            api::forum::ForumApi::new(&cli.endpoints.url, cli.endpoints.load_key()?, &cli.name)?;
        forum::spawn(
            api,
            voice,
            bot.knowledge.clone(),
            handle.snapshots.subscribe(),
            handle.events.clone(),
            &cli.endpoints.state_dir(),
        );
    }
    agent::run(bot, handle, args).await
}

/// One strategist step on the current game state, printing the prompt and the decision.
/// The strategist's view of the bot as it is now, without running it.
async fn live_snapshot(cli: &Cli, knowledge: &Knowledge) -> Result<(strategist::Snapshot, Memory)> {
    use strategist::Snapshot;
    let api = BotApi::new(&cli.endpoints.url, cli.endpoints.load_key()?)?;
    ensure_connected(&api, &cli.name).await?;
    let obs = wait_ready(&api, &cli.name).await?;
    let memory = Memory::load(&cli.endpoints.state_dir(), &cli.name)?;
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
    use strategist::{llm_step, options::build_options, prompt};
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
        match llm_step(&llm, &knowledge, &memory.strategy, &snap, &options).await {
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
    let mut history =
        strategist::history::History::with(&llm, compact_at, compact_to, model_compaction, true);
    for turn in 1..=turns {
        snap.goal_secs = 60 * turn as u64;
        let start = std::time::Instant::now();
        let result = history
            .step(&llm, &knowledge, &memory.strategy, &snap, &options)
            .await;
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
        let obs = api.observation(&cli.name).await?;
        let fighter = knowledge.fighter(&obs).await?;
        let from = if arg.is_empty() {
            obs.map.name.clone()
        } else {
            arg.to_owned()
        };
        println!("{fighter:?}");
        let grounds = knowledge::grounds::hunting_grounds(
            &knowledge.db,
            &knowledge.graph,
            &fighter,
            &from,
            4,
            &HashSet::new(),
            &HashSet::new(),
            10,
        )
        .await?;
        for g in grounds {
            println!("{:6.3} {}", g.score, g.describe());
        }
        return Ok(());
    }
    // `db farm <item id>[@map]`: where the bot would hunt for an item, from its map or the one given.
    if path == "farm" {
        let api = BotApi::new(&cli.endpoints.url, cli.endpoints.load_key()?)?;
        let obs = api.observation(&cli.name).await?;
        let fighter = knowledge.fighter(&obs).await?;
        let (item, from) = arg.split_once('@').unwrap_or((arg, obs.map.name.as_str()));
        let item = knowledge.db.item(item.parse()?).await?;
        let farm = knowledge::grounds::farm_ground(
            &knowledge.db,
            &knowledge.graph,
            &fighter,
            from,
            &item,
            6,
            &HashSet::new(),
            &HashSet::new(),
        )
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
        let obs = api.observation(&cli.name).await?;
        let budget = behaviors::gear::gear_budget(obs.me.zeny, 50);
        println!("{:?}", knowledge::gear::Worn::of(&obs));
        println!(
            "budget {budget}: {:#?}",
            knowledge.best_upgrade(&obs, budget, 10, None, &[]).await?
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
