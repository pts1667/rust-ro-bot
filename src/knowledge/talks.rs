//! NPCs that send the bot to another map when asked: the guards out of moc_fild20, and the Kafra teleport service.

use super::db::MapData;

/// A talk that sends the bot on, in place of a warp: whom to ask, the menu entries that ask, what it costs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Talk {
    /// The NPC's name in the reference.
    pub npc: String,
    /// The names the NPCs of this way go by (`npc_named`): the one nearest to the bot is asked.
    pub names: Vec<String>,
    /// The menu entries to pick, in the order they come (lower case, part of the entry).
    pub keywords: Vec<String>,
    /// The zeny it takes.
    pub fare: u64,
    /// A route may go through it (a town's Kafra). A guard's way is only a way out for a bot already there: routes do
    /// not cross moc_fild20, a level 80 field.
    pub through: bool,
}

/// Whether an NPC called `full` (in the reference or an observation) is one called `name`: the same name, or `name`
/// and a hidden `#` part. `Kafra` is the Kafra of Morocc's north, not the `Kafra Voting Staff`.
pub fn npc_named(full: &str, name: &str) -> bool {
    let full = full.split("::").next().unwrap_or(full).trim();
    full == name
        || full
            .strip_prefix(name)
            .is_some_and(|rest| rest.starts_with('#'))
}

/// The name an NPC shows in an observation: without its hidden `#` part, so guards are told apart by where they stand.
pub fn shown_name(full: &str) -> &str {
    full.split("::")
        .next()
        .unwrap_or(full)
        .split('#')
        .next()
        .unwrap_or(full)
        .trim()
}

/// An NPC that sends anyone who asks to another map.
struct NpcWay {
    names: &'static [&'static str],
    to: &'static str,
    keywords: &'static [&'static str],
}

/// The NPC ways the bot knows to ask for. moc_fild20 has no warp out, only a Continental Guard at every landing, who
/// tells commoners to go and offers to send them to Morocc's camp; the pair at the north landing also send them back to
/// Sograt Desert 1, by Prontera Field (a custom way of this server, after iRO).
const NPC_WAYS: &[NpcWay] = &[
    NpcWay {
        names: &["Continental Guard"],
        to: "morocc",
        keywords: &["ask what happened", "ask about guard", "please do"],
    },
    NpcWay {
        names: &["Continental Guard#07", "Continental Guard#08"],
        to: "moc_fild01",
        keywords: &["prontera field"],
    },
];

/// The talk of the NPC `npc` of the reference to `to`, when the bot knows one.
pub fn npc_way(npc: &str, to: &str) -> Option<Talk> {
    let way = NPC_WAYS
        .iter()
        .find(|w| w.to == to && w.names.iter().any(|n| npc_named(npc, n)))?;
    Some(Talk {
        npc: npc.to_owned(),
        names: way.names.iter().map(|n| (*n).to_owned()).collect(),
        keywords: way.keywords.iter().map(|k| (*k).to_owned()).collect(),
        fare: 0,
        through: false,
    })
}

/// The names Kafra Employees go by.
const KAFRA_NAMES: &[&str] = &["Kafra Employee", "Kafra"];

/// Where the Kafra teleport service sends to: its name in the menu, the map and cell it lands on
/// (`F_KafTele` of the server's `kafras_functions_kafras.rs`).
const KAFRA_LANDINGS: &[(&str, &str, u16, u16)] = &[
    ("Al De Baran", "aldebaran", 168, 112),
    ("Alberta", "alberta", 117, 56),
    ("Comodo", "comodo", 209, 143),
    ("Comodo Pharos Beacon", "cmd_fild07", 127, 134),
    ("Geffen", "geffen", 120, 39),
    ("Izlude", "izlude", 91, 105),
    ("Juno", "yuno", 158, 125),
    ("Mjolnir Dead Pit", "mjolnir_02", 99, 351),
    ("Morocc", "morocc", 156, 46),
    ("Orc Dungeon", "gef_fild10", 52, 326),
    ("Payon", "payon", 161, 58),
    ("Prontera", "prontera", 116, 72),
    ("Umbala", "umbala", 100, 154),
];

/// The teleports of the Kafras of each map, with their fares (`F_KafSet`).
const KAFRA_TELEPORTS: &[(&str, &[(&str, u64)])] = &[
    (
        "prontera",
        &[
            ("Izlude", 600),
            ("Geffen", 1200),
            ("Payon", 1200),
            ("Morocc", 1200),
            ("Orc Dungeon", 1700),
            ("Alberta", 1800),
        ],
    ),
    (
        "alberta",
        &[("Payon", 1200), ("Morocc", 1800), ("Prontera", 1800)],
    ),
    (
        "aldebaran",
        &[
            ("Geffen", 1200),
            ("Juno", 1200),
            ("Izlude", 1800),
            ("Mjolnir Dead Pit", 1700),
        ],
    ),
    (
        "comodo",
        &[
            ("Morocc", 1800),
            ("Comodo Pharos Beacon", 1200),
            ("Umbala", 1800),
        ],
    ),
    ("cmd_fild07", &[("Comodo", 1200), ("Morocc", 1200)]),
    (
        "geffen",
        &[
            ("Prontera", 1200),
            ("Al De Baran", 1200),
            ("Orc Dungeon", 1700),
            ("Mjolnir Dead Pit", 1700),
        ],
    ),
    (
        "izlude",
        &[
            ("Geffen", 1200),
            ("Payon", 1200),
            ("Morocc", 1200),
            ("Al De Baran", 1800),
        ],
    ),
    (
        "morocc",
        &[
            ("Prontera", 1200),
            ("Payon", 1200),
            ("Alberta", 1800),
            ("Comodo", 1800),
            ("Comodo Pharos Beacon", 1200),
        ],
    ),
    ("umbala", &[("Comodo", 1800)]),
    (
        "payon",
        &[("Prontera", 1200), ("Alberta", 1200), ("Morocc", 1200)],
    ),
    ("yuno", &[("Al De Baran", 1200)]),
];

/// The Kafra teleports of `data`'s map, from each of its Kafras: `(npc, x, y, region, to, to_x, to_y, talk)`. A Kafra
/// that turns out to have no teleport service is left out of the routes once asked.
pub fn kafra_teleports(
    data: &MapData,
) -> Vec<(u16, u16, Option<u32>, &'static str, u16, u16, Talk)> {
    let Some((_, teleports)) = KAFRA_TELEPORTS.iter().find(|(m, _)| *m == data.name) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for npc in data
        .npcs
        .iter()
        .filter(|n| KAFRA_NAMES.iter().any(|k| npc_named(&n.npc.name, k)))
    {
        for (dest, fare) in teleports.iter() {
            let Some((_, map, x, y)) = KAFRA_LANDINGS.iter().find(|(d, ..)| d == dest) else {
                continue;
            };
            let talk = Talk {
                npc: npc.npc.name.clone(),
                names: KAFRA_NAMES.iter().map(|n| (*n).to_owned()).collect(),
                // The menu reads "Payon -> 1200 z"
                keywords: vec![
                    "use teleport service".into(),
                    format!("{} ->", dest.to_lowercase()),
                ],
                fare: *fare,
                through: true,
            };
            out.push((npc.x, npc.y, npc.region, *map, *x, *y, talk));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npcs_are_known_by_name_and_hidden_part() {
        assert!(npc_named("Continental Guard#03", "Continental Guard"));
        assert!(npc_named("Continental Guard#07", "Continental Guard#07"));
        assert!(!npc_named("Continental Guard#03", "Continental Guard#07"));
        assert!(npc_named("Kafra", "Kafra"));
        assert!(npc_named("Kafra::kaf_morocc2", "Kafra"));
        assert!(npc_named("Kafra Employee#morocc_156_97", "Kafra Employee"));
        assert!(!npc_named("Kafra Voting Staff#prt", "Kafra"));
        assert!(npc_way("Continental Guard#07", "moc_fild01").is_some());
        assert!(npc_way("Continental Guard#03", "moc_fild01").is_none());
        assert!(npc_way("Continental Guard#03", "morocc").is_some());
    }

    #[test]
    fn every_kafra_teleport_lands_somewhere() {
        for (town, teleports) in KAFRA_TELEPORTS {
            for (dest, _) in teleports.iter() {
                assert!(
                    KAFRA_LANDINGS.iter().any(|(d, ..)| d == dest),
                    "{town}: {dest}"
                );
            }
        }
    }
}
