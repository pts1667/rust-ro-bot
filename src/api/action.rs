//! The actions of `POST /bots/{name}/actions` that the bot uses, as a serde enum tagged by `type`.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct BuyLine {
    pub item_id: u32,
    pub amount: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SellLine {
    pub index: u32,
    pub amount: u32,
}

/// An item of the cart put up for sale in a vending store.
#[derive(Debug, Clone, Serialize)]
pub struct VendLine {
    pub cart_index: u32,
    pub amount: u32,
    pub price: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Move {
        x: u16,
        y: u16,
        wait: bool,
    },
    Use {
        target: u64,
    },
    Attack {
        target: u64,
        wait: bool,
    },
    Stop,
    /// Sitting doubles natural HP and SP regeneration.
    Sit,
    Stand,
    Respawn,
    DialogNext,
    DialogChoose {
        option: u32,
    },
    DialogNumber {
        value: i64,
    },
    DialogText {
        text: String,
    },
    DialogClose,
    DialogShop {
        deal: &'static str,
    },
    DialogBuy {
        items: Vec<BuyLine>,
    },
    DialogSell {
        items: Vec<SellLine>,
    },
    UseItem {
        index: u32,
    },
    Equip {
        index: u32,
    },
    Unequip {
        index: u32,
    },
    /// Answers the observation's `skill_menu`.
    SkillChoose {
        choice: serde_json::Value,
    },
    SkillCancel,
    RaiseStat {
        stat: &'static str,
        amount: u32,
    },
    LearnSkill {
        skill: String,
    },
    Skill {
        skill: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },
    Say {
        text: String,
    },
    Whisper {
        to: String,
        text: String,
    },
    Command {
        text: String,
    },
    PartyChat {
        text: String,
    },
    PartyCreate {
        name: String,
        item_pickup: bool,
        item_share: bool,
    },
    PartyInvite {
        name: String,
    },
    PartyAccept,
    PartyDecline,
    PartyLeave,
    /// Puts up a party booking ad with the bot's level, open to any job.
    BookingRegister,
    /// Searches the booking board, all levels.
    BookingSearch,
    BookingDelete,
    PartyOptions {
        #[serde(skip_serializing_if = "Option::is_none")]
        exp_share: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        item_pickup: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        item_share: Option<bool>,
    },
    TradeRequest {
        target: u64,
    },
    TradeAccept,
    TradeDecline,
    TradeOffer {
        index: u32,
        amount: u32,
    },
    TradeLock,
    TradeConfirm,
    TradeCancel,
    /// Joins the waiting room of an NPC (`room_join` with `npc`).
    #[serde(rename = "room_join")]
    RoomJoinNpc {
        npc: u64,
    },
    RoomLeave,
    /// Opens a chat room whose title shows above the bot.
    RoomCreate {
        title: String,
        limit: u32,
        public: bool,
    },
    /// An emote over the bot, by its client command name (`hmm`, `heh`, `ok`...).
    Emote {
        emotion: String,
    },
    /// Inventory to cart.
    CartPut {
        index: u32,
        amount: u32,
    },
    VendingOpen {
        title: String,
        items: Vec<VendLine>,
    },
    VendingClose,
    /// Inventory to the storage a Kafra opened.
    StorageDeposit {
        index: u32,
        amount: u32,
    },
    StorageClose,
    /// Runs an action a plugin offers to bots (`plugins.<name>.actions`); the answer carries the plugin's `reply`.
    Plugin {
        plugin: String,
        name: String,
        args: serde_json::Value,
    },
    /// Clicks a button of a window a plugin opened for the bot.
    PluginUi {
        plugin: String,
        window: String,
        widget: String,
        kind: &'static str,
    },
}

impl Action {
    /// Short name for logs.
    pub fn kind(&self) -> &'static str {
        match self {
            Action::Move { .. } => "move",
            Action::Use { .. } => "use",
            Action::Attack { .. } => "attack",
            Action::Stop => "stop",
            Action::Sit => "sit",
            Action::Stand => "stand",
            Action::Respawn => "respawn",
            Action::DialogNext => "dialog_next",
            Action::DialogChoose { .. } => "dialog_choose",
            Action::DialogNumber { .. } => "dialog_number",
            Action::DialogText { .. } => "dialog_text",
            Action::DialogClose => "dialog_close",
            Action::DialogShop { .. } => "dialog_shop",
            Action::DialogBuy { .. } => "dialog_buy",
            Action::DialogSell { .. } => "dialog_sell",
            Action::UseItem { .. } => "use_item",
            Action::Equip { .. } => "equip",
            Action::Unequip { .. } => "unequip",
            Action::SkillChoose { .. } => "skill_choose",
            Action::SkillCancel => "skill_cancel",
            Action::RaiseStat { .. } => "raise_stat",
            Action::LearnSkill { .. } => "learn_skill",
            Action::Skill { .. } => "skill",
            Action::Say { .. } => "say",
            Action::Whisper { .. } => "whisper",
            Action::Command { .. } => "command",
            Action::PartyChat { .. } => "party_chat",
            Action::PartyCreate { .. } => "party_create",
            Action::PartyInvite { .. } => "party_invite",
            Action::PartyAccept => "party_accept",
            Action::PartyDecline => "party_decline",
            Action::PartyLeave => "party_leave",
            Action::BookingRegister => "booking_register",
            Action::BookingSearch => "booking_search",
            Action::BookingDelete => "booking_delete",
            Action::PartyOptions { .. } => "party_options",
            Action::TradeRequest { .. } => "trade_request",
            Action::TradeAccept => "trade_accept",
            Action::TradeDecline => "trade_decline",
            Action::TradeOffer { .. } => "trade_offer",
            Action::TradeLock => "trade_lock",
            Action::TradeConfirm => "trade_confirm",
            Action::TradeCancel => "trade_cancel",
            Action::RoomJoinNpc { .. } => "room_join",
            Action::RoomLeave => "room_leave",
            Action::RoomCreate { .. } => "room_create",
            Action::Emote { .. } => "emote",
            Action::CartPut { .. } => "cart_put",
            Action::VendingOpen { .. } => "vending_open",
            Action::VendingClose => "vending_close",
            Action::StorageDeposit { .. } => "storage_deposit",
            Action::StorageClose => "storage_close",
            Action::Plugin { .. } => "plugin",
            Action::PluginUi { .. } => "plugin_ui",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_with_type_tag() {
        let json = serde_json::to_value(Action::Attack {
            target: 7,
            wait: false,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"type": "attack", "target": 7, "wait": false})
        );
        let json = serde_json::to_value(Action::DialogClose).unwrap();
        assert_eq!(json, serde_json::json!({"type": "dialog_close"}));
        let json = serde_json::to_value(Action::PartyOptions {
            exp_share: Some(true),
            item_pickup: None,
            item_share: None,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"type": "party_options", "exp_share": true})
        );
        assert_eq!(
            serde_json::to_value(Action::BookingSearch).unwrap(),
            serde_json::json!({"type": "booking_search"})
        );
        assert_eq!(
            serde_json::to_value(Action::RoomJoinNpc { npc: 9 }).unwrap(),
            serde_json::json!({"type": "room_join", "npc": 9})
        );
    }
}
