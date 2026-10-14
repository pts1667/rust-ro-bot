//! Client for the server's read-only reference (`/db` and `/wiki`, port 6904). The data is static while the
//! server runs, so answers are cached in memory and on disk (`cache/db/`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Mob `mode` bits (rathena).
pub const MD_AGGRESSIVE: u32 = 0x4;
pub const MD_ANGRY: u32 = 0x800;

#[derive(Clone)]
pub struct Db {
    http: reqwest::Client,
    base: String,
    cache_dir: Option<PathBuf>,
    memory: Arc<Mutex<HashMap<String, Value>>>,
}

/// A `{id, name, href}` link of the reference. `id` is a number or a string depending on the category.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Link {
    pub id: Value,
    pub name: String,
    pub href: String,
}

impl Link {
    pub fn id_str(&self) -> String {
        match &self.id {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }

    pub fn id_u32(&self) -> Option<u32> {
        self.id.as_u64().map(|v| v as u32)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MapData {
    pub name: String,
    pub flags: Vec<String>,
    pub warps: Vec<MapWarp>,
    pub mob_spawns: Vec<Spawn>,
    pub npcs: Vec<MapNpc>,
    pub connected_maps: Vec<Link>,
    /// Warps of other maps that land here.
    pub arrivals: Vec<Arrival>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Arrival {
    pub from: WarpEnd,
    pub land: WarpEnd,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct WarpEnd {
    pub map: Link,
    pub x: u16,
    pub y: u16,
    /// The walkable part of the map the cell is in.
    pub region: Option<u32>,
}

impl MapData {
    pub fn is_town(&self) -> bool {
        self.flags.iter().any(|f| f == "town")
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MapWarp {
    pub name: String,
    pub x: u16,
    pub y: u16,
    pub x_size: u16,
    pub y_size: u16,
    pub to: WarpDest,
    /// The walkable part of the map the warp is stepped on from.
    pub region: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct WarpDest {
    pub map: Link,
    pub x: u16,
    pub y: u16,
    /// The walkable part of the map the warp lands in.
    pub region: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Spawn {
    pub amount: u32,
    pub mob: Link,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MapNpc {
    pub npc: Link,
    pub shop: bool,
    pub x: u16,
    pub y: u16,
    /// The walkable part of the map it is talked to from.
    pub region: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MobData {
    pub id: u32,
    pub name_english: String,
    pub level: u32,
    pub hp: u32,
    pub exp: u32,
    pub job_exp: u32,
    pub mode: u32,
    pub mvp: bool,
    pub monster_class: String,
    pub atk1: u32,
    pub atk2: u32,
    pub def: u32,
    pub atk_delay: u32,
    pub agi: u32,
    pub dex: u32,
    pub element: String,
    pub race: String,
    pub drops: Vec<Drop>,
    /// Where it spawns.
    pub spawns: Vec<MobSpawn>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MobSpawn {
    pub amount: u32,
    pub map: Link,
    /// `normal`, or `boss` for the spawns of an MVP.
    #[serde(rename = "type")]
    pub kind: String,
}

impl MobData {
    pub fn aggressive(&self) -> bool {
        self.mode & (MD_AGGRESSIVE | MD_ANGRY) != 0
    }

    pub fn boss(&self) -> bool {
        self.mvp || self.monster_class.eq_ignore_ascii_case("boss")
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Drop {
    pub item: Link,
    pub rate: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ItemData {
    pub id: u32,
    pub name_english: String,
    pub item_type: String,
    pub price_buy: u64,
    pub weight: u32,
    pub attack: u32,
    pub defense: u32,
    pub equip_level_min: u32,
    pub location: u32,
    pub weapon_type: String,
    pub weapon_level: u32,
    /// Jobs that can wear it, see `gear::job_flag`.
    pub job_flags: u64,
    /// `Male` or `Female` for the items of one sex.
    pub gender: Option<String>,
    pub slots: u32,
    pub sold_by: Vec<ShopPlace>,
    pub dropped_by: Vec<DropSource>,
    pub arrow_crafting: ArrowCrafting,
    /// The item script, e.g. `itemheal rand(45,65),0;` for a Red Potion.
    pub script: String,
}

impl ItemData {
    /// Average HP the item heals, from its `itemheal` script. The server adds a VIT bonus on top.
    pub fn hp_heal(&self) -> Option<f32> {
        let rest = &self.script[self.script.find("itemheal")? + "itemheal".len()..];
        let arg = rest.trim_start().split(';').next()?;
        let hp = match arg.trim_start().strip_prefix("rand(") {
            Some(range) => {
                let (lo, hi) = range.split_once(')')?.0.split_once(',')?;
                (lo.trim().parse::<f32>().ok()? + hi.trim().parse::<f32>().ok()?) / 2.0
            }
            None => arg.split(',').next()?.trim().parse().ok()?,
        };
        (hp > 0.0).then_some(hp)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DropSource {
    pub mob: Link,
    /// Chance in 1/10000.
    pub rate: u32,
    pub mvp: bool,
}

/// What Arrow Crafting makes of an item.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ArrowCrafting {
    pub as_source: Vec<ArrowRecipe>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ArrowRecipe {
    pub makes: Vec<ArrowProduct>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ArrowProduct {
    pub item: Link,
    pub amount: u32,
}

/// An NPC of the reference, with what it sells when it is a shop.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct NpcData {
    pub name: String,
    pub map: Link,
    pub x: u16,
    pub y: u16,
    pub shop: Option<Vec<ShopOffer>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ShopOffer {
    pub item: Link,
    pub price: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ShopPlace {
    pub map: Link,
    pub npc: Link,
    pub price: u64,
    pub x: u16,
    pub y: u16,
}

impl ShopPlace {
    /// `map:x,y`, as `Memory::bad_shops` holds it.
    pub fn key(&self) -> String {
        format!("{}:{},{}", self.map.id_str(), self.x, self.y)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SearchResults {
    pub results: Vec<Link>,
    pub total: u32,
}

impl Db {
    pub fn new(base: &str, cache_dir: Option<PathBuf>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()?;
        if let Some(dir) = &cache_dir {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            cache_dir,
            memory: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// `GET <path>` as JSON, from the cache when possible. `path` starts with `/db/`.
    pub async fn get(&self, path: &str) -> Result<Value> {
        if let Some(v) = self.memory.lock().unwrap().get(path) {
            return Ok(v.clone());
        }
        let file = self
            .cache_dir
            .as_ref()
            .map(|d| d.join(cache_file_name(path)));
        if let Some(file) = &file
            && let Ok(text) = std::fs::read_to_string(file)
            && let Ok(v) = serde_json::from_str::<Value>(&text)
        {
            self.memory
                .lock()
                .unwrap()
                .insert(path.to_owned(), v.clone());
            return Ok(v);
        }
        let resp = self
            .http
            .get(format!("{}{}", self.base, path))
            .send()
            .await
            .with_context(|| format!("GET {path}"))?;
        let status = resp.status();
        let v: Value = resp
            .json()
            .await
            .with_context(|| format!("parsing {path}"))?;
        if !status.is_success() {
            bail!(
                "{path}: {status}: {}",
                v["error"].as_str().unwrap_or("error")
            );
        }
        if let Some(file) = &file {
            let _ = std::fs::write(file, v.to_string());
        }
        self.memory
            .lock()
            .unwrap()
            .insert(path.to_owned(), v.clone());
        Ok(v)
    }

    pub async fn get_as<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let v = self.get(path).await?;
        serde_json::from_value(v).with_context(|| format!("decoding {path}"))
    }

    pub async fn map(&self, name: &str) -> Result<MapData> {
        self.get_as(&format!("/db/maps/{}", urlencoding::encode(name)))
            .await
    }

    pub async fn mob(&self, id: u32) -> Result<MobData> {
        self.get_as(&format!("/db/mobs/{id}")).await
    }

    pub async fn item(&self, id: u32) -> Result<ItemData> {
        self.get_as(&format!("/db/items/{id}")).await
    }

    /// An NPC by its `map/name` id (the name with its `#suffix`).
    pub async fn npc(&self, id: &str) -> Result<NpcData> {
        let path: Vec<String> = id
            .split('/')
            .map(|p| urlencoding::encode(p).into_owned())
            .collect();
        self.get_as(&format!("/db/npcs/{}", path.join("/"))).await
    }

    /// Searches a category by name, `?q=`.
    pub async fn search(&self, category: &str, query: &str, limit: u32) -> Result<Vec<Link>> {
        let path = format!(
            "/db/{category}?q={}&limit={limit}",
            urlencoding::encode(query)
        );
        Ok(self.get_as::<SearchResults>(&path).await?.results)
    }

    /// A wiki page as Markdown, not cached on disk.
    pub async fn wiki(&self, page: &str) -> Result<String> {
        let page = page.trim_start_matches("/wiki/");
        let url = format!(
            "{}/wiki/{}",
            self.base,
            page.split('/')
                .map(|p| urlencoding::encode(p).into_owned())
                .collect::<Vec<_>>()
                .join("/")
        );
        let resp = self.http.get(url).send().await?;
        if !resp.status().is_success() {
            bail!("wiki page {page}: {}", resp.status());
        }
        Ok(resp.text().await?)
    }
}

fn cache_file_name(path: &str) -> String {
    let mut name: String = path
        .trim_start_matches('/')
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    name.push_str(".json");
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heal_amounts_come_from_the_script() {
        let item = |script: &str| ItemData {
            script: script.into(),
            ..Default::default()
        };
        assert_eq!(item("itemheal rand(45,65),0;").hp_heal(), Some(55.0));
        assert_eq!(item("itemheal 70,0;").hp_heal(), Some(70.0));
        assert_eq!(item("itemheal 0,rand(40,60);").hp_heal(), None);
        assert_eq!(item("bonus bStr,1;").hp_heal(), None);
    }

    #[test]
    fn cache_names_are_flat() {
        assert_eq!(
            cache_file_name("/db/maps/prt_fild08"),
            "db_maps_prt_fild08.json"
        );
        assert_eq!(
            cache_file_name("/db/items?q=red potion"),
            "db_items_q_red_potion.json"
        );
    }

    #[test]
    fn mob_flags() {
        let mob = MobData {
            mode: 0x84,
            ..Default::default()
        };
        assert!(mob.aggressive());
        let mob = MobData {
            mode: 0x83,
            ..Default::default()
        };
        assert!(!mob.aggressive());
    }
}
