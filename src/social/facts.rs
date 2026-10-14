//! What the game reference says about the monsters and items a line names, for the LLM to answer with: a player
//! would know it, or look it up.

use crate::knowledge::Knowledge;

/// Reference searches per line, at most.
const LOOKUPS: usize = 8;
const FACTS: usize = 3;
/// Words that are no names: only the others are looked up, alone or two or three in a row.
const STOP: &[&str] = &[
    "the", "and", "but", "you", "your", "are", "was", "were", "is", "where", "what", "when",
    "which", "who", "why", "how", "can", "could", "would", "should", "will", "have", "has", "had",
    "with", "for", "from", "this", "that", "these", "those", "there", "here", "find", "get", "got",
    "need", "want", "know", "does", "did", "some", "any", "good", "hunt", "hunting", "kill",
    "drop", "drops", "level", "please", "pls", "nice", "cool", "great", "hello", "hey", "thanks",
    "thank", "yes", "yeah", "okay", "just", "like", "really", "very", "much", "many", "also",
    "well", "think", "maybe", "about", "into", "over", "more", "most", "other", "than", "too",
    "them", "they", "their", "then", "today", "now", "sure", "going", "come", "help", "buy",
    "sell", "where's", "what's", "it's", "i'm", "don't", "live", "lives", "spawn", "spawns", "map",
    "farm", "best", "place",
];

/// Whether `word` may be part of a name.
fn content(word: &str) -> bool {
    word.len() >= 3 && !STOP.contains(&word.to_lowercase().as_str())
}

/// What the reference says about a monster or an item.
#[derive(Debug, Clone, PartialEq)]
pub enum Lore {
    Monster {
        name: String,
        level: u32,
        hp: u32,
        aggressive: bool,
        /// Where it lives, as (map, amount), most first.
        maps: Vec<(String, u32)>,
        drops: Vec<String>,
    },
    Item {
        name: String,
        /// The monsters that drop it most often, bosses left out.
        droppers: Vec<String>,
        /// As (map, price).
        shops: Vec<(String, u64)>,
    },
}

impl Lore {
    /// The fact in a line, for the LLM.
    pub fn describe(&self) -> String {
        match self {
            Lore::Monster {
                name,
                level,
                hp,
                aggressive,
                maps,
                drops,
            } => {
                let places: Vec<String> =
                    maps.iter().map(|(map, n)| format!("{map} ({n})")).collect();
                format!(
                    "{name}: monster, level {level}, HP {hp}, {}; lives on {}; drops {}",
                    if *aggressive {
                        "attacks on sight"
                    } else {
                        "passive"
                    },
                    if places.is_empty() {
                        "no map".into()
                    } else {
                        places.join(", ")
                    },
                    if drops.is_empty() {
                        "nothing".into()
                    } else {
                        drops.join(", ")
                    }
                )
            }
            Lore::Item {
                name,
                droppers,
                shops,
            } => {
                let shops: Vec<String> = shops
                    .iter()
                    .map(|(map, price)| format!("{map} for {price} z"))
                    .collect();
                format!(
                    "{name}: item; dropped by {}; sold on {}",
                    if droppers.is_empty() {
                        "no monster".into()
                    } else {
                        droppers.join(", ")
                    },
                    if shops.is_empty() {
                        "no shop".into()
                    } else {
                        shops.join(", ")
                    }
                )
            }
        }
    }
}

/// What the reference says about the monsters and items `text` names.
pub async fn lore(k: &Knowledge, text: &str) -> Vec<Lore> {
    let words = words(text);
    let mut facts = Vec::new();
    let mut covered = vec![false; words.len()];
    for (start, len) in spans(&words).into_iter().take(LOOKUPS) {
        if facts.len() >= FACTS {
            break;
        }
        if covered[start..start + len].iter().any(|c| *c) {
            continue;
        }
        if let Some(fact) = fact_about(k, &words[start..start + len].join(" ")).await {
            facts.push(fact);
            covered[start..start + len]
                .iter_mut()
                .for_each(|c| *c = true);
        }
    }
    facts
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '-'))
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The runs of words (start, length) that may be a name, in the order to look them up: longest first, so
/// "Red Potion" comes before "Potion".
fn spans(words: &[String]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for len in (1..=3).rev() {
        for start in 0..words.len().saturating_sub(len - 1) {
            if words[start..start + len].iter().all(|w| content(w)) {
                out.push((start, len));
            }
        }
    }
    out
}

/// The fact about the monster or item named `phrase` (or its singular), when one is named exactly so.
async fn fact_about(k: &Knowledge, phrase: &str) -> Option<Lore> {
    let singular = phrase.strip_suffix('s').filter(|s| s.len() >= 3);
    for category in ["mobs", "items"] {
        let Ok(hits) = k.db.search(category, singular.unwrap_or(phrase), 3).await else {
            continue;
        };
        let Some(hit) = hits.iter().find(|h| {
            h.name.eq_ignore_ascii_case(phrase)
                || singular.is_some_and(|s| h.name.eq_ignore_ascii_case(s))
        }) else {
            continue;
        };
        let Some(id) = hit.id_u32() else { continue };
        return if category == "mobs" {
            monster(k, id).await
        } else {
            item(k, id).await
        };
    }
    None
}

async fn monster(k: &Knowledge, id: u32) -> Option<Lore> {
    let m = k.db.mob(id).await.ok()?;
    let mut spawns: Vec<(u32, String)> = m
        .spawns
        .iter()
        .map(|s| (s.amount, s.map.id_str()))
        .collect();
    spawns.sort_by(|a, b| b.0.cmp(&a.0));
    spawns.dedup_by(|a, b| a.1 == b.1);
    Some(Lore::Monster {
        name: m.name_english.clone(),
        level: m.level,
        hp: m.hp,
        aggressive: m.aggressive(),
        maps: spawns
            .into_iter()
            .take(4)
            .map(|(n, map)| (map, n))
            .collect(),
        drops: m
            .drops
            .iter()
            .filter(|d| !d.item.name.is_empty())
            .take(4)
            .map(|d| d.item.name.clone())
            .collect(),
    })
}

async fn item(k: &Knowledge, id: u32) -> Option<Lore> {
    let v = k.db.get(&format!("/db/items/{id}")).await.ok()?;
    let name = v["name_english"].as_str().unwrap_or_default();
    // The monsters that drop it most often, bosses left out
    let mut droppers: Vec<(u64, String)> = v["dropped_by"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|d| !d["mvp"].as_bool().unwrap_or(false))
                .filter_map(|d| {
                    Some((
                        d["rate"].as_u64().unwrap_or(0),
                        d["mob"]["name"].as_str()?.to_owned(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    droppers.sort_by(|a, b| b.0.cmp(&a.0));
    let mut dropped: Vec<String> = Vec::new();
    for (_, name) in droppers {
        if !dropped.contains(&name) && dropped.len() < 4 {
            dropped.push(name);
        }
    }
    let shops: Vec<(String, u64)> = v["sold_by"]
        .as_array()
        .map(|a| {
            a.iter()
                .take(2)
                .filter_map(|s| Some((s["map"]["id"].as_str()?.to_owned(), s["price"].as_u64()?)))
                .collect()
        })
        .unwrap_or_default();
    Some(Lore::Item {
        name: name.to_owned(),
        droppers: dropped,
        shops,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phrases(text: &str) -> Vec<String> {
        let w = words(text);
        spans(&w)
            .into_iter()
            .map(|(s, l)| w[s..s + l].join(" "))
            .collect()
    }

    #[test]
    fn names_are_looked_up_longest_first_among_content_words() {
        assert_eq!(
            phrases("nice! do you know where I can find Jellopy?"),
            vec!["Jellopy"]
        );
        assert_eq!(
            phrases("where do Orc Warriors live"),
            vec!["Orc Warriors", "Orc", "Warriors"]
        );
    }
}
