//! The map graph made of the warps of `/db/maps/<name>`, and routes through it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, RwLock};

use anyhow::Result;

use super::db::Db;

/// One warp: stepping on (`x`, `y`) of `from` lands on (`to_x`, `to_y`) of `to`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub from: String,
    pub x: u16,
    pub y: u16,
    pub to: String,
    pub to_x: u16,
    pub to_y: u16,
    /// Not a warp of the map but the way back of a warp landing there (`arrivals`), which may not exist.
    pub guessed: bool,
}

impl Edge {
    /// `from:x,y>to`, the key of a warp in the bad-warp list.
    pub fn key(&self) -> String {
        format!("{}:{},{}>{}", self.from, self.x, self.y, self.to)
    }

    /// `from:*`, the bad-warp key that rules out every guessed way out of `from`.
    pub fn guessed_key(map: &str) -> String {
        format!("{map}:*")
    }
}

/// Most maps a single search loads, so a route to an unreachable map does not crawl the whole world.
const MAX_EXPLORED: usize = 600;

/// Lazily loaded warp graph.
#[derive(Clone)]
pub struct WorldGraph {
    db: Db,
    /// Warps the bot could not take (keys of `Edge::key`), left out of every route. Shared by all clones.
    bad: Arc<RwLock<HashSet<String>>>,
}

impl WorldGraph {
    pub fn new(db: Db) -> Self {
        Self { db, bad: Default::default() }
    }

    /// Leaves these warps out of routes from now on.
    pub fn forbid<I: IntoIterator<Item = String>>(&self, keys: I) {
        self.bad.write().unwrap().extend(keys);
    }

    pub async fn edges(&self, map: &str) -> Result<Vec<Edge>> {
        let data = self.db.map(map).await?;
        let mut edges: Vec<Edge> = data
            .warps
            .iter()
            .filter(|w| !w.to.map.id_str().is_empty())
            .map(|w| Edge {
                from: map.to_owned(),
                x: w.x,
                y: w.y,
                to: w.to.map.id_str(),
                to_x: w.to.x,
                to_y: w.to.y,
                guessed: false,
            })
            .collect();
        // Some maps list no warps of their own (moc_fild20), only the warps that land on them. Warps come in pairs,
        // so the way back is next to where a warp lands; travel then takes the real warp from the observation.
        for a in &data.arrivals {
            let to = a.from.map.id_str();
            if to.is_empty() || to == map || edges.iter().any(|e| e.to == to) {
                continue;
            }
            edges.push(Edge { from: map.to_owned(), x: a.land.x, y: a.land.y, to, to_x: a.from.x, to_y: a.from.y, guessed: true });
        }
        let bad = self.bad.read().unwrap();
        let no_guesses = bad.contains(&Edge::guessed_key(map));
        edges.retain(|e| !bad.contains(&e.key()) && !(e.guessed && no_guesses));
        Ok(edges)
    }

    /// Fewest-warps route from `from` to `to`, avoiding the maps of `avoid` (except `to` itself).
    pub async fn route(&self, from: &str, to: &str, avoid: &HashSet<String>) -> Result<Option<Vec<Edge>>> {
        self.route_skipping(from, to, avoid, &HashSet::new()).await
    }

    /// `route`, also leaving out the warps of `skip` (keys of `Edge::key`).
    pub async fn route_skipping(
        &self,
        from: &str,
        to: &str,
        avoid: &HashSet<String>,
        skip: &HashSet<String>,
    ) -> Result<Option<Vec<Edge>>> {
        if from == to {
            return Ok(Some(Vec::new()));
        }
        let mut prev: HashMap<String, Edge> = HashMap::new();
        let mut seen: HashSet<String> = HashSet::from([from.to_owned()]);
        let mut queue = VecDeque::from([from.to_owned()]);
        while let Some(map) = queue.pop_front() {
            if seen.len() > MAX_EXPLORED {
                break;
            }
            let Ok(edges) = self.edges(&map).await else { continue };
            for edge in edges {
                if seen.contains(&edge.to) || (avoid.contains(&edge.to) && edge.to != to) || skip.contains(&edge.key()) {
                    continue;
                }
                seen.insert(edge.to.clone());
                prev.insert(edge.to.clone(), edge.clone());
                if edge.to == to {
                    return Ok(Some(unwind(&prev, from, to)));
                }
                queue.push_back(edge.to.clone());
            }
        }
        Ok(None)
    }

    /// The first of `candidates` found walking back from `to` along real warps (the `arrivals` of each map), with its
    /// distance in warps: a map with a way to `to` that is not a guess.
    pub async fn nearest_upstream(&self, to: &str, candidates: &[&str], max_hops: u32) -> Result<Option<(String, u32)>> {
        let mut dist: HashMap<String, u32> = HashMap::from([(to.to_owned(), 0)]);
        let mut queue = VecDeque::from([to.to_owned()]);
        while let Some(map) = queue.pop_front() {
            let d = dist[&map];
            if candidates.contains(&map.as_str()) {
                return Ok(Some((map, d)));
            }
            if d >= max_hops || dist.len() > MAX_EXPLORED {
                continue;
            }
            let Ok(data) = self.db.map(&map).await else { continue };
            let bad = self.bad.read().unwrap().clone();
            for a in &data.arrivals {
                let from = a.from.map.id_str();
                let key = format!("{from}:{},{}>{map}", a.from.x, a.from.y);
                if from.is_empty() || bad.contains(&key) || dist.contains_key(&from) {
                    continue;
                }
                // Arrivals also list NPC transports (ra_in01 to morocc): only a warp the other map lists counts.
                let listed = match self.db.map(&from).await {
                    Ok(source) => source.warps.iter().any(|w| w.to.map.id_str() == map && w.x == a.from.x && w.y == a.from.y),
                    Err(_) => false,
                };
                if !listed {
                    continue;
                }
                dist.insert(from.clone(), d + 1);
                queue.push_back(from);
            }
        }
        Ok(None)
    }

    /// Maps reachable from `from` within `max_hops` warps, with their distance in warps.
    pub async fn neighbourhood(&self, from: &str, max_hops: u32) -> Result<Vec<(String, u32)>> {
        let mut dist: HashMap<String, u32> = HashMap::from([(from.to_owned(), 0)]);
        let mut queue = VecDeque::from([from.to_owned()]);
        while let Some(map) = queue.pop_front() {
            let d = dist[&map];
            if d >= max_hops || dist.len() > MAX_EXPLORED {
                continue;
            }
            let Ok(edges) = self.edges(&map).await else { continue };
            for edge in edges {
                if !dist.contains_key(&edge.to) {
                    dist.insert(edge.to.clone(), d + 1);
                    queue.push_back(edge.to);
                }
            }
        }
        let mut out: Vec<_> = dist.into_iter().collect();
        out.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        Ok(out)
    }
}

fn unwind(prev: &HashMap<String, Edge>, from: &str, to: &str) -> Vec<Edge> {
    let mut path = Vec::new();
    let mut cur = to.to_owned();
    while cur != from {
        let edge = prev[&cur].clone();
        cur = edge.from.clone();
        path.push(edge);
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(from: &str, to: &str) -> Edge {
        Edge { from: from.into(), x: 0, y: 0, to: to.into(), to_x: 0, to_y: 0, guessed: false }
    }

    #[test]
    fn unwind_rebuilds_path_in_order() {
        let mut prev = HashMap::new();
        prev.insert("b".to_string(), edge("a", "b"));
        prev.insert("c".to_string(), edge("b", "c"));
        let path = unwind(&prev, "a", "c");
        assert_eq!(path, vec![edge("a", "b"), edge("b", "c")]);
    }
}
