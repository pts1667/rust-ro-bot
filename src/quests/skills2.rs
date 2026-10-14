//! Skill quests of the second jobs (rathena `npc/quests/skills/*_skills.txt`). The scripts keep their progress in
//! character variables the bot cannot see, so most steps are checked by what the NPC says.
//!
//! Not here yet: the Monk's (Ki Translation asks for an empty inventory, which needs the Kafra storage), Greed (a
//! full one), Bioethics (the long homunculus story of Lighthalzen), Pang Voice (a cooked drink), Sonic Acceleration
//! (the treasure site is told only in the dialogue, at random) and Close Confine.

use super::build::*;
use super::skills::skill_id;
use super::{Check, Quest, Reward, Stage};

pub fn quests() -> Vec<Quest> {
    vec![
        charge_attack(),
        shrink(),
        sight_blaster(),
        create_converter(),
        phantasmic_arrow(),
        redemptio(),
        unfair_trick(),
        venom_knife(),
        charming_wink(),
    ]
}

fn skill_quest(
    skill: &'static str,
    title: &'static str,
    job: &'static str,
    job_level: u32,
    skills: &'static [(&'static str, u32)],
    stages: Vec<Stage>,
) -> Quest {
    Quest {
        id: skill_id(skill),
        title,
        reward: Reward::Skill(skill),
        jobs: Box::leak(Box::new([job])),
        job_level,
        base_level: 1,
        sex: None,
        skills,
        stages,
    }
}

/// A Knight of the Chivalry misses the old days; the 7th Division training by the Prontera castle shows why, and
/// candy and roses cheer him up enough to share Charge Attack.
fn charge_attack() -> Quest {
    const KNIGHT: (&str, &str, u16, u16) = ("prt_in", "Knight", 85, 99);
    skill_quest(
        "KN_CHARGEATK",
        "Charge Attack quest",
        "Knight",
        1,
        &[],
        vec![
            talk(
                "hear out the Knight of the Chivalry",
                KNIGHT.0,
                KNIGHT.1,
                KNIGHT.2,
                KNIGHT.3,
            )
            .said("visited the 7th Division"),
            walk(
                "watch the 7th Division train by the castle",
                "prontera",
                69,
                351,
            ),
            talk(
                "speak to the Grand Master of the 7th Division",
                "prontera",
                "Grand Master",
                72,
                352,
            )
            .said("that's an order"),
            talk(
                "speak to the shouting Knight",
                "prontera",
                "Knight",
                78,
                354,
            )
            .said("Hooooo-AH"),
            talk(
                "speak to the Knight with a wife",
                "prontera",
                "Knight",
                73,
                357,
            )
            .said("necklace for my wife"),
            talk(
                "speak to the boastful Knight",
                "prontera",
                "Knight",
                78,
                357,
            )
            .said("Essofeit"),
            talk(
                "tell the Knight of the Chivalry what you saw",
                KNIGHT.0,
                KNIGHT.1,
                KNIGHT.2,
                KNIGHT.3,
            )
            .said("It's all in the past"),
            gather(
                "bring 5 Candy Canes and 3 Witherless Roses",
                &[(530, 5), (748, 3)],
            ),
            talk(
                "cheer the Knight up",
                KNIGHT.0,
                KNIGHT.1,
                KNIGHT.2,
                KNIGHT.3,
            )
            .menu(&["Give him Candy Canes and Witherless Roses"])
            .times(2)
            .done(Check::Skill("KN_CHARGEATK")),
        ],
    )
}

/// Ford of Geffen sends a Crusader to a wounded soldier on the fields; a potion, a report, and Father Arthur's antidote
/// save him, and he teaches Shrink.
fn shrink() -> Quest {
    const FORD: (&str, &str, u16, u16) = ("geffen", "Ford", 110, 117);
    const SOLDIER: (&str, &str, u16, u16) = ("gef_fild13", "Soldier", 297, 242);
    const ARTHUR: (&str, &str, u16, u16) = ("prt_church", "Pastor", 87, 127);
    skill_quest(
        "CR_SHRINK",
        "Shrink quest",
        "Crusader",
        1,
        &[],
        vec![
            talk("offer Ford your help", FORD.0, FORD.1, FORD.2, FORD.3)
                .menu(&["Sure."])
                .said("Your help is a great relief"),
            talk(
                "find the wounded soldier on the field",
                SOLDIER.0,
                SOLDIER.1,
                SOLDIER.2,
                SOLDIER.3,
            )
            .said("bring him a Red Potion"),
            gather("bring the soldier a Red Potion", &[(501, 1)]),
            talk(
                "give the soldier the Red Potion",
                SOLDIER.0,
                SOLDIER.1,
                SOLDIER.2,
                SOLDIER.3,
            )
            .said("smudged with"),
            talk(
                "bring Ford the soldier's report",
                FORD.0,
                FORD.1,
                FORD.2,
                FORD.3,
            )
            .said("too late for Sloutii"),
            talk(
                "ask Father Arthur for an antidote",
                ARTHUR.0,
                ARTHUR.1,
                ARTHUR.2,
                ARTHUR.3,
            )
            .said("if he's poisoned"),
            gather(
                "bring Father Arthur the antidote's ingredients",
                &[
                    (514, 1),
                    (619, 1),
                    (1003, 1),
                    (7053, 3),
                    (713, 3),
                    (909, 5),
                    (938, 20),
                ],
            ),
            talk(
                "have Father Arthur distill the antidote",
                ARTHUR.0,
                ARTHUR.1,
                ARTHUR.2,
                ARTHUR.3,
            )
            .times(2)
            .said("Godspeed, adventurer"),
            talk(
                "give the soldier the antidote",
                SOLDIER.0,
                SOLDIER.1,
                SOLDIER.2,
                SOLDIER.3,
            )
            .done(Check::Skill("CR_SHRINK")),
        ],
    )
}

/// Meow, the sleepy wizard of the Geffen tower, teaches Sight Blaster to one who learned the theory of push backs at
/// the Prontera Chivalry and brings elemental stones.
fn sight_blaster() -> Quest {
    const MEOW: (&str, &str, u16, u16) = ("gef_tower", "Meow", 115, 36);
    skill_quest(
        "WZ_SIGHTBLASTER",
        "Sight Blaster quest",
        "Wizard",
        1,
        &[],
        vec![
            talk(
                "wake Meow in the Geffen tower",
                MEOW.0,
                MEOW.1,
                MEOW.2,
                MEOW.3,
            )
            .menu(&[
                "A cat?",
                "Sure, I'd like to learn a new skill.",
                "I can push enemies away?",
            ])
            .said("Push Backs"),
            talk(
                "learn the theory of push backs from Simon Mayace",
                "prt_in",
                "Simon Mayace",
                82,
                98,
            )
            .menu(&["I want to learn about push backs.", "But at close range..."])
            .said("Good luck on your travels"),
            talk(
                "tell Meow you learned the theory",
                MEOW.0,
                MEOW.1,
                MEOW.2,
                MEOW.3,
            )
            .said("another nap"),
            gather(
                "bring Meow 10 of each elemental stone",
                &[(991, 10), (993, 10), (990, 10), (992, 10)],
            ),
            talk("give Meow the stones", MEOW.0, MEOW.1, MEOW.2, MEOW.3)
                .done(Check::Skill("WZ_SIGHTBLASTER")),
        ],
    )
}

/// Mischna of the Juno Sage castle makes an Elemental Converter before a Sage, given the materials.
fn create_converter() -> Quest {
    const MISCHNA: (&str, &str, u16, u16) = ("yuno_in03", "Mischna", 176, 24);
    skill_quest(
        "SA_CREATECON",
        "Elemental Converter quest",
        "Sage",
        1,
        &[],
        vec![
            talk(
                "ask Mischna for new knowledge",
                MISCHNA.0,
                MISCHNA.1,
                MISCHNA.2,
                MISCHNA.3,
            )
            .menu(&["I seek new knowledge.", "Yes, I am."])
            .said("need these items to create"),
            gather(
                "bring Mischna the converter's materials",
                &[(904, 10), (947, 7), (1013, 12), (946, 10), (7433, 4)],
            ),
            talk(
                "watch Mischna make a converter",
                MISCHNA.0,
                MISCHNA.1,
                MISCHNA.2,
                MISCHNA.3,
            )
            .done(Check::Skill("SA_CREATECON")),
        ],
    )
}

/// Arpesto of the Payon archer village shows Phantasmic Arrow for jewels, feathers and pet food.
fn phantasmic_arrow() -> Quest {
    const ARPESTO: (&str, &str, u16, u16) = ("pay_arche", "Arpesto", 109, 169);
    skill_quest(
        "HT_PHANTASMIC",
        "Phantasmic Arrow quest",
        "Hunter",
        40,
        &[],
        vec![
            talk(
                "ask Arpesto to teach you",
                ARPESTO.0,
                ARPESTO.1,
                ARPESTO.2,
                ARPESTO.3,
            )
            .menu(&["Yes, please!"])
            .said("me the following items"),
            gather(
                "bring Arpesto rubies, harpy feathers and pet food",
                &[(724, 5), (7115, 5), (537, 30)],
            ),
            talk(
                "give Arpesto the items",
                ARPESTO.0,
                ARPESTO.1,
                ARPESTO.2,
                ARPESTO.3,
            )
            .done(Check::Skill("HT_PHANTASMIC")),
        ],
    )
}

/// Sister Linus of the Prontera church teaches Redemptio to a Priest who knows Resurrection; each try costs a Holy
/// Water and a Blue Gemstone, and it may take a few.
fn redemptio() -> Quest {
    const LINUS: (&str, &str, u16, u16) = ("prt_church", "Sister Linus", 111, 112);
    skill_quest(
        "PR_REDEMPTIO",
        "Redemptio quest",
        "Priest",
        1,
        &[("ALL_RESURRECTION", 1)],
        vec![
            talk(
                "ask Sister Linus about her prayers",
                LINUS.0,
                LINUS.1,
                LINUS.2,
                LINUS.3,
            )
            .menu(&["Whom are you praying for, sister?", "Ancient spell...?"])
            .said("praying for her safety"),
            talk(
                "ask Sister Linus to teach Redemptio",
                LINUS.0,
                LINUS.1,
                LINUS.2,
                LINUS.3,
            )
            .menu(&["Please teach me"])
            .said("so you can attempt"),
            gather(
                "bring 30 Holy Water and 20 Blue Gemstones",
                &[(523, 30), (717, 20)],
            ),
            talk("try to learn Redemptio", LINUS.0, LINUS.1, LINUS.2, LINUS.3)
                .done(Check::Skill("PR_REDEMPTIO")),
        ],
    )
}

/// Akkie of Geffen tries to forge with a Blacksmith's materials until Dubious Salesmanship (Unfair Trick) comes out of
/// it. Christopher's workshop sells the Iron Hammers.
fn unfair_trick() -> Quest {
    const AKKIE: (&str, &str, u16, u16) = ("geffen", "Akkie", 178, 72);
    skill_quest(
        "BS_UNFAIRLYTRICK",
        "Unfair Trick quest",
        "Blacksmith",
        30,
        &[],
        vec![
            talk("offer Akkie your help", AKKIE.0, AKKIE.1, AKKIE.2, AKKIE.3)
                .menu(&["Sure."])
                .said("bring me those"),
            gather_zeny("save for 2 Iron Hammers and Akkie's fee", &[], 2_500),
            talk(
                "buy 2 Iron Hammers from Christopher in Geffen",
                "geffen_in",
                "Christopher",
                110,
                172,
            )
            .menu(&["Purchase Forging Item", "Iron Hammer - 1000 zeny"])
            .answers(&[("How many do ye wish", "2")])
            .done(Check::Count(613, 2)),
            gather_zeny(
                "bring Akkie steel, coal and a Detrimindexta",
                &[(999, 2), (1003, 8), (971, 1), (613, 2)],
                500,
            ),
            // Each try uses some of it; he gets there in the end.
            talk("forge with Akkie", AKKIE.0, AKKIE.1, AKKIE.2, AKKIE.3)
                .times(6)
                .done(Check::Skill("BS_UNFAIRLYTRICK")),
        ],
    )
}

/// The Assassin by the guild's entrance teaches Venom Knife for the asking.
fn venom_knife() -> Quest {
    skill_quest(
        "AS_VENOMKNIFE",
        "Venom Knife quest",
        "Assassin",
        1,
        &[],
        vec![
            talk(
                "ask the Assassin at the guild for new skills",
                "in_moc_16",
                "Assassin",
                14,
                27,
            )
            .menu(&["New skills?", "Learn Venom Knife", "Venom Knife"])
            .done(Check::Skill("AS_VENOMKNIFE")),
        ],
    )
}

/// Canell, an old dancer of Comodo, wants a Crystal Mirror and her sister Aelle in Prontera a picnic, before she
/// teaches Charming Wink.
fn charming_wink() -> Quest {
    const CANELL: (&str, &str, u16, u16) = ("comodo", "Canell", 204, 172);
    const AELLE: (&str, &str, u16, u16) = ("prontera", "Aelle", 183, 333);
    skill_quest(
        "DC_WINKCHARM",
        "Charming Wink quest",
        "Dancer",
        40,
        &[],
        vec![
            talk(
                "listen to Canell in Comodo",
                CANELL.0,
                CANELL.1,
                CANELL.2,
                CANELL.3,
            )
            .menu(&[
                "Of course, old crone!",
                "Alright, I'll see what I can learn.",
            ])
            .said("so fetch it"),
            gather("bring Canell a Crystal Mirror", &[(747, 1)]),
            talk(
                "give Canell the mirror",
                CANELL.0,
                CANELL.1,
                CANELL.2,
                CANELL.3,
            )
            .times(2)
            .said("keep your mirror"),
            talk(
                "find Canell's sister Aelle in Prontera",
                AELLE.0,
                AELLE.1,
                AELLE.2,
                AELLE.3,
            )
            .times(2)
            .said("too much trouble"),
            gather(
                "bring Aelle a picnic",
                &[(970, 1), (512, 1), (513, 3), (538, 5), (736, 1)],
            ),
            talk("give Aelle the picnic", AELLE.0, AELLE.1, AELLE.2, AELLE.3)
                .times(2)
                .said("my sister"),
            talk("dance for Canell", CANELL.0, CANELL.1, CANELL.2, CANELL.3)
                .menu(&["Un, deux, trois~"])
                .times(2)
                .done(Check::Skill("DC_WINKCHARM")),
        ],
    )
}
