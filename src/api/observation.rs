//! Typed view of `GET /bots/{name}/observation`. Every field defaults, so a missing or newly added field
//! never breaks the bot. Parts the bot does not act on (storage, companions…) stay raw JSON.

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
    /// The last page of the conversation the NPC closed, shown with a Close button and nothing to answer.
    pub closed_dialog: Option<ClosedDialog>,
    pub messages: Vec<Message>,
    pub storage: Option<Value>,
    pub party: Option<Party>,
    pub party_invitation: Option<PartyInvitation>,
    /// The party booking ad the bot put up.
    pub booking: Option<BookingAd>,
    /// What the last booking search found.
    pub booking_results: Option<BookingResults>,
    pub trade: Option<Trade>,
    pub skill_menu: Option<SkillMenu>,
    pub duel_invitation: bool,
    /// The quest window: quests started and not completed.
    pub quests: Vec<QuestEntry>,
    pub chat_room: Option<ChatRoom>,
}

/// A skill waiting for a choice (Arrow Crafting, Teleport...): answer with `skill_choose`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SkillMenu {
    pub skill: String,
    pub kind: String,
    pub choices: Vec<SkillChoice>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SkillChoice {
    /// A map name or a number, depending on the skill.
    pub choice: Value,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct QuestEntry {
    pub id: u32,
    pub title: String,
    pub active: bool,
    pub expires_in: Option<u32>,
    pub objectives: Vec<QuestObjective>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct QuestObjective {
    pub mob_id: u32,
    pub name: String,
    pub done: u32,
    pub total: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Me {
    pub name: String,
    pub char_id: u64,
    pub job: String,
    /// `male` or `female`.
    pub sex: String,
    pub base_level: u32,
    pub job_level: u32,
    pub hp: u32,
    pub max_hp: u32,
    pub sp: u32,
    pub max_sp: u32,
    pub zeny: u64,
    pub weight: u32,
    pub max_weight: u32,
    pub status_points: u32,
    pub skill_points: u32,
    pub stats: Stats,
    pub x: u16,
    pub y: u16,
    pub action: String,
    pub moving: bool,
    pub attack_target: Option<u64>,
    pub dead: bool,
    /// The walkable part of the map the bot is in (a room of an interior map).
    pub region: Option<u32>,
}

impl Me {
    pub fn hp_frac(&self) -> f32 {
        if self.max_hp == 0 {
            1.0
        } else {
            self.hp as f32 / self.max_hp as f32
        }
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
    pub job: String,
    pub dead: bool,
    /// What the player is hitting, as its sprite shows.
    pub attack_target: Option<u64>,
    /// A member of the bot's party.
    pub party: bool,
    pub base_level: u32,
    pub job_level: u32,
    pub sex: String,
    pub action: String,
    /// The party and guild it is in, `None` for none.
    pub party_name: Option<String>,
    pub guild_name: Option<String>,
    /// HP bar, for the members of the bot's party only.
    pub hp: Option<u32>,
    pub max_hp: Option<u32>,
    /// The title of the chat room it opened.
    pub chat_room: Option<String>,
}

/// An ad of the party booking board: a party looking for members.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BookingAd {
    pub index: u64,
    /// Who put it up: whisper them to join.
    pub name: String,
    pub level: i32,
    pub map_id: i32,
    /// The jobs it looks for, empty for anyone.
    pub jobs: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BookingResults {
    pub ads: Vec<BookingAd>,
    pub more: bool,
}

/// The party window of the game client.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Party {
    pub id: u64,
    pub name: String,
    /// The leader's name.
    pub leader: Option<String>,
    pub exp_share: bool,
    pub exp_share_denied: bool,
    pub item_pickup: bool,
    pub item_share: bool,
    pub members: Vec<PartyMember>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PartyMember {
    pub name: String,
    pub char_id: Option<u64>,
    pub map: String,
    pub leader: bool,
    pub online: bool,
    /// While the member is in the game.
    pub job: Option<String>,
    pub base_level: Option<u32>,
    /// On the bot's map only.
    pub x: Option<u16>,
    pub y: Option<u16>,
    /// While the member is in sight.
    pub hp: Option<u32>,
    pub max_hp: Option<u32>,
}

impl PartyMember {
    pub fn pos(&self) -> Option<(u16, u16)> {
        self.x.zip(self.y)
    }

    pub fn hp_frac(&self) -> Option<f32> {
        match (self.hp, self.max_hp) {
            (Some(hp), Some(max)) if max > 0 => Some(hp as f32 / max as f32),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PartyInvitation {
    pub party_id: u64,
    pub inviter: u64,
    pub inviter_name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Trade {
    pub partner: u64,
    pub partner_name: String,
    /// `requested`, `accepted`, `locked` or `confirmed`.
    pub phase: String,
    pub partner_phase: String,
    pub incoming_request: bool,
    pub my_items: Vec<TradeItem>,
    pub partner_items: Vec<TradeItem>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TradeItem {
    pub item_id: u32,
    pub name: String,
    pub amount: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Npc {
    pub id: u64,
    pub name: String,
    pub x: u16,
    pub y: u16,
    /// A quest icon shows above it: it has a quest, or the next step of one, for the bot.
    pub quest_icon: bool,
    /// Its waiting room, where players queue for a test.
    pub waiting_room: Option<NpcRoom>,
    pub region: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct NpcRoom {
    pub title: String,
    pub waiting: u32,
    pub limit: u32,
}

/// The chat room (or NPC waiting room) the bot is in.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ChatRoom {
    pub id: u64,
    pub title: String,
    pub members: Vec<String>,
    /// The NPC of a waiting room.
    pub npc: Option<u64>,
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
    /// Who the monster is after.
    pub target: Option<u64>,
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
    pub region: Option<u32>,
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
    /// Weight of one, in the units of `self.weight`.
    pub weight: u32,
    pub sell_price: u32,
    /// For equipment: where it goes and whether the bot can wear it.
    pub equip: Option<Equip>,
}

impl InventoryItem {
    /// Worn, and of use: a piece the job cannot wear any more (a Novice's Axe on an Acolyte) stays on after the job
    /// change and adds nothing.
    pub fn worn_and_usable(&self) -> bool {
        self.equipped && self.equip.as_ref().is_none_or(|e| e.why_not.is_none())
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Equip {
    /// `HeadTop`, `HeadMid`, `HeadLow`, `Armor`, `HandRight` (weapon), `HandLeft` (shield), `Garment`, `Shoes`,
    /// `AccessoryLeft`, `AccessoryRight`, `Ammo`.
    pub slots: Vec<String>,
    pub attack: Option<i32>,
    pub defense: Option<i32>,
    pub weapon_level: Option<i32>,
    pub equip_level: Option<i32>,
    pub refine: i32,
    pub identified: bool,
    pub can_equip: bool,
    pub why_not: Option<String>,
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
pub struct ClosedDialog {
    pub npc_id: u64,
    pub lines: Vec<String>,
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
        self.inventory
            .iter()
            .filter(|i| i.item_id == item_id)
            .map(|i| i.amount)
            .sum()
    }

    pub fn skill_level(&self, name: &str) -> u32 {
        self.skills
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.level)
            .unwrap_or(0)
    }

    pub fn last_message_seq(&self) -> u64 {
        self.messages.iter().map(|m| m.seq).max().unwrap_or(0)
    }

    /// A player standing within `radius` cells, by name.
    pub fn player_near(&self, name: &str, radius: u32) -> Option<&Player> {
        self.players
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name) && dist((p.x, p.y), self.pos()) <= radius)
    }

    /// Whether `cell` is on a warp of this map or within `margin` cells of one. Walking there, or through there,
    /// takes the bot to another map.
    pub fn near_warp(&self, cell: (u16, u16), margin: u16) -> bool {
        self.warps.iter().any(|w| {
            cell.0.abs_diff(w.x) <= w.half_width + margin
                && cell.1.abs_diff(w.y) <= w.half_height + margin
        })
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
      "players": [{"id": 150002, "name": "Other", "x": 150, "y": 190, "attackable": false, "job": "Acolyte", "dead": false,
                   "attack_target": null, "party": true}],
      "party": {"id": 1, "name": "Squad", "leader": "Scout", "exp_share": true, "exp_share_denied": false, "item_pickup": true,
                "item_share": false,
                "members": [{"name": "Scout", "char_id": 150001, "map": "prontera", "leader": true, "online": true,
                             "x": null, "y": null, "hp": null, "max_hp": null},
                            {"name": "Other", "char_id": 150002, "map": "prontera", "leader": false, "online": true,
                             "x": 150, "y": 190, "hp": 80, "max_hp": 120}]},
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
        assert!(
            obs.near_warp((158, 21), 0)
                && obs.near_warp((160, 23), 2)
                && !obs.near_warp((160, 23), 1)
        );
        assert_eq!(obs.skills[0].range, Some(9));
        let party = obs.party.as_ref().unwrap();
        assert_eq!(party.leader.as_deref(), Some("Scout"));
        assert_eq!(party.members[1].pos(), Some((150, 190)));
        assert_eq!(party.members[1].hp_frac(), Some(80.0 / 120.0));
        assert!(obs.players[0].party);
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
