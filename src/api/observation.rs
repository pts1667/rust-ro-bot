//! Typed view of `GET /bots/{name}/observation`. Every field defaults, so a missing or newly added field
//! never breaks the bot. Parts the bot does not act on (party, trade, companions…) stay raw JSON.

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Observation {
    pub ready: bool,
    #[serde(rename = "self")]
    pub me: Me,
    pub map: MapInfo,
    pub players: Vec<Player>,
    pub mobs: Vec<Mob>,
    pub npcs: Vec<Npc>,
    pub items: Vec<GroundItem>,
    pub warps: Vec<Warp>,
    pub inventory: Vec<InventoryItem>,
    pub skills: Vec<Skill>,
    pub dialog: Option<Dialog>,
    pub messages: Vec<Message>,
    pub storage: Option<Value>,
    pub party: Option<Value>,
    pub party_invitation: Option<Value>,
    pub trade: Option<Value>,
    pub skill_menu: Option<Value>,
    pub duel_invitation: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Me {
    pub name: String,
    pub char_id: u64,
    pub job: String,
    pub base_level: u32,
    pub job_level: u32,
    pub hp: u32,
    pub max_hp: u32,
    pub sp: u32,
    pub max_sp: u32,
    pub zeny: u64,
    pub weight: u32,
    pub status_points: u32,
    pub skill_points: u32,
    pub stats: Stats,
    pub x: u16,
    pub y: u16,
    pub action: String,
    pub moving: bool,
    pub attack_target: Option<u64>,
    pub dead: bool,
}

impl Me {
    pub fn hp_frac(&self) -> f32 {
        if self.max_hp == 0 { 1.0 } else { self.hp as f32 / self.max_hp as f32 }
    }

    pub fn pos(&self) -> (u16, u16) {
        (self.x, self.y)
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Stats {
    pub str: u32,
    pub agi: u32,
    pub vit: u32,
    pub int: u32,
    pub dex: u32,
    pub luk: u32,
}

impl Stats {
    pub fn get(&self, stat: &str) -> u32 {
        match stat {
            "str" => self.str,
            "agi" => self.agi,
            "vit" => self.vit,
            "int" => self.int,
            "dex" => self.dex,
            "luk" => self.luk,
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MapInfo {
    pub name: String,
    pub instance: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Player {
    pub id: u64,
    pub name: String,
    pub x: u16,
    pub y: u16,
    pub attackable: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Npc {
    pub id: u64,
    pub name: String,
    pub x: u16,
    pub y: u16,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Mob {
    pub id: u64,
    pub name: String,
    pub mob_id: u32,
    pub x: u16,
    pub y: u16,
    pub hp: u32,
    pub max_hp: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct GroundItem {
    pub id: u64,
    pub item_id: u32,
    pub name: String,
    pub amount: u32,
    pub x: u16,
    pub y: u16,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Warp {
    pub id: u64,
    pub x: u16,
    pub y: u16,
    pub half_width: u16,
    pub half_height: u16,
    pub to_map: String,
    pub to_x: u16,
    pub to_y: u16,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct InventoryItem {
    pub index: u32,
    pub item_id: u32,
    pub name: String,
    pub kind: String,
    pub amount: u32,
    pub equipped: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Skill {
    pub id: u32,
    pub name: String,
    pub level: u32,
    pub aim: String,
    pub range: Option<i32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Dialog {
    pub npc_id: u64,
    pub lines: Vec<String>,
    pub prompt: Option<Prompt>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Prompt {
    pub kind: String,
    pub options: Vec<String>,
    pub items: Vec<ShopLine>,
}

/// A line of a shop's buy or sell list.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ShopLine {
    pub item_id: u32,
    pub index: Option<u32>,
    pub name: String,
    pub price: u64,
    pub amount: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Message {
    pub seq: u64,
    pub channel: String,
    pub text: String,
}

pub fn dist(a: (u16, u16), b: (u16, u16)) -> u32 {
    let dx = (a.0 as i32 - b.0 as i32).unsigned_abs();
    let dy = (a.1 as i32 - b.1 as i32).unsigned_abs();
    dx.max(dy)
}

impl Observation {
    pub fn pos(&self) -> (u16, u16) {
        self.me.pos()
    }

    pub fn item_count(&self, item_id: u32) -> u32 {
        self.inventory.iter().filter(|i| i.item_id == item_id).map(|i| i.amount).sum()
    }

    pub fn skill_level(&self, name: &str) -> u32 {
        self.skills.iter().find(|s| s.name == name).map(|s| s.level).unwrap_or(0)
    }

    pub fn last_message_seq(&self) -> u64 {
        self.messages.iter().map(|m| m.seq).max().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example observation of SKILL.md, trimmed, plus a field the bot does not know.
    const EXAMPLE: &str = r#"{
      "ready": true,
      "self": {"name": "Scout", "char_id": 150001, "job": "Novice", "base_level": 1, "job_level": 1, "hp": 40, "max_hp": 40,
               "sp": 11, "max_sp": 11, "zeny": 0, "weight": 0, "status_points": 48, "skill_points": 0,
               "stats": {"str": 1, "agi": 1, "vit": 1, "int": 1, "dex": 1, "luk": 1}, "x": 156, "y": 191,
               "action": "idle", "moving": false, "attack_target": null, "dead": false},
      "map": {"name": "prontera", "instance": 0, "width": 312, "height": 392},
      "players": [{"id": 150002, "name": "Other", "x": 150, "y": 190, "attackable": false}],
      "mobs":    [{"id": 400012, "name": "Poring", "mob_id": 1002, "x": 160, "y": 200, "hp": 50, "max_hp": 50}],
      "npcs":    [{"id": 100123, "name": "Kafra Employee", "x": 151, "y": 29}],
      "items":   [{"id": 400301, "item_id": 909, "name": "Jellopy", "amount": 1, "x": 158, "y": 195}],
      "warps":   [{"id": 100500, "x": 156, "y": 20, "half_width": 2, "half_height": 1, "to_map": "prt_fild08", "to_x": 170, "to_y": 380}],
      "inventory": [{"index": 0, "item_id": 1201, "name": "Knife", "kind": "Weapon", "amount": 1, "equipped": true}],
      "skills": [{"id": 28, "name": "AL_HEAL", "level": 3, "aim": "target", "range": 9}],
      "dialog": {"npc_id": 100123, "lines": ["[Kafra Employee]", "Welcome!"], "prompt": {"kind": "menu", "options": ["Save", "Cancel"]}},
      "messages": [{"seq": 1, "channel": "system", "text": "..."}],
      "brand_new_field": {"x": 1}
    }"#;

    #[test]
    fn parses_skill_md_example() {
        let obs: Observation = serde_json::from_str(EXAMPLE).unwrap();
        assert!(obs.ready);
        assert_eq!(obs.me.status_points, 48);
        assert_eq!(obs.mobs[0].name, "Poring");
        assert_eq!(obs.warps[0].to_map, "prt_fild08");
        assert_eq!(obs.skills[0].range, Some(9));
        let prompt = obs.dialog.unwrap().prompt.unwrap();
        assert_eq!(prompt.kind, "menu");
        assert_eq!(prompt.options.len(), 2);
    }

    #[test]
    fn missing_fields_default() {
        let obs: Observation = serde_json::from_str(r#"{"self": {"hp": 3}}"#).unwrap();
        assert_eq!(obs.me.hp, 3);
        assert!(obs.mobs.is_empty());
        assert!(obs.dialog.is_none());
    }
}
