mod agent;
mod api;
mod behaviors;
mod bot;
mod config;
mod goap;
mod knowledge;
mod memory;
mod strategist;

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
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let cli = Cli::parse();
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
        Command::LlmCheck { order } => llm_check(&cli, knowledge, *order).await,
        Command::HistoryCheck { turns, compact_at, compact_to, model_compaction } => {
            history_check(&cli, knowledge, *turns, *compact_at, *compact_to, *model_compaction).await
        }
        Command::LlmEval { order, n, no_examples, no_shuffle } => {
            let mut llm = Llm::new(&cli.endpoints.llm_url, &cli.endpoints.llm_model, cli.endpoints.llm_think, &cli.endpoints.sampling)?;
            let orders = match order {
                Some(o) => vec![*o],
                None => vec![strategist::prompt::Order::TaskFirst, strategist::prompt::Order::DataFirst],
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

    let llm = if args.no_llm {
        None
    } else {
        let mut llm = Llm::new(&cli.endpoints.llm_url, &cli.endpoints.llm_model, cli.endpoints.llm_think, &cli.endpoints.sampling)?;
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
    let helper = llm.clone().map(Helper::new);
    let bot = Bot::new(api, cli.name.clone(), knowledge.clone(), memory, helper);
    let handle = strategist::spawn(
        knowledge,
        llm,
        bot.memory.strategy.clone(),
        Duration::from_secs(args.strategist_period.max(5)),
        bot.latest_directive.clone(),
    );
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
    let ws = goap::WorldState::from_observation(&obs, &goap::state::Extra { near_shop: false, potion_price: 50, heal_hp });
    let snap = Snapshot {
        heal_hp,
        heal_items: knowledge.heal_items(&obs),
        loot_items: obs.inventory.iter().filter(|i| goap::state::is_loot(&i.kind, i.equipped)).map(|i| i.amount).sum(),
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

async fn llm_check(cli: &Cli, knowledge: Knowledge, order: strategist::prompt::Order) -> Result<()> {
    use strategist::{llm_step, options::build_options, prompt};
    let (snap, memory) = live_snapshot(cli, &knowledge).await?;
    let options = build_options(&snap, &knowledge, &memory.strategy).await?;
    let user = prompt::user_message(order, &memory.strategy, &snap, &options);
    println!("--- system ({} chars) ---
{}
--- user ({} chars) ---
{user}", prompt::SYSTEM.len(), prompt::SYSTEM, user.len());
    let mut llm = Llm::new(&cli.endpoints.llm_url, &cli.endpoints.llm_model, cli.endpoints.llm_think, &cli.endpoints.sampling)?;
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
                    d.option.id, d.millis, d.reason, d.lookups, d.patch, report.applied, report.rejected
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
    let llm = Llm::new(&cli.endpoints.llm_url, &cli.endpoints.llm_model, cli.endpoints.llm_think, &cli.endpoints.sampling)?;
    let mut history = strategist::history::History::with(&llm, compact_at, compact_to, model_compaction, true);
    for turn in 1..=turns {
        snap.goal_secs = 60 * turn as u64;
        let start = std::time::Instant::now();
        let result = history.step(&llm, &knowledge, &memory.strategy, &snap, &options).await;
        println!("=== turn {turn} ({} ms, {} tokens after) ===", start.elapsed().as_millis(), history.estimated_tokens());
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
        let from = if arg.is_empty() { obs.map.name.clone() } else { arg.to_owned() };
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
    if path == "go" {
        let town = knowledge.graph.nearest_upstream(arg, behaviors::travel::GO_TOWNS, 6).await?;
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
    if bots.iter().any(|b| b.name.eq_ignore_ascii_case(name) && b.connected) {
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
    println!("map {} at ({}, {}) | action {} | dead {}", obs.map.name, me.x, me.y, me.action, me.dead);
    println!("mobs {}, npcs {}, items {}, players {}", obs.mobs.len(), obs.npcs.len(), obs.items.len(), obs.players.len());
    for w in &obs.warps {
        println!("  warp #{} at ({}, {}) -> {} ({}, {})", w.id, w.x, w.y, w.to_map, w.to_x, w.to_y);
    }
    for n in obs.npcs.iter().take(20) {
        println!("  npc #{} {} at ({}, {})", n.id, n.name, n.x, n.y);
    }
    for i in &obs.inventory {
        println!("  [{}] {} x{} kind={}{}", i.index, i.name, i.amount, i.kind, if i.equipped { " (equipped)" } else { "" });
    }
    for sk in &obs.skills {
        println!("  skill {} lv{} aim={}", sk.name, sk.level, sk.aim);
    }
}
