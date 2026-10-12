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
    Command {
        text: String,
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
            Action::RaiseStat { .. } => "raise_stat",
            Action::LearnSkill { .. } => "learn_skill",
            Action::Skill { .. } => "skill",
            Action::Say { .. } => "say",
            Action::Command { .. } => "command",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_with_type_tag() {
        let json = serde_json::to_value(Action::Attack { target: 7, wait: false }).unwrap();
        assert_eq!(json, serde_json::json!({"type": "attack", "target": 7, "wait": false}));
        let json = serde_json::to_value(Action::DialogClose).unwrap();
        assert_eq!(json, serde_json::json!({"type": "dialog_close"}));
    }
}
