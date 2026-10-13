//! The map graph made of the warps of `/db/maps/<name>`, and routes through it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, RwLock};

use anyhow::Result;

use super::db::Db;

/// One warp: stepping on (`x`, `y`) of `from` lands on (`to_x`, `to_y`) of `to`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Edge {
    pub from: String,
    pub x: u16,
    pub y: u16,
    pub to: String,
    pub to_x: u16,
    pub to_y: u16,
    /// Not a warp of the map but the way back of a warp landing there (`arrivals`), which may not exist.
    pub guessed: bool,
    /// The walkable part of `from` the warp is stepped on from, and the part of `to` it lands in, when known. A warp
    /// inside one map (the floors of a tower) goes from one part to another.
    pub from_region: Option<u32>,
    pub to_region: Option<u32>,
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

    /// Whether the bot can step on it from the part `region` of its map (unknown parts are taken to connect).
    fn usable_from(&self, region: Option<u32>) -> bool {
        region.is_none() || self.from_region.is_none() || self.from_region == region
    }
}

/// Where a search stands: a map, and the part of it when known.
type Node = (String, Option<u32>);

/// Most places (map parts) a single search reaches, so a route to an unreachable map does not crawl the whole world.
const MAX_EXPLORED: usize = 2000;

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
                from_region: w.region,
                to_region: w.to.region,
            })
            .collect();
        // Some maps list no warps of their own (moc_fild20), only the warps that land on them. Warps come in pairs,
        // so the way back is next to where a warp lands; travel then takes the real warp from the observation.
        for a in &data.arrivals {
            let to = a.from.map.id_str();
            if to.is_empty() || to == map || edges.iter().any(|e| e.to == to) {
                continue;
            }
            edges.push(Edge {
                from: map.to_owned(),
                x: a.land.x,
                y: a.land.y,
                to,
                to_x: a.from.x,
                to_y: a.from.y,
                guessed: true,
                from_region: a.land.region,
                to_region: a.from.region,
            });
        }
        let bad = self.bad.read().unwrap();
        let no_guesses = bad.contains(&Edge::guessed_key(map));
        edges.retain(|e| !bad.contains(&e.key()) && !(e.guessed && no_guesses));
        Ok(edges)
    }

    /// Fewest-warps route from `from` to `to`, avoiding the maps of `avoid` (except `to` itself).
    pub async fn route(&self, from: &str, to: &str, avoid: &HashSet<String>) -> Result<Option<Vec<Edge>>> {
        self.route_skipping(from, None, to, avoid, &HashSet::new()).await
    }

    /// `route` from the part `region` of `from` (where the bot stands; `None` for any), also leaving out the warps of
    /// `skip` (keys of `Edge::key`). Parts of a map count: a warp in a room the bot cannot walk to is no way out, and
    /// the floors of a tower are reached through its own warps.
    pub async fn route_skipping(
        &self,
        from: &str,
        region: Option<u32>,
        to: &str,
        avoid: &HashSet<String>,
        skip: &HashSet<String>,
    ) -> Result<Option<Vec<Edge>>> {
        self.route_into(from, region, to, None, avoid, skip).await
    }

    /// `route_skipping` into the part `to_region` of `to` (any part for `None`): a room of an interior map the bot
    /// gets to only through another door of the town outside.
    pub async fn route_into(
        &self,
        from: &str,
        region: Option<u32>,
        to: &str,
        to_region: Option<u32>,
        avoid: &HashSet<String>,
        skip: &HashSet<String>,
    ) -> Result<Option<Vec<Edge>>> {
        let arrived = |map: &str, part: Option<u32>| map == to && (to_region.is_none() || part.is_none() || part == to_region);
        if arrived(from, region) {
            return Ok(Some(Vec::new()));
        }
        let start: Node = (from.to_owned(), region);
        let mut prev: HashMap<Node, (Node, Edge)> = HashMap::new();
        let mut seen: HashSet<Node> = HashSet::from([start.clone()]);
        let mut queue = VecDeque::from([start.clone()]);
        while let Some(node) = queue.pop_front() {
            if seen.len() > MAX_EXPLORED {
                break;
            }
            let Ok(edges) = self.edges(&node.0).await else { continue };
            for edge in edges {
                let next: Node = (edge.to.clone(), edge.to_region);
                let inner = edge.to == edge.from;
                if !edge.usable_from(node.1)
                    || seen.contains(&next)
                    || (inner && edge.to_region.is_none())
                    || (avoid.contains(&edge.to) && edge.to != to && edge.to != from && !inner)
                    || skip.contains(&edge.key())
                {
                    continue;
                }
                seen.insert(next.clone());
                prev.insert(next.clone(), (node.clone(), edge.clone()));
                if arrived(&edge.to, edge.to_region) {
                    return Ok(Some(unwind(&prev, &start, next)));
                }
                queue.push_back(next);
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
        let start: Node = (from.to_owned(), None);
        let mut dist: HashMap<Node, u32> = HashMap::from([(start.clone(), 0)]);
        let mut queue = VecDeque::from([start]);
        while let Some(node) = queue.pop_front() {
            let d = dist[&node];
            if d >= max_hops || dist.len() > MAX_EXPLORED {
                continue;
            }
            let Ok(edges) = self.edges(&node.0).await else { continue };
            for edge in edges.into_iter().filter(|e| e.usable_from(node.1) && (e.to != e.from || e.to_region.is_some())) {
                let next: Node = (edge.to, edge.to_region);
                if !dist.contains_key(&next) {
                    dist.insert(next.clone(), d + 1);
                    queue.push_back(next);
                }
            }
        }
        let mut best: HashMap<String, u32> = HashMap::new();
        for ((map, _), d) in dist {
            let e = best.entry(map).or_insert(d);
            *e = (*e).min(d);
        }
        let mut out: Vec<_> = best.into_iter().collect();
        out.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        Ok(out)
    }
}

fn unwind(prev: &HashMap<Node, (Node, Edge)>, from: &Node, to: Node) -> Vec<Edge> {
    let mut path = Vec::new();
    let mut cur = to;
    while &cur != from {
        let (back, edge) = prev[&cur].clone();
        path.push(edge);
        cur = back;
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn warp(x: u16, region: u32, to: &str, to_region: u32) -> serde_json::Value {
        json!({"x": x, "y": 1, "region": region, "to": {"map": {"id": to, "name": to}, "x": 1, "y": 1, "region": to_region}})
    }

    /// A world in a cache directory: the town leads into a tower on its part 9, whose stairs down to the dungeon are
    /// on part 4 (9 -> 10 -> 8 -> 4), which also has a way out to the town; a room of the castle, closed off from where the bot lands, has a warp to the town.
    fn world() -> WorldGraph {
        let dir = std::env::temp_dir().join(format!("rro-graph-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let maps = [
            ("town", vec![warp(1, 2, "tower", 9), warp(2, 2, "castle", 1)]),
            ("tower", vec![warp(3, 9, "town", 2), warp(4, 9, "tower", 10), warp(5, 10, "tower", 8), warp(6, 8, "tower", 4), warp(7, 4, "dungeon", 3), warp(8, 9, "tower", 6), warp(12, 4, "town", 2)]),
            ("dungeon", vec![warp(9, 3, "tower", 4)]),
            ("castle", vec![warp(10, 1, "town", 2), warp(11, 5, "far", 1)]),
            ("far", vec![]),
        ];
        for (name, warps) in maps {
            let data = json!({"name": name, "warps": warps, "arrivals": []});
            std::fs::write(dir.join(format!("db_maps_{name}.json")), data.to_string()).unwrap();
        }
        WorldGraph::new(Db::new("http://127.0.0.1:9", Some(dir)).unwrap())
    }

    #[tokio::test]
    async fn the_floors_of_a_tower_are_walked_through_its_own_warps() {
        let g = world();
        let route = g.route("town", "dungeon", &HashSet::new()).await.unwrap().unwrap();
        let steps: Vec<(String, u16)> = route.iter().map(|e| (e.from.clone(), e.x)).collect();
        let want = [("town", 1), ("tower", 4), ("tower", 5), ("tower", 6), ("tower", 7)].map(|(m, x)| (m.to_string(), x));
        assert_eq!(steps, want);
        // Standing on part 8 already
        let route = g.route_skipping("tower", Some(8), "dungeon", &HashSet::new(), &HashSet::new()).await.unwrap().unwrap();
        assert_eq!(route.iter().map(|e| e.x).collect::<Vec<_>>(), vec![6, 7]);
    }

    #[tokio::test]
    async fn a_room_with_no_door_inside_is_reached_from_outside() {
        // Out of the tower's part 9 to the town, and back in: no warp inside the tower leads from 6 to 9
        let g = world();
        let route = g.route_into("tower", Some(6), "tower", Some(9), &HashSet::new(), &HashSet::new()).await.unwrap();
        assert!(route.is_none(), "part 6 has no warps in this world");
        let route = g.route_into("dungeon", Some(3), "tower", Some(9), &HashSet::new(), &HashSet::new()).await.unwrap().unwrap();
        assert_eq!(route.iter().map(|e| (e.to.as_str(), e.to_region)).collect::<Vec<_>>(), vec![("tower", Some(4)), ("town", Some(2)), ("tower", Some(9))]);
    }

    #[tokio::test]
    async fn a_warp_in_a_closed_room_is_no_way_out() {
        let g = world();
        assert!(g.route("town", "far", &HashSet::new()).await.unwrap().is_none());
        let near: HashMap<String, u32> = g.neighbourhood("town", 5).await.unwrap().into_iter().collect();
        assert_eq!(near.get("dungeon"), Some(&5));
        assert_eq!(near.get("far"), None);
    }
}
