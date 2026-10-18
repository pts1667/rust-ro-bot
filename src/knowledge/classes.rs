//! The reborn classes of pre-renewal: a second job at base level 99 and job level 50 is reborn as a High Novice
//! (`npc/jobs/valkyrie.txt`), becomes the high first job of its line, then at job level 40 its transcendent job.
//! The server names them as its `JobName` does (`NoviceHigh`, `SwordsmanHigh`, `LordKnight`); the bot reads each as
//! the job it continues (`Novice`, `Swordsman`, `Knight`), with `reborn` set, so what is known of a job holds for its
//! reborn class too.

/// Each reborn class, as the server names it, and the job it continues.
const REBORN: [(&str, &str); 20] = [
    ("NoviceHigh", "Novice"),
    ("SwordsmanHigh", "Swordsman"),
    ("MageHigh", "Mage"),
    ("ArcherHigh", "Archer"),
    ("AcolyteHigh", "Acolyte"),
    ("MerchantHigh", "Merchant"),
    ("ThiefHigh", "Thief"),
    ("LordKnight", "Knight"),
    ("HighPriest", "Priest"),
    ("HighWizard", "Wizard"),
    ("Whitesmith", "Blacksmith"),
    ("Sniper", "Hunter"),
    ("AssassinCross", "Assassin"),
    ("Paladin", "Crusader"),
    ("Champion", "Monk"),
    ("Professor", "Sage"),
    ("Stalker", "Rogue"),
    ("Creator", "Alchemist"),
    ("Clown", "Bard"),
    ("Gypsy", "Dancer"),
];

/// The job a reborn class continues, `None` for a class that is not reborn.
pub fn base_class(class: &str) -> Option<&'static str> {
    let key: String = class
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    REBORN
        .iter()
        .find(|(reborn, _)| reborn.eq_ignore_ascii_case(&key))
        .map(|(_, base)| *base)
}

/// What a second job becomes past its rebirth: the name a player knows it by, and the NPC of Valhalla
/// (`npc/jobs/2-1a`, `2-2a`) that changes the high first job to it, with its place.
pub struct Transcendent {
    pub second: &'static str,
    pub name: &'static str,
    pub npc: &'static str,
    pub x: u16,
    pub y: u16,
}

pub const TRANSCENDENT: [Transcendent; 13] = [
    t("Knight", "Lord Knight", "Lord Knight", 44, 39),
    t("Priest", "High Priest", "High Priest", 44, 42),
    t("Wizard", "High Wizard", "High Wizard", 44, 47),
    t("Blacksmith", "Whitesmith", "MasterSmith", 44, 50),
    t("Hunter", "Sniper", "Sniper", 44, 55),
    t("Assassin", "Assassin Cross", "Assassin Cross", 44, 58),
    t("Crusader", "Paladin", "Paladin", 53, 39),
    t("Monk", "Champion", "Champion", 53, 42),
    t("Sage", "Professor", "Scholar", 53, 47),
    t("Alchemist", "Creator", "Biochemist", 53, 50),
    t("Bard", "Clown", "Minstrel", 53, 54),
    t("Dancer", "Gypsy", "Gypsy", 53, 56),
    t("Rogue", "Stalker", "Stalker", 53, 58),
];

const fn t(
    second: &'static str,
    name: &'static str,
    npc: &'static str,
    x: u16,
    y: u16,
) -> Transcendent {
    Transcendent {
        second,
        name,
        npc,
        x,
        y,
    }
}

/// The second jobs a character can be reborn from.
pub const SECOND_JOBS: [&str; 13] = [
    "Knight",
    "Priest",
    "Wizard",
    "Blacksmith",
    "Hunter",
    "Assassin",
    "Crusader",
    "Monk",
    "Sage",
    "Alchemist",
    "Bard",
    "Dancer",
    "Rogue",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reborn_class_reads_as_the_job_it_continues() {
        assert_eq!(base_class("NoviceHigh"), Some("Novice"));
        assert_eq!(base_class("Swordsman High"), Some("Swordsman"));
        assert_eq!(base_class("LordKnight"), Some("Knight"));
        assert_eq!(base_class("Knight"), None);
        assert_eq!(base_class("Novice"), None);
        assert!(
            SECOND_JOBS
                .iter()
                .all(|j| TRANSCENDENT.iter().any(|t| t.second == *j))
        );
    }
}
