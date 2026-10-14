//! Skill quests: the skills a guild teaches for items and errands rather than skill points. Taken from rathena's
//! `npc/quests/skills/*.txt` (pre-renewal where it differs), the scripts this server runs.

use super::build::*;
use super::{Check, Items, Quest, Reward};

pub fn quests() -> Vec<Quest> {
    let mut v = vec![first_aid(), play_dead()];
    v.extend(first_jobs());
    v.extend(super::skills2::quests());
    v
}

/// A quest that asks for items, then teaches the skill when the bot talks to the NPC with them.
#[allow(clippy::too_many_arguments)]
pub(super) fn bring(
    skill: &'static str,
    title: &'static str,
    jobs: &'static [&'static str],
    job_level: u32,
    items: Items,
    (map, npc, x, y): (&'static str, &'static str, u16, u16),
    menu: &'static [&'static str],
) -> Quest {
    Quest {
        id: skill_id(skill),
        title,
        reward: Reward::Skill(skill),
        jobs,
        job_level,
        base_level: 1,
        sex: None,
        skills: &[],
        stages: vec![
            gather("bring what the teacher asks for", items),
            talk("learn the skill", map, npc, x, y)
                .menu(menu)
                .done(Check::Skill(skill)),
        ],
    }
}

/// `skill:<NAME>`, leaked once per quest so ids stay `&'static str`.
pub(super) fn skill_id(skill: &'static str) -> &'static str {
    Box::leak(format!("skill:{skill}").into_boxed_str())
}

/// Nami in the Prontera Chivalry teaches First Aid (`novice_skills.txt`). The bandages are a talk with the
/// Nursing Instructor of the castle, not an item.
fn first_aid() -> Quest {
    Quest {
        id: "skill:NV_FIRSTAID",
        title: "First Aid quest",
        reward: Reward::Skill("NV_FIRSTAID"),
        jobs: &["Novice"],
        job_level: 4,
        base_level: 1,
        sex: None,
        skills: &[],
        stages: vec![
            talk("ask Nami to teach First Aid", "prt_in", "Nami", 234, 133)
                .menu(&["Continue conversation"])
                .said("small preparations"),
            talk(
                "get bandages from the Nursing Instructor",
                "prt_castle",
                "Nursing Instructor",
                175,
                146,
            ),
            gather("bring 3 Red Herb and 3 Clover", &[(507, 3), (705, 3)]),
            talk("show Nami the herbs", "prt_in", "Nami", 234, 133)
                .menu(&["Continue conversation"])
                .done(Check::Skill("NV_FIRSTAID")),
        ],
    }
}

/// Bulma of the Chivalry teaches Play Dead to a Novice who learned First Aid; the Nursing Instructor hands the
/// Newbie Tag he asks for.
fn play_dead() -> Quest {
    Quest {
        id: "skill:NV_TRICKDEAD",
        title: "Play Dead quest",
        reward: Reward::Skill("NV_TRICKDEAD"),
        jobs: &["Novice"],
        job_level: 7,
        base_level: 1,
        sex: None,
        skills: &[("NV_FIRSTAID", 1)],
        stages: vec![
            talk(
                "ask the Chivalry Member about Play Dead",
                "prt_in",
                "Chivalry Member",
                73,
                87,
            )
            .said("Newbie Tag")
            .done(Check::Item(7039)),
            talk(
                "get a Newbie Tag from the Nursing Instructor",
                "prt_castle",
                "Nursing Instructor",
                175,
                146,
            )
            .done(Check::Item(7039)),
            talk(
                "bring the Newbie Tag to the Chivalry Member",
                "prt_in",
                "Chivalry Member",
                73,
                87,
            )
            .done(Check::Skill("NV_TRICKDEAD")),
        ],
    }
}

const SWORDMAN: &[&str] = &["Swordman", "Knight", "Crusader"];
const MAGE: &[&str] = &["Mage", "Wizard", "Sage"];
const ARCHER: &[&str] = &["Archer", "Hunter", "Bard", "Dancer"];
const ACOLYTE: &[&str] = &["Acolyte", "Priest", "Monk"];
const MERCHANT: &[&str] = &["Merchant", "Blacksmith", "Alchemist"];
const THIEF: &[&str] = &["Thief", "Assassin", "Rogue"];

fn first_jobs() -> Vec<Quest> {
    // A second job of the line may take the quest at any job level; only the first job needs the level.
    vec![
        bring(
            "SM_MOVINGRECOVERY",
            "Moving HP Recovery quest",
            SWORDMAN,
            35,
            &[(713, 200), (1058, 1)],
            ("izlude_in", "Knight De Thomas", 175, 130),
            &["Yes."],
        ),
        bring(
            "SM_FATALBLOW",
            "Fatal Blow quest",
            SWORDMAN,
            25,
            &[(1752, 10), (1751, 10), (532, 1), (962, 30), (526, 5)],
            ("prt_in", "Leon Von Frich", 75, 88),
            &["Yes!"],
        ),
        bring(
            "SM_AUTOBERSERK",
            "Auto Berserk quest",
            SWORDMAN,
            30,
            &[(924, 35), (958, 10), (957, 10), (518, 10)],
            ("prt_in", "Juan", 94, 57),
            &["I would like to learn it now"],
        ),
        bring(
            "MG_ENERGYCOAT",
            "Energy Coat quest",
            MAGE,
            35,
            &[(746, 3), (730, 1), (935, 5), (943, 1)],
            ("geffen_in", "Great Wizard", 151, 119),
            &["Energy Coat"],
        ),
        bring(
            "AC_MAKINGARROW",
            "Arrow Crafting quest",
            ARCHER,
            30,
            &[(907, 20), (921, 7), (906, 41), (1019, 13), (501, 1)],
            ("moc_ruins", "Roberto", 118, 99),
            &["It must be hard"],
        ),
        bring(
            "AC_CHARGEARROW",
            "Arrow Repel quest",
            ARCHER,
            35,
            &[(721, 2), (942, 3), (962, 10), (925, 10), (532, 36)],
            ("payon", "Jason", 103, 63),
            &["Teach me"],
        ),
        bring(
            "AL_HOLYLIGHT",
            "Holy Light quest",
            ACOLYTE,
            30,
            &[(727, 1), (991, 1), (2608, 1)],
            ("prt_church", "Cleric", 173, 23),
            &["Holy light"],
        ),
        bring(
            "MC_LOUD",
            "Crazy Uproar quest",
            MERCHANT,
            15,
            &[(722, 7), (532, 1), (921, 50)],
            ("alberta", "Necko", 83, 96),
            &["Yes!!!!"],
        ),
        bring(
            "MC_CHANGECART",
            "Change Cart quest",
            MERCHANT,
            30,
            &[(1019, 50), (998, 10), (919, 20)],
            ("alberta", "Charlron", 119, 221),
            &["That's why I came here"],
        ),
        // Gershaun asks for one of three lists at random each time: the largest covers them all.
        bring(
            "MC_CARTREVOLUTION",
            "Cart Revolution quest",
            MERCHANT,
            35,
            &[(533, 2), (998, 23), (938, 32), (601, 23), (962, 6)],
            ("alberta", "Gershaun", 232, 106),
            &["that's why I came"],
        ),
        bring(
            "TF_BACKSLIDING",
            "Back Slide quest",
            THIEF,
            35,
            &[(940, 20)],
            ("moc_prydb1", "Alcouskou", 154, 128),
            &["Back Slide"],
        ),
        bring(
            "TF_PICKSTONE",
            "Find Stone quest",
            THIEF,
            20,
            &[(912, 1), (948, 1), (908, 5)],
            ("moc_prydb1", "Alcouskou", 154, 128),
            &["Find Stone"],
        ),
        bring(
            "TF_THROWSTONE",
            "Stone Fling quest",
            THIEF,
            15,
            &[(910, 2), (911, 2)],
            ("moc_prydb1", "Alcouskou", 154, 128),
            &["Stone Fling"],
        ),
        sand_attack(),
    ]
}

/// Sand Attack: Fine Grit for Alcouskou, then a Leather Bag of Infinity from the Bag Seller of Payon.
fn sand_attack() -> Quest {
    const ALCOUSKOU: (&str, &str, u16, u16) = ("moc_prydb1", "Alcouskou", 154, 128);
    Quest {
        id: "skill:TF_SPRINKLESAND",
        title: "Sand Attack quest",
        reward: Reward::Skill("TF_SPRINKLESAND"),
        jobs: THIEF,
        job_level: 25,
        base_level: 1,
        sex: None,
        skills: &[],
        stages: vec![
            gather("bring 5 Fine Grit", &[(7041, 5)]),
            talk(
                "give Alcouskou the Fine Grit",
                ALCOUSKOU.0,
                ALCOUSKOU.1,
                ALCOUSKOU.2,
                ALCOUSKOU.3,
            )
            .menu(&["Sand Attack"])
            .done(Check::NoItem(7041)),
            gather(
                "bring a Cactus Needle, an Earthworm Peeling and a Cobweb",
                &[(952, 1), (1055, 1), (1025, 1)],
            ),
            talk(
                "get a Leather Bag of Infinity from the Bag Seller",
                "payon",
                "Bag Seller",
                91,
                77,
            )
            .done(Check::Item(7042)),
            talk(
                "bring the bag to Alcouskou",
                ALCOUSKOU.0,
                ALCOUSKOU.1,
                ALCOUSKOU.2,
                ALCOUSKOU.3,
            )
            .menu(&["Sand Attack"])
            .done(Check::Skill("TF_SPRINKLESAND")),
        ],
    }
}
