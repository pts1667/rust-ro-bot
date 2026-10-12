//! The two parts of a SKILL.state prompt besides Σ: the immutable specification P (system prompt) and the
//! observation O, a short digest built by code from the game state. No history is ever sent.

use std::fmt::Write;

use super::Snapshot;
use super::options::Opt;
use super::skill_state::StrategicState;

pub const SYSTEM: &str = "\
YOUR TASK: each turn, choose the next goal of a Ragnarok Online character by calling `commit` once with one option id \
from OPTIONS. A program plays the character: it walks, fights, loots, rests, trades and spends points by itself, and \
then works on the goal you chose until you choose again.

Each turn gives you:
- TASK and OPTIONS: the goals you can choose from now.
- STATE: your own notes as JSON. They are the only thing kept between turns.
- OBSERVATION: the character now and recent events.

How to answer:
- Call `commit` exactly once with: `analysis` (compare the options against the guidelines below in one or two sentences), then `option` (one id from OPTIONS, the one your analysis favours), then `state_patch`.
- Before committing you may call `monster_info`, `map_info`, `wiki_page` or `quest_info` (up to eight per round, eight rounds; look things up only when the answer could change your choice). `quest_info` has guides, e.g. for job change quests.
- `state_patch` holds only keys that change, `{}` when nothing does. List keys ADD entries; \"-name\" removes one.
  Keys: objective (a long-term aim in words; never an option id), stat_build (melee|tank|caster|archer|balanced), job_target (the next job: a first job like Swordman or Mage for a Novice, a second job like Knight or Priest after it), avoid_maps, avoid_mobs, notes.

Guidelines:
- Staying alive matters more than speed: prefer threat 0% maps; after deaths on a map, add it to avoid_maps or pick another.
- Keep hunting on the current map while it works; switch when a listed map has clearly better exp/s (1.5 times or more) at low threat.
- Healing is counted in HP: \"1.0x max HP\" means your healing items give back your max HP once. Buy potions below 1.0x when you can afford them. Sell loot when weight is above 40%, or when healing is below 1.0x and you have too little zeny to buy potions.
- When job_change options appear, take the one matching job_target (or stat_build: melee/tank -> Swordman, caster -> Mage, archer -> Archer).
- Set an objective early and keep it until it is reached.
- notes are for lessons learned (\"Lunatics flee, Porings are faster kills\"), never for numbers already in OBSERVATION.

Game primer (Ragnarok Online, pre-renewal rules; this server gives 5x experience):
- Killing monsters gives base experience (base level, up to 99) and job experience (job level). Dying costs 1% of the base experience needed for the next level and sends you back to town.
- Stats: STR melee damage and carry weight; AGI attack speed and dodging; VIT max HP, HP regeneration and healing item power; INT max SP and magic; DEX hit chance, bow damage and cast time; LUK critical hits. Each level gives status points; high stats cost more points.
- Jobs: everyone starts as a Novice. At job level 10 a Novice becomes one of six first jobs: Swordman (melee, tough), Mage (magic, fragile), Archer (bows, DEX), Acolyte (healing and support), Merchant (trade, carries more), Thief (AGI, dodging). At job level 40 or more a first job becomes a second job: Swordman -> Knight or Crusader, Mage -> Wizard or Sage, Archer -> Hunter or Bard/Dancer, Acolyte -> Priest or Monk, Merchant -> Blacksmith or Alchemist, Thief -> Assassin or Rogue. Job level starts again at 1 after a change.
- Monsters: passive ones fight only when attacked, aggressive ones attack on sight, bosses are deadly. Monsters far above your level hit hard and are hard to hit.
- Items: loot (Etc items like Jellopy) is only for selling to NPC shops. Potions and food heal HP. Above 50% weight HP stops regenerating; above 90% you cannot attack.
- Maps: towns (prontera, izlude, geffen, payon, morocc, alberta) are safe and have shops. Fields like prt_fild08 have monsters, and the threat % tells how dangerous they are for you. Dungeons are harder.";

/// The observation part of the prompt.
pub fn observation(snap: &Snapshot) -> String {
    let me = &snap.obs.me;
    let s = &me.stats;
    let mut o = String::new();
    let _ = writeln!(
        o,
        "Character: {} the {} (base level {}, job level {}), HP {}/{} ({:.0}%), SP {}/{}, zeny {}, weight {:.0}%.",
        me.name,
        me.job,
        me.base_level,
        me.job_level,
        me.hp,
        me.max_hp,
        me.hp_frac() * 100.0,
        me.sp,
        me.max_sp,
        me.zeny,
        snap.weight_frac * 100.0
    );
    let _ = writeln!(
        o,
        "Stats: str {} agi {} vit {} int {} dex {} luk {}. Unspent: {} status points, {} skill points.",
        s.str, s.agi, s.vit, s.int, s.dex, s.luk, me.status_points, me.skill_points
    );
    let _ = writeln!(
        o,
        "Map: {} ({} monsters, {} players). Healing items {} (heal {} HP, {:.1}x max HP), loot items {}.",
        snap.obs.map.name,
        snap.obs.mobs.len(),
        snap.obs.players.len(),
        snap.heal_items,
        snap.heal_hp,
        super::options::heal_ratio(snap),
        snap.loot_items
    );
    match &snap.goal {
        Some(goal) => {
            let _ = writeln!(o, "Current goal: {} for {} s. Last result: {}.", goal.label(), snap.goal_secs, snap.last_outcome);
        }
        None => {
            let _ = writeln!(o, "No current goal. Last result: {}.", snap.last_outcome);
        }
    }
    let top = |m: &[(String, u64)]| m.iter().take(4).map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ");
    let _ = writeln!(
        o,
        "Totals: {} kills ({}), {} deaths ({}).",
        snap.kills,
        top(&snap.kills_by_mob),
        snap.deaths,
        top(&snap.deaths_by_cause)
    );
    if !snap.journal.is_empty() {
        let _ = writeln!(o, "Recent events:");
        for line in snap.journal.iter().rev().take(6).rev() {
            let _ = writeln!(o, "- {line}");
        }
    }
    o
}

/// The options, one per line.
pub fn options_text(options: &[Opt]) -> String {
    options.iter().map(|o| format!("- {}: {}\n", o.id, o.line)).collect()
}

/// Where the task and the options go in a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Order {
    /// The task and the options first, then Σ and O: the model knows what it reads the data for.
    #[default]
    TaskFirst,
    /// Σ and O first, the options last (the first layout).
    DataFirst,
}

/// One turn, real or example, from its parts.
pub fn render_turn(order: Order, state: &str, observation: &str, options: &str) -> String {
    let observation = observation.trim_end();
    match order {
        Order::TaskFirst => format!(
            "TASK: choose the next goal. Call `commit` with your analysis of the OPTIONS, the option id it favours, and a state_patch.\n\nOPTIONS:\n{options}\nSTATE:\n{state}\n\nOBSERVATION:\n{observation}"
        ),
        Order::DataFirst => format!("STATE:\n{state}\n\nOBSERVATION:\n{observation}\n\nOPTIONS:\n{options}"),
    }
}

/// The user message of a real turn.
pub fn user_message(order: Order, state: &StrategicState, snap: &Snapshot, options: &[Opt]) -> String {
    render_turn(order, &state.to_prompt_json(), &observation(snap), &options_text(options))
}
