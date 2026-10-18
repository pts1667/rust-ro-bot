//! The second-job quests of the guilds, as the wiki guides tell them and the scripts this server runs (rathena
//! `npc/jobs/2-1`, `npc/jobs/2-2`, pre-renewal where it differs) carry them out. Quiz answers are those the
//! scripts count as right; menus are picked by their text.

use super::build::*;
use super::{Check, Dance, Items, Quest, Reward};

pub fn quests() -> Vec<Quest> {
    vec![
        knight(),
        crusader(),
        wizard(),
        sage(),
        hunter(),
        priest(),
        monk(),
        blacksmith(),
        alchemist(),
        assassin(),
        rogue(),
        bard(),
        dancer(),
    ]
}

fn job_quest(job: &'static str, from: &'static [&'static str], stages: Vec<super::Stage>) -> Quest {
    Quest {
        id: Box::leak(format!("job:{job}").into_boxed_str()),
        title: Box::leak(format!("{job} job quest").into_boxed_str()),
        reward: Reward::Job(job),
        jobs: from,
        job_level: 40,
        base_level: 1,
        sex: None,
        reborn: Some(false),
        past: &[],
        no_cart: false,
        skills: &[],
        stages,
    }
}

/// The Prontera Chivalry (`2-1/knight.txt`): the quest window shows ids 9000 to 9012 along the way.
fn knight() -> Quest {
    const CAPTAIN: (&str, &str, u16, u16) = ("prt_in", "Chivalry Captain", 88, 101);
    const ANDREW: (&str, &str, u16, u16) = ("prt_in", "Sir Andrew", 75, 107);
    job_quest(
        "Knight",
        &["Swordman"],
        vec![
            talk(
                "apply to the Chivalry Captain",
                CAPTAIN.0,
                CAPTAIN.1,
                CAPTAIN.2,
                CAPTAIN.3,
            )
            .menu(&[
                "I want to change my job to a Knight",
                "Yes, I would like to apply",
            ])
            .done(Check::Quest(9000)),
            talk(
                "ask Sir Andrew for his test",
                ANDREW.0,
                ANDREW.1,
                ANDREW.2,
                ANDREW.3,
            )
            .menu(&["I would like to take the test"])
            .assigns(&[
                &[
                    (1040, 5),
                    (7006, 5),
                    (931, 5),
                    (1057, 5),
                    (903, 5),
                    (1028, 5),
                ],
                &[
                    (1042, 5),
                    (950, 5),
                    (1032, 5),
                    (966, 5),
                    (7031, 5),
                    (946, 5),
                ],
            ])
            .done(Check::Quest(9001))
            .done(Check::Quest(9002))
            .done(Check::Quest(9003))
            .log(&[9000]),
            gather_assigned("bring Sir Andrew the items he asked for").log(&[9001, 9002]),
            talk(
                "bring Sir Andrew the items",
                ANDREW.0,
                ANDREW.1,
                ANDREW.2,
                ANDREW.3,
            )
            .done(Check::Quest(9003))
            .log(&[9001, 9002]),
            talk(
                "answer Sir Siracuse's questions",
                "prt_in",
                "Sir Siracuse",
                71,
                91,
            )
            .menu(&["Sir Andrew sent me", "I wish to take the test again"])
            .answers(&[
                ("not affected by the Two Hand Quicken", "Flamberge"),
                ("not necessary to learn Bowling Bash", "Provoke Lv.10"),
                (
                    "not necessary to learn the skill Brandish Spear",
                    "Spear Boomerang Lv.3",
                ),
                ("which has the Ghost attribute", "Zephyrus"),
                ("Level 3 Cavalier Mastery", "80 %"),
                ("Novice asking for help", "reasonable hunting area"),
                ("how should you act within", "Protect everyone"),
                ("important value", "Honor"),
            ])
            .done(Check::Quest(9004))
            .log(&[9003]),
            talk(
                "ask Sir Windsor for the battle test",
                "prt_in",
                "Sir Windsor",
                79,
                94,
            )
            .menu(&["Sir Siracuse sent me", "I want to try again"])
            .done(Check::OnMap("job_knt"))
            .log(&[9004, 9005, 9006]),
            room("wait for the battle test", "job_knt", "Windsor Benedict").inside(),
            fight("win the three rounds of the battle test", "job_knt").passed(Check::Quest(9007)),
            talk(
                "take Lady Amy's etiquette test",
                "prt_in",
                "Lady Amy",
                69,
                107,
            )
            .menu(&["Sir Windsor told me to", "I would like to take the test"])
            .answers(&[
                (
                    "looking for a party in Morocc",
                    "Look for people seeking Knights",
                ),
                ("Level 4", "Check out the area and plan ahead"),
                ("rude players", "Keep the monsters from reaching the party"),
                ("collapsed on the ground", "Ask your party's Priest to help"),
                ("rare item during", "Decide with party who gets it"),
                ("do to sell your item", "Inquire if there is anyone"),
                ("begs", "Give suggestions for a place to hunt"),
                ("someone that is lost", "Lead the person to the exit"),
                (
                    "the Priest for a Heal",
                    "Would it be possible to get a heal please",
                ),
                ("rare item on", "Ask around to find the owner"),
            ])
            .done(Check::Quest(9009))
            .log(&[9007, 9008]),
            talk(
                "ask Sir Edmond for the patience test",
                "prt_in",
                "Sir Edmond",
                70,
                99,
            )
            .menu(&["Lady Amy sent me", "I'm sorry, I didn't mean to"])
            .done(Check::OnMap("job_knt"))
            .log(&[9009, 9010]),
            // Killing anything ends the test: stand among the Porings until the way back opens.
            endure("wait among the monsters without attacking", "job_knt")
                .passed(Check::Quest(9011)),
            talk("answer Sir Gray", "prt_in", "Sir Gray", 87, 92)
                .menu(&[
                    "I've been thinking a lot",
                    "I would like to take the test",
                    "To become stronger",
                    "I can protect others",
                    "There are those waiting for me",
                    "My friends",
                ])
                .done(Check::Quest(9012))
                .log(&[9011]),
            talk(
                "become a Knight at the Chivalry Captain",
                CAPTAIN.0,
                CAPTAIN.1,
                CAPTAIN.2,
                CAPTAIN.3,
            )
            .done(Check::Job("Knight"))
            .log(&[9012]),
        ],
    )
}

/// The Crusaders of Prontera Castle (`2-2/crusader.txt`, pre-renewal places): quest window ids 3006 to 3015. The
/// Rosary is worn for the tests; the endurance walk and the last battle both happen on `job_cru`.
fn crusader() -> Quest {
    const SENIOR: (&str, &str, u16, u16) = ("prt_castle", "Senior Crusader", 45, 169);
    const ANGUISH: (&str, &str, u16, u16) = ("prt_castle", "Man in Anguish", 164, 32);
    const PATRON: (&str, &str, u16, u16) = ("prt_castle", "Patron Knight", 35, 151);
    const ROSARY: u32 = 2608;
    job_quest(
        "Crusader",
        &["Swordman"],
        vec![
            gather("get a Rosary to wear for the tests", &[(ROSARY, 1)]),
            talk(
                "ask the Senior Crusader for the tests",
                SENIOR.0,
                SENIOR.1,
                SENIOR.2,
                SENIOR.3,
            )
            .menu(&["I want to prepare for the Holy War", "Yes, I do"])
            .assigns(&[
                &[(957, 10), (959, 10), (1099, 10), (901, 10)],
                &[(932, 10), (1043, 10), (1098, 10), (1094, 10)],
                &[(958, 10), (930, 10), (1041, 10), (1062, 10)],
            ])
            .done(Check::Quest(3006))
            .done(Check::Quest(3007))
            .done(Check::Quest(3008))
            .done(Check::Quest(3009)),
            gather_assigned("bring the Senior Crusader the items he asked for")
                .log(&[3006, 3007, 3008]),
            talk(
                "bring the Senior Crusader the items",
                SENIOR.0,
                SENIOR.1,
                SENIOR.2,
                SENIOR.3,
            )
            .done(Check::Quest(3009))
            .log(&[3006, 3007, 3008]),
            talk(
                "ask the Man in Anguish for the endurance test",
                ANGUISH.0,
                ANGUISH.1,
                ANGUISH.2,
                ANGUISH.3,
            )
            .menu(&[
                "I'd like to take the Crusader test",
                "Let me retake the test",
            ])
            .wear(ROSARY)
            .done(Check::OnMap("job_cru"))
            .log(&[3009, 3010]),
            // Killing any of the monsters on the way ends the test.
            walk(
                "walk past the monsters to the end without killing any",
                "job_cru",
                98,
                105,
            )
            .inside()
            .area(80, 30, 116, 112)
            .done(Check::Quest(3011)),
            talk(
                "answer Gabriel Valentine's questions",
                "prt_church",
                "Crusader",
                95,
                127,
            )
            .answers(&[
                ("most effective in atttacking the Undead", "Holy"),
                ("Level 2 Undead", "25 %"),
                ("can you not get from an Evil Druid", "Monk Hat"),
                ("Undead monster has the highest HP", "Zombie Prisoner"),
                ("following monsters is a different size", "Drake"),
                ("tolerance to Undead property", "Orc Zombie Card"),
                (
                    "relationship between Munak and Bongun",
                    "Childhood friends in the same village",
                ),
                ("monsters is not aggressive", "Skeleton"),
                ("Munak Card has been inserted", "Amulet Shield"),
                ("does not drop Memento", "Munak"),
                ("different attribute than the others", "Isis"),
                ("effective in attacking Demon monsters", "Decussate Tsurugi"),
                ("NOT dropped by Dokebi", "Golden Hammer"),
                ("Demon monster has the most HP", "Marionette"),
                ("Demon monster is a different size", "Ghostring"),
                ("reduces damage inflicted by Demon", "Shield from Hell"),
                ("most effective on the Wind Ghost", "Earth"),
                ("different from the other Demon", "Whisper"),
                (
                    "Marionette Card have",
                    "Increase defense against Ghost attacks by 30 %",
                ),
                (
                    "encountering a demon monster",
                    "Put Holy Water on a weapon and attack.",
                ),
                ("need to learn 'Demon Bane", "Level 3"),
                ("how much HP does Level 5 Heal", "440"),
                ("Level 7 Divine Protection", "21"),
                ("endowed with the Ghost attribute", "Zephyrus"),
                ("need to learn 'Cure", "Level 2"),
                ("Level 3 Cavalier Mastery", "80 % of normal speed"),
                (
                    "not correct of the Demon Bane",
                    "Only Acolytes can learn the skill",
                ),
                ("SP does Level 7 Heal", "31"),
                ("cured with the Cure skill", "Curse"),
                (
                    "best describes a Crusader",
                    "One preparing for the Holy War.",
                ),
            ])
            .done(Check::Quest(3013))
            .log(&[3011, 3012]),
            gather("bring Holy Water for the purification", &[(523, 1)]).log(&[3013, 3014]),
            talk(
                "ask the Patron Knight for the battle test",
                PATRON.0,
                PATRON.1,
                PATRON.2,
                PATRON.3,
            )
            .menu(&["I would like to begin"])
            .wear(ROSARY)
            .done(Check::OnMap("job_cru"))
            .log(&[3013, 3014]),
            room("wait for the battle test", "job_cru", "Waiting Room")
                .inside()
                .area(5, 150, 45, 200),
            // Four minutes up the corridor; the last monster is a Ghoul named after the bot, and the way out opens
            // when it falls.
            fight(
                "defeat the undead up the corridor and leave by the far end",
                "job_cru",
            )
            .area(155, 10, 180, 190)
            .ahead(168, 180),
            talk(
                "become a Crusader at the Senior Crusader",
                SENIOR.0,
                SENIOR.1,
                SENIOR.2,
                SENIOR.3,
            )
            .done(Check::Job("Crusader"))
            .log(&[3015]),
        ],
    )
}

/// The Wizard Guild of the Geffen tower (`2-1/wizard.txt`): quest window ids 9013 to 9018. Raulel, the Gloomy
/// Wizard, asks the questions and sends the bot to the three rooms of `job_wiz` (water, earth, fire), each to
/// clear before its time runs out.
fn wizard() -> Quest {
    const GUILDSMAN: (&str, &str, u16, u16) = ("gef_tower", "Wizard Guildsman", 111, 37);
    const RAULEL: (&str, &str, u16, u16) = ("gef_tower", "Gloomy Wizard", 102, 24);
    job_quest(
        "Wizard",
        &["Mage"],
        vec![
            talk(
                "apply to the Wizard Guildsman",
                GUILDSMAN.0,
                GUILDSMAN.1,
                GUILDSMAN.2,
                GUILDSMAN.3,
            )
            .menu(&["I want to become a Wizard", "Yes, I do"])
            .assigns(&[
                &[(716, 10), (717, 10), (715, 10)],
                &[(991, 5), (993, 5), (990, 5), (992, 5)],
            ])
            .done(Check::Quest(9013))
            .done(Check::Quest(9014))
            .done(Check::Quest(9015)),
            gather_assigned("bring the Wizard Guildsman the items he asked for").log(&[9013, 9014]),
            talk(
                "bring the Wizard Guildsman the items",
                GUILDSMAN.0,
                GUILDSMAN.1,
                GUILDSMAN.2,
                GUILDSMAN.3,
            )
            .done(Check::Quest(9015))
            .log(&[9013, 9014]),
            talk(
                "answer Raulel's questions",
                RAULEL.0,
                RAULEL.1,
                RAULEL.2,
                RAULEL.3,
            )
            .menu(&["I would like to continue with the tests"])
            .answers(&[
                ("not necessary to learn Fire Wall", "Napalm Beat Lv 4"),
                ("cast Frost Diver on it", "Water"),
                ("completely master Napalm Beat", "1.7 times"),
                ("casting Stone Curse", "Red Gemstone"),
                (
                    "not required to master Safety Wall",
                    "Increase SP Recovery Lv 6",
                ),
                ("Increase SP Recovery Lv 7", "21"),
                ("Using Energy Coat", "Damage 18% SP2%"),
                ("Safety Wall Lv 6", "SP 40, 7 times"),
                ("Lv 10 Thunderstorm", "74"),
                ("training in the Byalan Dungeon", "Lightning Bolt"),
                ("slotted Guard", "Pupa"),
                ("easiest monster for a low level Mage", "Flora"),
                ("not be affected by Stone Curse", "Evil Druid"),
                ("Lv 3 water attribute monster", "200%"),
                ("Baby Desert Wolf and a Familiar fought", "Baby Desert Wolf"),
                ("cannot be a Cute Pet", "Roda Frog"),
                ("weak against a fire attribute", "Hammer Goblin"),
                ("has the highest defense", "Caramel"),
                ("of a different species", "Ghostring"),
                ("not an Undead monster", "Deviace"),
                ("most important for a Mage", "INT"),
                ("does not have a 'Bolt'", "Earth"),
                ("does not relate to a Mage", "Good at selling stuff."),
                ("home of Mages", "Geffen"),
                ("nothing to do with INT", "Soldier Andre Card"),
                ("Mage good at compared", "Exceptional Magic Skills"),
                ("INT bonus at Job Lv 40", "5"),
                ("Mage not equip", "Boys Cap"),
                ("Mage test solution 3", "Blue Gemstone"),
                ("irrelevant to magic", "Magnolia Card"),
            ])
            .done(Check::Quest(9017))
            .log(&[9015, 9016]),
            // Each failed try sends the bot back to Raulel, who asks five more questions before the next try.
            talk(
                "ask Raulel for the final test",
                RAULEL.0,
                RAULEL.1,
                RAULEL.2,
                RAULEL.3,
            )
            .menu(&[
                "No, it's ok, I'm ready",
                "Continue testing",
                "Continue the test",
                "Begin the test please",
            ])
            .answers(&[
                ("different attribute than the others", "Cornutus"),
                ("not a looting one", "Zerom"),
                ("does not recognize casting", "Marina"),
                ("efficient against a Marine Sphere", "Lightning Bolt"),
                ("monster that can move", "Frilldora"),
            ])
            .done(Check::OnMap("job_wiz"))
            .log(&[9017]),
            room("wait for the final test", "job_wiz", "Arena Assistant")
                .inside()
                .area(40, 140, 75, 185),
            fight(
                "clear the rooms of water, earth and fire in time",
                "job_wiz",
            )
            .passed(Check::Quest(9018)),
            talk(
                "become a Wizard at the Wizard Guildsman",
                GUILDSMAN.0,
                GUILDSMAN.1,
                GUILDSMAN.2,
                GUILDSMAN.3,
            )
            .done(Check::Job("Wizard"))
            .log(&[9018]),
        ],
    )
}

/// The Sage Academy of Juno (`2-2/sage.txt`): quest window ids 2041 to 2062. The fee is paid in zeny, the written
/// test answered, the practical test fought, and then Hermes sends each student to one of three professors (the
/// branches), whose lessons end with a thesis to take to the Dean.
fn sage() -> Quest {
    const DEAN: (&str, &str, u16, u16) = ("yuno_in02", "Dean of the Academy", 38, 61);
    const STAFF: (&str, &str, u16, u16) = ("yuno_in03", "Staff of the Academy", 154, 35);
    const CLAYTOS: (&str, &str, u16, u16) = ("yuno_in03", "Written Test Professor", 105, 177);
    const HERMES: (&str, &str, u16, u16) = ("yuno_in03", "Practical Examination P", 169, 180);
    const HISTORY: (&str, &str, u16, u16) = ("yuno_in03", "History Professor", 62, 176);
    const BIOLOGY: (&str, &str, u16, u16) = ("yuno_in03", "Biology Professor", 32, 102);
    const PHYSICS: (&str, &str, u16, u16) = ("yuno_in03", "Physics Professor", 244, 31);
    // Feather of Birds, Animal Skin, Trunk, Squid Ink and an Empty Bottle make the thesis.
    const THESIS: Items = &[(916, 1), (919, 1), (1019, 1), (1024, 1), (713, 1)];
    job_quest(
        "Sage",
        &["Mage"],
        vec![
            gather_zeny("save the 70,000 zeny of the academy fee", &[], 70_000),
            talk(
                "enroll at the Staff of the Academy",
                STAFF.0,
                STAFF.1,
                STAFF.2,
                STAFF.3,
            )
            .menu(&[
                "I want to enroll in the school",
                "Yes, I do",
                "Pay 70,000 zeny",
            ])
            .done(Check::Quest(2041)),
            talk(
                "take Claytos' written test",
                CLAYTOS.0,
                CLAYTOS.1,
                CLAYTOS.2,
                CLAYTOS.3,
            )
            .answers(SAGE_WRITTEN_TEST)
            .done(Check::Quest(2046))
            .log(&[2041]),
            // Before the practical test he sends the bot in; after it, he sends it to a professor.
            talk(
                "ask Hermes for the practical test",
                HERMES.0,
                HERMES.1,
                HERMES.2,
                HERMES.3,
            )
            .menu(&["Yes, I am"])
            .done(Check::OnMap("job_sage"))
            .done(Check::Quest(2047))
            .done(Check::Quest(2048))
            .done(Check::Quest(2049))
            .log(&[2046]),
            room("wait for the practical test", "job_sage", "Waiting Room")
                .inside()
                .area(30, 140, 70, 185),
            // The last round: only Academic Probation counts, the absences are left alone.
            fight("defeat the monsters of the three rounds", "job_sage")
                .area(98, 80, 133, 115)
                .spare(&[
                    "Absent 3 times",
                    "Being Late 5 times",
                    "Cheating 2 times",
                    "Cheating 4 times",
                ])
                .passed(Check::OnMap("yuno_in03")),
            // History: Saphien wants a gift of Yggdrasil.
            talk(
                "attend Saphien's history lesson",
                HISTORY.0,
                HISTORY.1,
                HISTORY.2,
                HISTORY.3,
            )
            .menu(&["Yes, I know very well", "It's the source of life"])
            .done(Check::Quest(2050))
            .log(&[2047])
            .branch("history"),
            gather("bring Saphien a Yggdrasil Leaf", &[(610, 1)])
                .log(&[2050])
                .branch("history"),
            talk(
                "give Saphien the Yggdrasil Leaf",
                HISTORY.0,
                HISTORY.1,
                HISTORY.2,
                HISTORY.3,
            )
            .done(Check::Quest(2051))
            .log(&[2050])
            .branch("history"),
            // Biology: Lucius asks for two lists of monster parts, one at a time.
            talk(
                "attend Lucius' biology lesson",
                BIOLOGY.0,
                BIOLOGY.1,
                BIOLOGY.2,
                BIOLOGY.3,
            )
            .menu(&["Well, I can't even count"])
            .assigns(&[
                &[(962, 5), (1052, 5), (1023, 5)],
                &[(960, 5), (966, 5), (950, 5)],
                &[(1050, 5), (960, 5), (963, 5)],
            ])
            .done(Check::Quest(2053))
            .done(Check::Quest(2054))
            .log(&[2048])
            .branch("biology"),
            gather_assigned("bring Lucius the first monster parts")
                .log(&[2053, 2054])
                .branch("biology"),
            talk(
                "bring Lucius the first monster parts",
                BIOLOGY.0,
                BIOLOGY.1,
                BIOLOGY.2,
                BIOLOGY.3,
            )
            .answers(&[
                (
                    "Can you tell me what that similarity is",
                    "They possess water property.",
                ),
                ("work best on most fish", "Lightening Bolt."),
            ])
            .assigns(&[
                &[(1025, 5), (935, 5), (928, 5)],
                &[(947, 5), (946, 5), (1057, 5)],
                &[(1031, 5), (955, 5), (1013, 5)],
                &[(1025, 5), (1031, 5), (943, 5)],
            ])
            .done(Check::Quest(2056))
            .done(Check::Quest(2057))
            .done(Check::Quest(2058))
            .done(Check::Quest(2059))
            .log(&[2053, 2054])
            .branch("biology"),
            gather_assigned("bring Lucius the second monster parts")
                .log(&[2056, 2057, 2058, 2059])
                .branch("biology"),
            talk(
                "bring Lucius the second monster parts",
                BIOLOGY.0,
                BIOLOGY.1,
                BIOLOGY.2,
                BIOLOGY.3,
            )
            .done(Check::Quest(2051))
            .log(&[2056, 2057, 2058, 2059])
            .branch("biology"),
            // Physics: Aebecee wants stones, elemental arrows and Holy Water.
            talk(
                "attend Aebecee's physics lesson",
                PHYSICS.0,
                PHYSICS.1,
                PHYSICS.2,
                PHYSICS.3,
            )
            .done(Check::Quest(2060))
            .log(&[2049])
            .branch("physics"),
            gather("bring Aebecee 30 Stones", &[(7049, 30)])
                .log(&[2060])
                .branch("physics"),
            talk(
                "give Aebecee the Stones",
                PHYSICS.0,
                PHYSICS.1,
                PHYSICS.2,
                PHYSICS.3,
            )
            .done(Check::Quest(2061))
            .log(&[2060])
            .branch("physics"),
            gather(
                "bring Aebecee 50 Crystal, Stone and Wind Arrows",
                &[(1754, 50), (1756, 50), (1755, 50)],
            )
            .log(&[2061])
            .branch("physics"),
            talk(
                "give Aebecee the arrows",
                PHYSICS.0,
                PHYSICS.1,
                PHYSICS.2,
                PHYSICS.3,
            )
            .done(Check::Quest(2062))
            .log(&[2061])
            .branch("physics"),
            gather("bring Aebecee a Holy Water", &[(523, 1)])
                .log(&[2062])
                .branch("physics"),
            talk(
                "give Aebecee the Holy Water",
                PHYSICS.0,
                PHYSICS.1,
                PHYSICS.2,
                PHYSICS.3,
            )
            .done(Check::Quest(2051))
            .log(&[2062])
            .branch("physics"),
            // The thesis, with the professor of the branch.
            gather("gather what the thesis is written with", THESIS).log(&[2051]),
            talk(
                "write the thesis with Saphien",
                HISTORY.0,
                HISTORY.1,
                HISTORY.2,
                HISTORY.3,
            )
            .answers(&[
                ("coiled up by", "A giant snake Yormungandr."),
                (
                    "consists of three places such as",
                    "Utgard, where titans live,",
                ),
                ("Utgard where titans live", "Midgard, where humans live,"),
                ("where humans live in", "Asgard, where gods live."),
                ("Yggdrasil is...", "A giant ash tree."),
                ("reach to 3 places", "Asgard, Jotunnheim, Niflheim."),
                (
                    "has a Seed of Yggdrasil",
                    "can restore half of total HP and SP.",
                ),
            ])
            .done(Check::Quest(2052))
            .log(&[2051])
            .branch("history"),
            talk(
                "write the thesis with Lucius",
                BIOLOGY.0,
                BIOLOGY.1,
                BIOLOGY.2,
                BIOLOGY.3,
            )
            .done(Check::Quest(2052))
            .log(&[2051])
            .branch("biology"),
            talk(
                "write the thesis with Aebecee",
                PHYSICS.0,
                PHYSICS.1,
                PHYSICS.2,
                PHYSICS.3,
            )
            .done(Check::Quest(2052))
            .log(&[2051])
            .branch("physics"),
            talk(
                "hand the thesis to the Dean and become a Sage",
                DEAN.0,
                DEAN.1,
                DEAN.2,
                DEAN.3,
            )
            .done(Check::Job("Sage"))
            .log(&[2052]),
        ],
    )
}

/// Claytos asks twenty questions of one of three sets; 80 points of 100 pass.
const SAGE_WRITTEN_TEST: &[(&str, &str)] = &[
    ("Gift merchant in Prontera does not sell", "Bouquet"),
    ("cannot purchase a Stiletto", "Prontera"),
    ("closest city to Turtle Island", "Alberta"),
    ("different type than the others", "Aster"),
    ("has a different attribute than the others", "Metaller"),
    ("different sized than the others", "Raydric"),
    ("doesn't drop 'Alcohol'", "Poison Spore"),
    ("irrelevant to the Knight job change", "Thomas Servantes"),
    ("not a citizen of Prontera", "Pina"),
    ("Kafra lady who wears glasses", "Leilah"),
    ("lvl 7 Thunderstorm", "59"),
    ("Energy Coat skill when the caster", "Damage -18% SP2%"),
    ("irrelevant to 'Bolt' type skills", "Earth"),
    ("lvl 7 Double Attack", "35% / 140%"),
    ("learning Magnus Exorcismus", "Divine Protection"),
    ("ability of the Bunny Band", "2 / LUK +2"),
    ("cannot equip Padded Armor", "Archer"),
    (
        "cures all abnormal status and restores full HP and SP",
        "Yggdrasilberry",
    ),
    ("rules the Rune-Midgarts kingdom", "Tristram the 3rd"),
    ("god of Crusaders", "Odin"),
    ("Morocc Jewel Merchant does not sell", "Garnet"),
    ("cannot purchase Monster's Feed", "Al De Baran"),
    ("closest city to the Maze", "Prontera"),
    ("different type than the others", "Penomena"),
    ("monster with the different attribute", "Giearth"),
    ("different in size", "Argos"),
    ("does not drop 'Yggdrasil Leaf'", "Marduk"),
    ("irrelevant to the Priest job change", "Sir Windsor"),
    ("not a citizen of Morocc", "Antonio"),
    ("gorgeous blue hair", "Pavianne"),
    ("learning Fire Wall", "lvl 4 Napalm Beat"),
    ("SP recovery at lvl 6", "18"),
    ("bonus at job lvl 33", "4"),
    ("Improve Concentration lvl 5", "45 / 80 sec"),
    ("learning Maximize Power", "Skin Tempering"),
    ("ability of Cute Ribbon", "1 / SP +20"),
    ("cannot equip Saint Robe", "Thief"),
    ("cannot be cured by Green Potion", "Curse"),
    ("disappeared somewhere in Geffen", "Geffenia"),
    ("root of this world", "Yggdrasil"),
    ("Magical Tool merchant in Geffen does not sell", "Mantle"),
    ("cannot purchase Blade", "Al De Baran"),
    ("closest city to Glast Heim", "Geffen"),
    ("different type than the others", "Marin"),
    ("monster that has a different attribute.", "Smokie"),
    ("monster that is different sized.", "Drake"),
    ("does not drop 'Phracon'", "Peco Peco Egg"),
    ("irrelevant to the Blacksmith job change", "Barcadi"),
    ("not a citizen of Al De Baran", "GOD-POING"),
    ("youngest among the staff", "Curly Sue"),
    ("Safety Wall lvl 6", "SP 40, 7 times"),
    ("Napalm Beat lvl 6", "MATK * 1.3"),
    ("Mage Solution no. 4", "1 carat Diamond"),
    ("Bash lvl 6", "280% / 15"),
    ("learning Claymore Trap", "Remove Trap"),
    ("ability of Wedding Veil", "0 / MDEF +5"),
    ("cannot equip Coat", "Novice"),
    ("ingredient for Blue Dyestuffs", "Karvodailnirol"),
    ("created by the god Odin", "The heart of Ymir"),
    ("bring fortune and fame", "Emperium"),
];

/// The Hunter Guild of Hugel and Payon (`2-1/hunter.txt`): quest window ids 4000 to 4013. Sherin's interview, the
/// receptionist's list, then the guildmaster's test: shoot four "Job Change Monster" and nothing else, press Escape on
/// the switch and leave by the portal.
fn hunter() -> Quest {
    const SHERIN: (&str, &str, u16, u16) = ("hu_in01", "Hunter Guildsman", 386, 373);
    const DEMON: (&str, &str, u16, u16) = ("hu_in01", "Guild Receptionist", 382, 382);
    const MASTER: (&str, &str, u16, u16) = ("payon_in03", "Hunter", 131, 7);
    const MASTER2: (&str, &str, u16, u16) = ("payon_in02", "Hunter", 21, 31);
    const ARENA: (u16, u16, u16, u16) = (50, 64, 129, 143);
    const BEGIN: &[&str] = &["Begin test", "Okay. Let's start", "Yes, let's start"];
    job_quest(
        "Hunter",
        &["Archer"],
        vec![
            talk(
                "apply to Sherin of the Hunter Guild",
                SHERIN.0,
                SHERIN.1,
                SHERIN.2,
                SHERIN.3,
            )
            .menu(&["Yes. That's what I'm here for", "Yes~ Let's start now"])
            .done(Check::Quest(4000)),
            talk(
                "answer Sherin's interview",
                SHERIN.0,
                SHERIN.1,
                SHERIN.2,
                SHERIN.3,
            )
            .answers(&[
                (
                    "don't know where you should go to hunt",
                    "Quietly ask a person passing by.",
                ),
                ("How do you go to the desert", "Use the Kafra service."),
                ("Zeny that you need", "Sell items I do not need."),
                ("solution to this situation", "Go back to town."),
                (
                    "How would you ask for a Heal",
                    "Would it be possible to get a Heal, please?",
                ),
                ("sell your item", "Look to see if anyone already wants it."),
                (
                    "begging for items and Zeny",
                    "Give some of my items and Zeny.",
                ),
                ("someone that is lost", "Guide them to their destination."),
                (
                    "attacking a boss",
                    "Watch, then attack when asked for help.",
                ),
                ("lying on the floor", "Try to find the owner."),
            ])
            .done(Check::Quest(4001))
            .log(&[4000]),
            talk(
                "ask the Guild Receptionist what the guild needs",
                DEMON.0,
                DEMON.1,
                DEMON.2,
                DEMON.3,
            )
            .menu(&["Yes, that is correct", "Yes..."])
            .assigns(&[
                &[(7030, 5), (1019, 5), (509, 3)],
                &[(925, 3), (932, 5), (511, 3)],
                &[(937, 3), (919, 3), (507, 5)],
                &[(1021, 3), (7032, 3), (914, 10)],
                &[(935, 9), (955, 9), (508, 9)],
                &[(913, 3), (938, 1), (948, 1)],
                &[(1027, 2), (942, 1), (1026, 1)],
            ])
            .done(Check::Quest(4002))
            .done(Check::Quest(4003))
            .done(Check::Quest(4004))
            .done(Check::Quest(4005))
            .done(Check::Quest(4006))
            .done(Check::Quest(4007))
            .done(Check::Quest(4008))
            .log(&[4001]),
            gather_assigned("bring the Guild Receptionist the items")
                .log(&[4002, 4003, 4004, 4005, 4006, 4007, 4008]),
            talk(
                "bring the Guild Receptionist the items",
                DEMON.0,
                DEMON.1,
                DEMON.2,
                DEMON.3,
            )
            .done(Check::Quest(4009))
            .done(Check::Quest(4010))
            .log(&[4002, 4003, 4004, 4005, 4006, 4007, 4008]),
            // The receptionist sends the bot to one of two places where the guildmaster is.
            talk(
                "ask the guildmaster in the Payon palace for the test",
                MASTER.0,
                MASTER.1,
                MASTER.2,
                MASTER.3,
            )
            .menu(BEGIN)
            .done(Check::OnMap("job_hunte"))
            .log(&[4009]),
            talk(
                "ask the guildmaster at the Archer Guild for the test",
                MASTER2.0,
                MASTER2.1,
                MASTER2.2,
                MASTER2.3,
            )
            .menu(BEGIN)
            .done(Check::OnMap("job_hunte"))
            .log(&[4010, 4011]),
            room("wait for the test", "job_hunte", "Waiting Room")
                .inside()
                .area(160, 10, 195, 50),
            // Shooting any other monster sends the bot back to the waiting room.
            fight("shoot four Job Change Monsters and no other", "job_hunte")
                .only("Job Change Monster")
                .area(ARENA.0, ARENA.1, ARENA.2, ARENA.3)
                .ahead(93, 101)
                .passed(Check::At("job_hunte", ARENA)),
            talk("press Escape on the switch", "job_hunte", "Switch", 93, 101)
                .menu(&["Escape"])
                .inside()
                .area(ARENA.0, ARENA.1, ARENA.2, ARENA.3)
                .said("Escape Warp Portal"),
            walk("leave by the escape portal", "job_hunte", 89, 139)
                .inside()
                .area(ARENA.0, ARENA.1, ARENA.2, ARENA.3)
                .done(Check::Quest(4012)),
            talk(
                "report to the guildmaster",
                MASTER.0,
                MASTER.1,
                MASTER.2,
                MASTER.3,
            )
            .done(Check::Quest(4013))
            .log(&[4012]),
            talk(
                "become a Hunter at Sherin",
                SHERIN.0,
                SHERIN.1,
                SHERIN.2,
                SHERIN.3,
            )
            .done(Check::Job("Hunter"))
            .log(&[4013]),
        ],
    )
}

/// The Prontera church (`2-1/priest.txt`): quest window ids 8009 to 8016. A pilgrimage to the three ascetics of the
/// Acolyte test, the halls of `job_prist` (slay the zombies, refuse the demons, walk past the mummies), then Sister
/// Cecilia's questions.
fn priest() -> Quest {
    const PAUL: (&str, &str, u16, u16) = ("prt_church", "High Bishop", 16, 41);
    const CECILIA: (&str, &str, u16, u16) = ("prt_church", "Sister Cecilia", 27, 24);
    const ZOMBIES: (u16, u16, u16, u16) = (8, 34, 39, 112);
    const DEMONS: (u16, u16, u16, u16) = (155, 10, 182, 185);
    const MUMMIES: (u16, u16, u16, u16) = (82, 35, 115, 110);
    job_quest(
        "Priest",
        &["Acolyte"],
        vec![
            talk("apply to Bishop Paul", PAUL.0, PAUL.1, PAUL.2, PAUL.3)
                .menu(&["I want to be a Priest", "Yes, I do"])
                .done(Check::Quest(8010))
                .done(Check::Quest(8011)),
            talk(
                "visit Father Rubalkabara on the pilgrimage",
                "prt_fild03",
                "Ascetic",
                365,
                255,
            )
            .said("next destination")
            .log(&[8010]),
            talk(
                "visit Mother Marthilda on the pilgrimage",
                "moc_fild07",
                "Ascetic",
                41,
                355,
            )
            .said("on your journey")
            .said("continue your training")
            .log(&[8010]),
            talk(
                "visit Father Yosuke on the pilgrimage",
                "prt_fild00",
                "Ascetic",
                208,
                218,
            )
            .said("completed")
            .said("live with me here")
            .log(&[8010]),
            talk(
                "ask Bishop Paul for the spiritual training",
                PAUL.0,
                PAUL.1,
                PAUL.2,
                PAUL.3,
            )
            .menu(&["I'm ready", "I'll try again", "I am ready"])
            .done(Check::OnMap("job_prist"))
            .log(&[8010, 8011, 8012]),
            talk(
                "ask Brother Peter to begin the training",
                "job_prist",
                "Peter S. Alberto",
                24,
                187,
            )
            .menu(&["Yes, I do", "I'm ready"])
            .inside()
            .area(10, 170, 40, 195)
            .done(Check::At("job_prist", ZOMBIES)),
            fight(
                "slay every zombie of the hall and leave by its far end",
                "job_prist",
            )
            .area(ZOMBIES.0, ZOMBIES.1, ZOMBIES.2, ZOMBIES.3)
            .ahead(24, 109)
            .passed(Check::At("job_prist", DEMONS)),
            walk("walk past the demons, refusing each", "job_prist", 168, 180)
                .menu(&[
                    "Out of my sight, demon!",
                    "Silence!",
                    "No deal... Doppelganger.",
                    "I'll never listen to you!",
                    "God will protect me.",
                    "Begone, vile fiend!",
                    "No, Baphomet. You lose.",
                ])
                .inside()
                .area(DEMONS.0, DEMONS.1, DEMONS.2, DEMONS.3)
                .done(Check::At("job_prist", MUMMIES)),
            walk("walk past the mummies to the way out", "job_prist", 98, 105)
                .inside()
                .area(MUMMIES.0, MUMMIES.1, MUMMIES.2, MUMMIES.3)
                .done(Check::Quest(8013)),
            // The script counts the other answer as wrong, whatever one might think of these.
            talk(
                "answer Sister Cecilia",
                CECILIA.0,
                CECILIA.1,
                CECILIA.2,
                CECILIA.3,
            )
            .answers(&[
                ("give your life to God", "Yes."),
                ("selfish, destructive", "No."),
                ("easing their suffering", "Yes."),
                ("sacrifice yourself", "Yes."),
                ("same phrase in public", "No."),
                ("lure many monsters", "No."),
                ("demands you to kill yourself", "Yes."),
                ("adhere to his teachings", "I do."),
            ])
            .done(Check::Quest(8015))
            .log(&[8013, 8014]),
            talk(
                "become a Priest at Bishop Paul",
                PAUL.0,
                PAUL.1,
                PAUL.2,
                PAUL.3,
            )
            .done(Check::Job("Priest"))
            .log(&[8015, 8016]),
        ],
    )
}

/// The St. Capitolina Abbey (`2-2/monk.txt`): quest window ids 3016 to 3032. Items for Sensei Moohae, a pledge to
/// say back to Touha, Boohae's questions, thirty mushrooms from the abbey grounds, a hall to cross, then the vows.
fn monk() -> Quest {
    const MOOHAE: (&str, &str, u16, u16) = ("monk_in", "Sensei Moohae", 99, 58);
    const TOUHA: (&str, &str, u16, u16) = ("prt_monk", "Touha", 251, 255);
    const TOMOON: (&str, &str, u16, u16) = ("monk_test", "Tomoon", 319, 139);
    const HALL: (u16, u16, u16, u16) = (118, 255, 175, 300);
    job_quest(
        "Monk",
        &["Acolyte"],
        vec![
            talk(
                "tell the Guarding Monk of the abbey you wish to become a monk",
                "prt_monk",
                "Guarding Monk",
                59,
                247,
            )
            .menu(&["Tell him", "I wish to become a monk"])
            .done(Check::Quest(3016)),
            talk(
                "ask Sensei Moohae what the abbey needs",
                MOOHAE.0,
                MOOHAE.1,
                MOOHAE.2,
                MOOHAE.3,
            )
            .menu(&["Yes."])
            .assigns(&[
                &[(938, 5), (1055, 10), (511, 20)],
                &[(942, 20), (1002, 5), (510, 3)],
                &[(905, 30), (909, 5), (955, 10)],
                &[(943, 5), (935, 20), (912, 5)],
                &[(7053, 5), (509, 10), (508, 10)],
                &[(913, 10), (948, 5), (7033, 20)],
                &[(1027, 5), (1025, 20), (1042, 10)],
            ])
            .done(Check::Quest(3017))
            .done(Check::Quest(3018))
            .done(Check::Quest(3019))
            .done(Check::Quest(3020))
            .done(Check::Quest(3021))
            .done(Check::Quest(3022))
            .done(Check::Quest(3023))
            .log(&[3016]),
            gather_assigned("bring Sensei Moohae the items")
                .log(&[3017, 3018, 3019, 3020, 3021, 3022, 3023]),
            talk(
                "bring Sensei Moohae the items",
                MOOHAE.0,
                MOOHAE.1,
                MOOHAE.2,
                MOOHAE.3,
            )
            .done(Check::Quest(3024))
            .log(&[3017, 3018, 3019, 3020, 3021, 3022, 3023]),
            // Touha says a pledge line by line, then asks for it back.
            talk(
                "say Touha's pledge back to him",
                TOUHA.0,
                TOUHA.1,
                TOUHA.2,
                TOUHA.3,
            )
            .menu(&["Yes."])
            .recite()
            .done(Check::Quest(3026))
            .log(&[3024, 3025]),
            talk("tell Boohae what Touha did", "prt_monk", "Boohae", 57, 179)
                .menu(&[
                    "...excuse me",
                    "Touha sent me",
                    "We recited a holy pledge",
                    "He modified my body",
                    "Gathering mushrooms",
                ])
                .done(Check::Quest(3027))
                .log(&[3026]),
            talk(
                "ask Bashu for the gathering test",
                "monk_test",
                "Bashu",
                329,
                61,
            )
            .menu(&["Tolerance - Gathering Mushrooms"])
            .done(Check::OnMap("job_monk"))
            .log(&[3027]),
            fight(
                "gather thirty mushrooms of one kind on the abbey grounds",
                "job_monk",
            )
            .only("Thief Mushroom")
            .until(Check::Count(1069, 30))
            .until(Check::Count(1070, 30))
            .passed(Check::Count(1069, 30))
            .done(Check::Count(1070, 30)),
            talk(
                "bring Hyunmoo the mushrooms",
                "job_monk",
                "Hyunmoo",
                225,
                180,
            )
            .inside()
            .done(Check::Quest(3029)),
            talk(
                "ask Tomoon for the last test",
                TOMOON.0,
                TOMOON.1,
                TOMOON.2,
                TOMOON.3,
            )
            .done(Check::Quest(3031))
            .log(&[3029]),
            talk(
                "ask the Proctor to open the hall",
                "monk_test",
                "Proctor",
                82,
                85,
            )
            .menu(&["Yes!"])
            .done(Check::At("monk_test", HALL))
            .log(&[3031]),
            walk("cross the hall to the way out", "monk_test", 166, 278)
                .inside()
                .area(HALL.0, HALL.1, HALL.2, HALL.3)
                .done(Check::OnMap("prt_monk")),
            talk(
                "tell Tomoon the hall is crossed",
                TOMOON.0,
                TOMOON.1,
                TOMOON.2,
                TOMOON.3,
            )
            .done(Check::Quest(3032))
            .log(&[3031]),
            // Sensei Moohae asks for the Green Potion Tomoon gives.
            gather("keep Tomoon's Green Potion", &[(506, 1)]).log(&[3032]),
            // The script counts the other answer as wrong.
            talk(
                "take the vows before Sensei Moohae",
                MOOHAE.0,
                MOOHAE.1,
                MOOHAE.2,
                MOOHAE.3,
            )
            .answers(&[
                ("pursuit of purity", "Yes."),
                ("for personal benefit", "No."),
                ("punishing those who are against", "Yes."),
                ("sacrifice yourself as a means to an end", "Yes."),
                ("gathering monsters to follow you", "No."),
                ("yell and shout the same things", "No."),
                ("die for others on your monk's path", "Yes."),
                ("make your oath", "I vow to keep these oaths."),
            ])
            .done(Check::Job("Monk"))
            .log(&[3032]),
        ],
    )
}

/// The Blacksmith Guild of Einbroch (`2-1/blacksmith.txt`): quest window ids 2000 to 2016. Geschupenschte's merchant
/// quiz, the parts of an order bought in shops, the weapon he makes from them delivered in another town, his receipt
/// shown, then Mitehmaeeuh's smithing quiz for the Hammer of Blacksmith.
fn blacksmith() -> Quest {
    const ALTIREGEN: (&str, &str, u16, u16) = ("ein_in01", "Guildsman", 18, 28);
    const GESCHUPENSCHTE: (&str, &str, u16, u16) = ("ein_in01", "Guildsman", 201, 27);
    const ORDERS: &[u32] = &[2003, 2004, 2005, 2006, 2007];
    const DELIVERIES: &[u32] = &[2008, 2009, 2010, 2011, 2012];
    job_quest(
        "Blacksmith",
        &["Merchant"],
        vec![
            talk(
                "apply to Altiregen of the Blacksmith Guild",
                ALTIREGEN.0,
                ALTIREGEN.1,
                ALTIREGEN.2,
                ALTIREGEN.3,
            )
            .menu(&["Fill out Application."])
            .done(Check::Quest(2000)),
            talk(
                "answer Geschupenschte's merchant questions",
                GESCHUPENSCHTE.0,
                GESCHUPENSCHTE.1,
                GESCHUPENSCHTE.2,
                GESCHUPENSCHTE.3,
            )
            .menu(&["Yes."])
            .answers(&[
                ("matched incorrectly", "Alberta - Swordmace"),
                ("not correctly matched", "Al De Baran - Hammer"),
                ("Hammer Fall", "Stun"),
                ("cannot be performed", "Increase AGI"),
                ("sells Blue Gemstones", "Geffen"),
                ("Tool Dealer", "8 o'clock direction from the town square"),
                ("Weapons Dealer", "5 o'clock from the town's center"),
                ("cannot be used by a Merchant", "Bible"),
                ("can a Merchant not use", "Claymore"),
                ("highest defense", "Mink Coat"),
                ("limit for upgrading", "up to + 5"),
                ("Trunks", "Sakkat"),
                ("Jellopy worth", "3 Zeny"),
                ("Vending Skill", "Must have a Cart."),
                ("become a Merchant", "Alberta"),
                ("NOT drop Iron Ore", "Anolian"),
                ("part of being a Merchant", "Credit"),
                ("important to a Merchant", "Credit"),
            ])
            .done(Check::Quest(2002))
            .log(&[2000, 2001]),
            talk(
                "take Geschupenschte's order",
                GESCHUPENSCHTE.0,
                GESCHUPENSCHTE.1,
                GESCHUPENSCHTE.2,
                GESCHUPENSCHTE.3,
            )
            .assigns(&[
                &[(999, 1), (930, 1), (717, 2), (1610, 1)],
                &[(1001, 2), (932, 1), (912, 1), (1219, 1)],
                &[(1003, 1), (935, 2), (990, 2), (1119, 1)],
                &[(1002, 1), (2212, 1), (717, 2), (1713, 1)],
                &[(998, 1), (511, 1), (919, 2), (1122, 1)],
            ])
            .done(Check::Quest(2003))
            .done(Check::Quest(2004))
            .done(Check::Quest(2005))
            .done(Check::Quest(2006))
            .done(Check::Quest(2007))
            .log(&[2002]),
            gather_assigned("bring Geschupenschte the parts of the order").log(ORDERS),
            talk(
                "bring Geschupenschte the parts of the order",
                GESCHUPENSCHTE.0,
                GESCHUPENSCHTE.1,
                GESCHUPENSCHTE.2,
                GESCHUPENSCHTE.3,
            )
            .menu(&["Oh, I brought what you asked for."])
            .done(Check::Quest(2008))
            .done(Check::Quest(2009))
            .done(Check::Quest(2010))
            .done(Check::Quest(2011))
            .done(Check::Quest(2012))
            .log(ORDERS),
            // He makes a weapon of the parts and sends it to one of five buyers, who pays with a receipt.
            talk(
                "deliver the Arc Wand to Baisulist in Geffen",
                "geffen",
                "Baisulist",
                46,
                164,
            )
            .menu(&["I'm sure! It's brand new!"])
            .done(Check::Item(1073))
            .log(&[2008])
            .branch("geffen"),
            talk(
                "deliver the Gladius to Wickebine in Morocc",
                "morocc",
                "Wickebine",
                27,
                112,
            )
            .menu(&["I guarantee you it is new."])
            .done(Check::Item(1073))
            .log(&[2009])
            .branch("morocc"),
            talk(
                "deliver the Tsurugi to Krongast in Lighthalzen",
                "lighthalzen",
                "Krongast",
                209,
                80,
            )
            .menu(&["It was just made, so it's new."])
            .done(Check::Item(1073))
            .log(&[2010])
            .branch("lighthalzen"),
            talk(
                "deliver the Arbalest to Talpiz in Payon",
                "payon",
                "Talpiz",
                214,
                79,
            )
            .menu(&["I'm sure it's new."])
            .done(Check::Item(1073))
            .log(&[2011])
            .branch("payon"),
            talk(
                "deliver the Ring Pommel Saber to Bismarc in Hugel",
                "hugel",
                "Bismarc",
                168,
                183,
            )
            .menu(&["I'm sure this is the one."])
            .done(Check::Item(1073))
            .log(&[2012])
            .branch("hugel"),
            talk(
                "show Geschupenschte the receipt",
                GESCHUPENSCHTE.0,
                GESCHUPENSCHTE.1,
                GESCHUPENSCHTE.2,
                GESCHUPENSCHTE.3,
            )
            .menu(&["Yes sir, here it is."])
            .done(Check::Quest(2013))
            .log(DELIVERIES),
            talk(
                "report to Altiregen",
                ALTIREGEN.0,
                ALTIREGEN.1,
                ALTIREGEN.2,
                ALTIREGEN.3,
            )
            .menu(&["I want to change my job quickly! But...oh well."])
            .done(Check::Quest(2014))
            .log(&[2013]),
            talk(
                "answer Mitehmaeeuh's smithing questions",
                "ein_in01",
                "Blacksmith Guildsman",
                24,
                41,
            )
            .menu(&["Yes"])
            .answers(&[
                ("required to learn the Discount", "Level 3 Enlarge Weight"),
                ("Hammerfall", "Stun"),
                ("mastered Mammonite", "1000 Zeny"),
                ("discount rate", "24 %"),
                ("overcharge items", "23 %"),
                ("drops Steel", "Skel Worker"),
                ("made from Red Bloods", "Flame Heart"),
                ("Kafra Storage", "Red Blood"),
                ("from a Wind attribute", "Water Property"),
                ("make 1 Steel", "5 Iron Ore"),
                ("meet someone randomly", "Ask them what they need."),
                ("Crazy Uproar", "Alberta"),
                ("direction is the Blacksmith Guild", "5 o'clock"),
                ("most Blacksmiths", "Einbroch"),
                ("affect your skill as a Blacksmith", "DEX"),
            ])
            .done(Check::Item(1005))
            .log(&[2014]),
            talk(
                "become a Blacksmith at Altiregen",
                ALTIREGEN.0,
                ALTIREGEN.1,
                ALTIREGEN.2,
                ALTIREGEN.3,
            )
            .done(Check::Job("Blacksmith"))
            .log(&[2015]),
        ],
    )
}

/// The Alchemist Union of Al De Baran (`2-2/alchemist.txt`): quest window ids 2028 to 2040. A fee and one of three
/// orders of items, Raspuchin's sums and prices, Darwin's herb mixing, Van Helmont's question on Morgenstein's
/// solutions (`quests/counteragent_mixture.txt`), Nicholas' word puzzles, then Bain and Bajin's experiment in Juno.
fn alchemist() -> Quest {
    const PARMY: (&str, &str, u16, u16) = ("alde_alche", "Alchemist Guildsman", 27, 185);
    const DARWIN: (&str, &str, u16, u16) = ("alde_alche", "Studying Man", 13, 15);
    const HELMONT: (&str, &str, u16, u16) = ("alde_alche", "Experiment Expert", 79, 19);
    const NICHOLAS: (&str, &str, u16, u16) = ("alde_alche", "Chief Researcher", 145, 19);
    // Christopher's workshop in Geffen sells Mini Furnaces by the number.
    const CHRISTOPHER: (&str, &str, u16, u16) = ("geffen_in", "Christopher", 110, 172);
    const FURNACE: &[&str] = &["Purchase Forging Item", "Mini-Furnace - 150 zeny"];
    const ORDERS: &[u32] = &[2028, 2029, 2030];
    job_quest(
        "Alchemist",
        &["Merchant"],
        vec![
            gather_zeny(
                "save the 50,000 zeny fee of the Alchemist Union",
                &[],
                50_000,
            ),
            talk(
                "apply to Parmy Gianino of the Alchemist Union",
                PARMY.0,
                PARMY.1,
                PARMY.2,
                PARMY.3,
            )
            .menu(&["I want to become an Alchemist.", "I would like to apply."])
            .done(Check::Quest(2028))
            .done(Check::Quest(2029))
            .done(Check::Quest(2030)),
            gather("bring Parmy 7 Berserk Potions", &[(657, 7)])
                .log(&[2028])
                .branch("potions"),
            gather_zeny("save for 100 Mini Furnaces", &[], 15_000)
                .log(&[2029])
                .branch("furnaces"),
            talk(
                "buy 100 Mini Furnaces from Christopher in Geffen",
                CHRISTOPHER.0,
                CHRISTOPHER.1,
                CHRISTOPHER.2,
                CHRISTOPHER.3,
            )
            .menu(FURNACE)
            .answers(&[("How many do ye wish", "100")])
            .done(Check::Count(612, 100))
            .log(&[2029])
            .branch("furnaces"),
            gather("bring Parmy 7,500 Fire Arrows", &[(1752, 7500)])
                .log(&[2030])
                .branch("arrows"),
            talk("bring Parmy the order", PARMY.0, PARMY.1, PARMY.2, PARMY.3)
                .done(Check::Quest(2031))
                .log(ORDERS),
            // Sums are worked out on the spot; prices, weights and defense are known.
            talk(
                "answer Raspuchin's questions",
                "alde_alche",
                "Fastidious Alchemist",
                175,
                107,
            )
            .answers(&[
                ("12 Red Potions, 1 Butterfly Wing", "909"),
                ("3 Scimiters, 2 Helms", "450"),
                ("Biretta, Mantle, Opera Mask", "20"),
                ("buy 5 Helms", "8800"),
                ("15 Green Potions, 6 Magnifiers", "934"),
                ("3 Ring Pommel Sabers, 4 Caps", "550"),
                ("Buckler, Long Coat, Gas Mask", "16"),
                ("buy Tights", "8520"),
                ("6 Red Potions, 7 Green Potions", "798"),
                ("2 Ring Pommel Sabers, 3 Caps", "480"),
                ("Mirror Shield, Mr. Smile", "12"),
                ("4 Padded Armors", "7680"),
            ])
            .done(Check::Quest(2032))
            .log(&[2031]),
            talk(
                "ask Darwin to teach experimenting",
                DARWIN.0,
                DARWIN.1,
                DARWIN.2,
                DARWIN.3,
            )
            .menu(&["I want to learn how to experiment."])
            .done(Check::Quest(2033))
            .log(&[2032]),
            talk(
                "buy 3 Medicine Bowls from the Guild Dealer",
                "alde_alche",
                "Guild Dealer",
                24,
                188,
            )
            .menu(&["Purchase materials.", "Medicine Bowl - 8 Zeny"])
            .answers(&[("How many do you want", "3")])
            .done(Check::Count(7134, 3))
            .log(&[2033]),
            gather(
                "bring Darwin bottles and herbs",
                &[(7134, 3), (713, 3), (507, 1), (508, 1), (509, 1)],
            )
            .log(&[2033]),
            talk(
                "mix the herbs with Darwin",
                DARWIN.0,
                DARWIN.1,
                DARWIN.2,
                DARWIN.3,
            )
            .menu(&[
                "Prepare the Medicine Bowl.",
                "Put some Herbs in the Medicine Bowl.",
                "Crush the Herbs.",
                "Pour clean water.",
                "Continue crushing the Herbs.",
                "Pour it in an empty bottle.",
            ])
            .done(Check::Quest(2035))
            .log(&[2033]),
            talk(
                "ask Van Helmont to teach something",
                HELMONT.0,
                HELMONT.1,
                HELMONT.2,
                HELMONT.3,
            )
            .menu(&["I want to become an Alchemist.", "Teach me something."])
            .done(Check::Quest(2036))
            .log(&[2035]),
            // Van Helmont sends the bot to watch Morgenstein, whom a merchant of Alberta and a sage of Geffen know of.
            gather("find a Detrimindexta to show around", &[(971, 1)]).log(&[2036]),
            talk(
                "show Louitz in Alberta the Detrimindexta",
                "alberta_in",
                "Louitz",
                133,
                55,
            )
            .menu(&["Talk about Detrimindexta", "Talk about the solutions."])
            .said("ask him about it")
            .log(&[2036]),
            talk(
                "ask Aure Dupon in Geffen about Morgenstein",
                "geffen",
                "Aure Dupon",
                181,
                114,
            )
            .menu(&["Ask About Morgenstein"])
            .said("tides of destiny carry you")
            .log(&[2036]),
            talk(
                "meet the chemist Morgenstein",
                "geffen_in",
                "Morgenstein",
                141,
                140,
            )
            .menu(&["Ask about his research."])
            .said("living person")
            .log(&[2036]),
            talk(
                "tell Van Helmont what Morgenstein's solutions need",
                HELMONT.0,
                HELMONT.1,
                HELMONT.2,
                HELMONT.3,
            )
            .answers(&[
                ("not necessary to make a Counteragent", "Karvodailnirol"),
                ("not necessary to make a Mixture", "Detrimindexta"),
            ])
            .done(Check::Quest(2037))
            .log(&[2036]),
            // The word puzzles are read off the letters; the second talk hands out the parts for Juno.
            talk(
                "solve Nicholas' word puzzles",
                NICHOLAS.0,
                NICHOLAS.1,
                NICHOLAS.2,
                NICHOLAS.3,
            )
            .times(2)
            .done(Check::Quest(2038))
            .log(&[2037]),
            gather_zeny("bring Bain coal and 2,000 zeny", &[(1003, 5)], 2_750).log(&[2038]),
            talk(
                "buy 5 Mini Furnaces from Christopher in Geffen",
                CHRISTOPHER.0,
                CHRISTOPHER.1,
                CHRISTOPHER.2,
                CHRISTOPHER.3,
            )
            .menu(FURNACE)
            .answers(&[("How many do ye wish", "5")])
            .done(Check::Count(612, 5))
            .log(&[2038]),
            talk(
                "help Bain with the experiment in Juno",
                "yuno_in01",
                "Bain",
                103,
                157,
            )
            .said("parts that I needed")
            .log(&[2038]),
            talk(
                "report to Nicholas",
                NICHOLAS.0,
                NICHOLAS.1,
                NICHOLAS.2,
                NICHOLAS.3,
            )
            .done(Check::Quest(2039))
            .log(&[2038]),
            talk(
                "become an Alchemist at the Master Alchemist",
                "alde_alche",
                "Master Alchemist",
                101,
                184,
            )
            .done(Check::Job("Alchemist"))
            .log(&[2039]),
        ],
    )
}

/// The Assassin Guild in the Sograt desert (`2-1/assassin.txt`): quest window ids 8000 to 8008, all on `in_moc_16`.
/// Khai takes the application, the Anonymous One asks ten questions, then three tests: kill only the six monsters
/// named "Job change target" in the arena, cross Thomas' hall to Barcardi without killing, find the way through the
/// maze (its guard sends one back twice), and the Guildmaster's talk.
fn assassin() -> Quest {
    const GUILD: &str = "in_moc_16";
    const GUILDSMAN: (&str, &str, u16, u16) = (GUILD, "Guildsman", 19, 33);
    const KHAI_ROOM: (u16, u16, u16, u16) = (5, 60, 40, 130);
    const HALL: (u16, u16, u16, u16) = (5, 140, 40, 185);
    const ARENA: (u16, u16, u16, u16) = (60, 136, 93, 177);
    const CORRIDOR: (u16, u16, u16, u16) = (60, 40, 115, 115);
    const MAZE: (u16, u16, u16, u16) = (170, 150, 199, 199);
    const MASTER_ROOM: (u16, u16, u16, u16) = (140, 70, 199, 118);
    job_quest(
        "Assassin",
        &["Thief"],
        vec![
            talk(
                "apply to the Assassin Guild",
                GUILDSMAN.0,
                GUILDSMAN.1,
                GUILDSMAN.2,
                GUILDSMAN.3,
            )
            .menu(&["Yes. I've picked my last pocket."])
            .done(Check::Quest(8001)),
            // Khai is spoken to by walking up to him; after a failed quiz he gives hints.
            walk("hand Khai the application", GUILD, 25, 90)
                .menu(&["Yes, I am.", "I beg you, give me hints.", "...Got you."])
                .inside()
                .area(KHAI_ROOM.0, KHAI_ROOM.1, KHAI_ROOM.2, KHAI_ROOM.3)
                .done(Check::Quest(8002))
                .log(&[8001]),
            walk("answer the Anonymous One", GUILD, 19, 154)
                .menu(&[
                    "You're all talk. I challenge you!",
                    "Hmpf, I know it all.",
                    "I challenge you again!",
                ])
                .answers(&[
                    (
                        "not required to learn Grimtooth",
                        "Right hand Mastery level 2",
                    ),
                    ("Enchant Poison possess", "Poison"),
                    (
                        "Level 4 Right Hand Mastery",
                        "Recover 90% of damage decrease",
                    ),
                    ("required for using Venom Dust", "Red Gemstone"),
                    ("Level 5 Enchant Poison", "Venom Dust"),
                    ("walk while invisible", "Cloaking"),
                    ("unrelated to Venom Splasher", "Red Gemstone."),
                    ("Vadon card", "Elder Willow"),
                    (
                        "Double Attack need",
                        "It's a passive skill, so SP use is 0.",
                    ),
                    ("hunting in Izlude dungeon", "Wind Main Gauche"),
                    ("slotted Katar", "Desert Wolf"),
                    ("slotted Jur", "Martin"),
                    ("craft elemental weapons", "Blacksmith"),
                    ("not in the Katar class", "Gladius"),
                    ("Izlude dungeon monsters posses", "Water"),
                    ("cannot be a Cute Pet", "Roda Frog"),
                    ("Fire property Daggers", "Hammer Goblin"),
                    ("non-elemental Katar", "Infiltrator"),
                    ("uncommon monster", "Ghostring"),
                    ("not Undead", "Spore"),
                    ("Increase Dodge", "30"),
                    ("detects hiding", "Andre"),
                    (
                        "cannot be used by an Assassin at once",
                        "Katar + Maingauche",
                    ),
                    ("Thieves can change their jobs", "Morocc"),
                    ("does not affect the AGI", "Whisper Card"),
                    ("specialty of the Assassin", "Excellent dodge ability"),
                    ("AGI bonus an Assassin", "10"),
                    ("Assassin cannot equip", "Helm"),
                    ("job change item for Thief", "Orange Gooey Mushroom"),
                    ("benefit an Assassin the least", "Elder Willow card"),
                ])
                .inside()
                .area(HALL.0, HALL.1, HALL.2, HALL.3)
                .done(Check::Quest(8003))
                .log(&[8002]),
            room(
                "wait in the Standby Room for the arena",
                GUILD,
                "Standby Room",
            )
            .inside()
            .area(HALL.0, HALL.1, HALL.2, HALL.3)
            .log(&[8003]),
            fight("kill only the six monsters named Job change target", GUILD)
                .only("Job change target")
                .inside()
                .area(ARENA.0, ARENA.1, ARENA.2, ARENA.3)
                .ahead(87, 137)
                .passed(Check::Quest(8004))
                .log(&[8003]),
            walk(
                "cross Thomas' hall to Barcardi without killing",
                GUILD,
                87,
                48,
            )
            .menu(&["I'm gonna try it again!"])
            .inside()
            .area(CORRIDOR.0, CORRIDOR.1, CORRIDOR.2, CORRIDOR.3)
            .done(Check::Quest(8005))
            .log(&[8004]),
            walk("find the way through the maze", GUILD, 182, 169)
                .inside()
                .area(MAZE.0, MAZE.1, MAZE.2, MAZE.3)
                .done(Check::At(GUILD, MASTER_ROOM))
                .log(&[8005, 8006]),
            // The Guildmaster's questions count for nothing; the weapon he offers is a Katar.
            walk("talk with the Guildmaster", GUILD, 149, 80)
                .menu(&[
                    "Endless practice.",
                    "For spiritual improvement.",
                    "Check how I can help as an Assassin.",
                    "Financial consulting.",
                    "Katar",
                ])
                .inside()
                .area(MASTER_ROOM.0, MASTER_ROOM.1, MASTER_ROOM.2, MASTER_ROOM.3)
                .done(Check::Quest(8007))
                .log(&[8006]),
            talk(
                "become an Assassin at the guild door",
                GUILDSMAN.0,
                GUILDSMAN.1,
                GUILDSMAN.2,
                GUILDSMAN.3,
            )
            .done(Check::Job("Assassin"))
            .log(&[8007]),
        ],
    )
}

/// The Rogue Guild under the Paros Lighthouse (`2-2/rogue.txt`): quest window ids 2017 to 2027. Markie's quiz, Mr.
/// Smith's fee and one of four orders of items, then one of three juniors met through a password door (or Hermanthorn
/// Jr. for the long order), and the walk back to the guild through a tunnel full of monsters.
fn rogue() -> Quest {
    const GUILD: &str = "in_rogue";
    const GUILDSMAN: (&str, &str, u16, u16) = (GUILD, "Rogue Guildsman", 363, 122);
    const SMITH: (&str, &str, u16, u16) = (GUILD, "Mr. Smith", 376, 23);
    const HALL: (u16, u16, u16, u16) = (330, 0, 399, 140);
    job_quest(
        "Rogue",
        &["Thief"],
        vec![
            talk(
                "answer Markie's questions at the Rogue Guild",
                GUILDSMAN.0,
                GUILDSMAN.1,
                GUILDSMAN.2,
                GUILDSMAN.3,
            )
            .menu(&["I'm ready."])
            .answers(&[
                ("necessary for learning Stalk", "Hiding"),
                ("how much more of a discount", "1 %"),
                ("skill, Mug", "Steal Zeny from monsters"),
                ("activate the skill, Slyness", "2 Rogues"),
                ("Level 5 Divest Helm", "Divest Shield"),
                ("move while hiding", "Stalk"),
                ("increases the accuracy", "Mummy Card."),
                ("Vadon card", "Elder Willow"),
                ("Double Attack require", "Passive skill, no SP required."),
                ("Byalan Dungeon", "Wind Main-Gauche"),
                ("slotted Gladius", "Kobold"),
                ("slotted Main-Gauche", "Hornet"),
                ("create unique potions", "Alchemist"),
                ("Rogues aren't allowed to use", "Katar"),
                ("Hode possesses", "Earth"),
                ("tamed for as a Cute Pet", "Creamy"),
                ("Dagger with the Fire property", "Hammer Goblin"),
                ("doesn't have any guild castles", "Alberta"),
                ("drops Blue Herbs", "Blue Plant"),
                ("not have the Undead property", "Familiar"),
                ("masters the Improve Dodge", "30"),
                ("detects a characters using the Hiding", "Argos"),
                ("change their jobs to Rogues", "Paros Lighthouse"),
                ("Novices change their jobs to Thieves", "Morocc"),
                ("does not affect the DEX", "Mummy Card"),
                ("cool about being a Rogue", "Excellent attack strength"),
                ("change jobs from Thief to Rogue", "At Job Level 40"),
                ("dye your hair blue", "Prontera, 7 o'clock"),
                ("required on the Thief job change", "Orange Gooey Mushroom"),
                ("least benefits the Rogue", "Elder Willow Card"),
            ])
            .done(Check::Quest(2017)),
            talk(
                "ask Mr. Smith what the guild wants",
                SMITH.0,
                SMITH.1,
                SMITH.2,
                SMITH.3,
            )
            .done(Check::Quest(2018))
            .done(Check::Quest(2019))
            .done(Check::Quest(2020))
            .done(Check::Quest(2021))
            .log(&[2017]),
            gather_zeny(
                "bring Mr. Smith the fee and his order",
                &[(932, 10), (510, 6), (957, 10), (958, 10)],
                10_000,
            )
            .log(&[2018]),
            gather_zeny(
                "bring Mr. Smith the fee and his order",
                &[(511, 10), (910, 10), (926, 10), (964, 10)],
                10_000,
            )
            .log(&[2019]),
            gather_zeny(
                "bring Mr. Smith the fee and his order",
                &[(508, 10), (948, 10), (935, 10), (940, 10)],
                10_000,
            )
            .log(&[2020]),
            gather_zeny(
                "bring Mr. Smith the fee and his long order",
                &[
                    (915, 5),
                    (713, 5),
                    (1002, 5),
                    (953, 5),
                    (507, 5),
                    (919, 5),
                    (715, 5),
                    (913, 5),
                    (904, 5),
                    (942, 5),
                    (528, 5),
                    (914, 5),
                    (705, 5),
                    (916, 5),
                    (917, 5),
                    (908, 5),
                    (945, 5),
                ],
                10_000,
            )
            .log(&[2021]),
            // Paid, he names a junior to train with and the password of the junior's door.
            talk(
                "pay Mr. Smith and learn who trains you",
                SMITH.0,
                SMITH.1,
                SMITH.2,
                SMITH.3,
            )
            .times(2)
            .done(Check::Quest(2022))
            .done(Check::Quest(2023))
            .done(Check::Quest(2024))
            .done(Check::Quest(2025))
            .log(&[2018, 2019, 2020, 2021]),
            walk(
                "say Aragham Junior's password at his door in the desert",
                "cmd_fild09",
                106,
                195,
            )
            .menu(&["Aragham", "never", "hoarded", "upgrade items."])
            .done(Check::OnMap(GUILD))
            .log(&[2022])
            .branch("junior"),
            talk(
                "ask Aragham Junior for the test",
                GUILD,
                "Aragham Junior",
                244,
                39,
            )
            .menu(&["Yes, let's go."])
            .inside()
            .done(Check::Quest(2026))
            .log(&[2022])
            .branch("junior"),
            walk(
                "say Antonio Junior's password at his door",
                "cmd_fild04",
                304,
                180,
            )
            .menu(&["Antonio", "doesn't enjoy", "destroying", "upgrade items."])
            .done(Check::OnMap(GUILD))
            .log(&[2023])
            .branch("junior"),
            talk(
                "ask Antonio Junior for the test",
                GUILD,
                "Antonio junior",
                177,
                109,
            )
            .menu(&["Let's go!"])
            .inside()
            .done(Check::Quest(2026))
            .log(&[2023])
            .branch("junior"),
            walk(
                "say Hollgrehenn Junior's password at his door in the desert",
                "cmd_fild09",
                335,
                143,
            )
            .menu(&["My father", "never", "hoarded", "upgrade items."])
            .done(Check::OnMap(GUILD))
            .log(&[2024])
            .branch("junior"),
            talk(
                "ask Hollgrehenn Junior for the test",
                GUILD,
                "Hollgrehenn junior",
                160,
                34,
            )
            .menu(&["Yes, I am."])
            .inside()
            .done(Check::Quest(2026))
            .log(&[2024])
            .branch("junior"),
            // The test: through the tunnel the junior opens, to its far end, which leads back to the guild hall.
            walk("walk through the tunnel back to the guild", GUILD, 370, 320)
                .inside()
                .done(Check::At(GUILD, HALL))
                .log(&[2026])
                .branch("junior"),
            talk(
                "visit Hermanthorn Jr. by the lighthouse checkpoint",
                GUILD,
                "Hermanthorn Jr",
                272,
                135,
            )
            .done(Check::Quest(2026))
            .log(&[2025])
            .branch("hermanthorn"),
            walk(
                "open the tunnel door with Hermanthorn's combination",
                GUILD,
                270,
                130,
            )
            .answers(&[("four number combination", "3019")])
            .done(Check::At(GUILD, (0, 0, 40, 60)))
            .log(&[2026])
            .branch("hermanthorn"),
            walk("walk through the tunnel back to the guild", GUILD, 9, 389)
                .inside()
                .done(Check::At(GUILD, HALL))
                .log(&[2026])
                .branch("hermanthorn"),
            talk(
                "become a Rogue at the Rogue Guildsman",
                GUILDSMAN.0,
                GUILDSMAN.1,
                GUILDSMAN.2,
                GUILDSMAN.3,
            )
            .done(Check::Job("Rogue"))
            .log(&[2026]),
        ],
    )
}

/// Lalo the wandering bard of Comodo (`2-2/bard.txt`): quest window ids 3000 to 3003. A flower he likes, then the
/// friendship of Snowysnow, the talking snowman of Lutie (`cities/lutie.txt`), earned by hearing his story from the
/// townsfolk one after another, then singing Lalo's song back to him line by line.
fn bard() -> Quest {
    const LALO: (&str, &str, u16, u16) = ("comodo", "Wandering Bard", 226, 123);
    const SNOWMAN: (&str, &str, u16, u16) = ("xmas", "Snowman", 134, 112);
    const AUNT: (&str, &str, u16, u16) = ("xmas_in", "Lenient Aunt", 27, 103);
    Quest {
        sex: Some("M"),
        ..job_quest(
            "Bard",
            &["Archer"],
            vec![
                talk(
                    "listen to Lalo's song in Comodo",
                    LALO.0,
                    LALO.1,
                    LALO.2,
                    LALO.3,
                )
                .menu(&["Could you sing for me, please?", "Yes, it was very nice."])
                .done(Check::Quest(3000)),
                talk(
                    "tell Lalo you would sing too",
                    LALO.0,
                    LALO.1,
                    LALO.2,
                    LALO.3,
                )
                .menu(&["You have a nice voice.", "Of course!"])
                .done(Check::Quest(3001))
                .log(&[3000]),
                gather("bring Lalo a flower he likes", &[(748, 1)]).log(&[3001]),
                talk(
                    "give Lalo the Witherless Rose",
                    LALO.0,
                    LALO.1,
                    LALO.2,
                    LALO.3,
                )
                .done(Check::Quest(3002))
                .log(&[3001]),
                // Snowysnow's story, from one townsman of Lutie to the next.
                gather(
                    "bring Uncle Hairy his ink and mucus",
                    &[(1024, 1), (938, 1)],
                )
                .log(&[3002]),
                talk("meet Duffle in Lutie", "xmas_in", "Duffle", 167, 173)
                    .said("talk to Snowysnow")
                    .log(&[3002]),
                talk(
                    "ask Snowysnow about himself",
                    SNOWMAN.0,
                    SNOWMAN.1,
                    SNOWMAN.2,
                    SNOWMAN.3,
                )
                .menu(&["Snowysnow?"])
                .said("fixated on Poze")
                .log(&[3002]),
                talk("visit Poze", "xmas", "Poze", 117, 304)
                    .said("For the actual details")
                    .log(&[3002]),
                talk("hear Uncle Hairy Cantata", "xmas", "Uncle Hairy", 176, 236)
                    .said("that lovely pickle maker")
                    .log(&[3002]),
                talk("visit Aunt Thachentze", AUNT.0, AUNT.1, AUNT.2, AUNT.3)
                    .said("Now be a dear")
                    .log(&[3002]),
                talk(
                    "fetch the salt from Snowysnow",
                    SNOWMAN.0,
                    SNOWMAN.1,
                    SNOWMAN.2,
                    SNOWMAN.3,
                )
                .menu(&["Snowysnow?"])
                .said("deliver that salt")
                .log(&[3002]),
                talk(
                    "bring Aunt Thachentze the salt",
                    AUNT.0,
                    AUNT.1,
                    AUNT.2,
                    AUNT.3,
                )
                .said("You gave her the")
                .log(&[3002]),
                talk(
                    "ask Hashokii the clown about Snowysnow",
                    "xmas",
                    "Hashokii",
                    146,
                    136,
                )
                .menu(&["About Snowysnow"])
                .said("go meet those 2 children")
                .log(&[3002]),
                talk("hear the Little Girl", "xmas", "Little Girl", 208, 168)
                    .said("become his friend")
                    .log(&[3002]),
                talk(
                    "become Snowysnow's friend",
                    SNOWMAN.0,
                    SNOWMAN.1,
                    SNOWMAN.2,
                    SNOWMAN.3,
                )
                .menu(&["Snowysnow?"])
                .said("small present")
                .log(&[3002]),
                // He sings, colouring how far he got; the bot sings each line back.
                talk(
                    "sing Lalo's song back to him",
                    LALO.0,
                    LALO.1,
                    LALO.2,
                    LALO.3,
                )
                .menu(&[
                    "Just change my job please.",
                    "Yes, just change my job already.",
                ])
                .recite()
                .done(Check::Job("Bard"))
                .log(&[3002, 3003]),
            ],
        )
    }
}

/// The Comodo dance school (`2-2/dancer.txt`): quest window ids 7000 to 7006 on `job_duncer`. Aile's fee and one of three
/// orders, Bijou's questions, then a dance to the calls announced on the stage.
fn dancer() -> Quest {
    const SCHOOL: &str = "job_duncer";
    const AILE: (&str, &str, u16, u16) = (SCHOOL, "Aile", 43, 93);
    const BIJOU: (&str, &str, u16, u16) = (SCHOOL, "Bijou", 95, 93);
    const STAGE: (u16, u16, u16, u16) = (60, 100, 80, 120);
    Quest {
        sex: Some("F"),
        ..job_quest(
            "Dancer",
            &["Archer"],
            vec![
                talk(
                    "apply to Aile of the Comodo dance school",
                    AILE.0,
                    AILE.1,
                    AILE.2,
                    AILE.3,
                )
                .menu(&["Fill out the application."])
                .done(Check::Quest(7000)),
                talk(
                    "ask Aile what the school needs",
                    AILE.0,
                    AILE.1,
                    AILE.2,
                    AILE.3,
                )
                .done(Check::Quest(7001))
                .done(Check::Quest(7002))
                .done(Check::Quest(7003))
                .log(&[7000]),
                gather_zeny(
                    "bring Aile the tuition and her order",
                    &[(938, 20), (909, 3), (501, 5), (2403, 1)],
                    10_000,
                )
                .log(&[7001]),
                gather_zeny(
                    "bring Aile the tuition and her order",
                    &[(1055, 5), (2405, 1)],
                    10_000,
                )
                .log(&[7002]),
                gather_zeny(
                    "bring Aile the tuition and her order",
                    &[(965, 2), (503, 5), (909, 20), (1020, 10), (2401, 1)],
                    10_000,
                )
                .log(&[7003]),
                talk("pay Aile", AILE.0, AILE.1, AILE.2, AILE.3)
                    .done(Check::Quest(7004))
                    .log(&[7001, 7002, 7003]),
                talk(
                    "answer Bijou's questions",
                    BIJOU.0,
                    BIJOU.1,
                    BIJOU.2,
                    BIJOU.3,
                )
                .answers(&[
                    ("Lady Luck", "Critical Attack Rate"),
                    ("not consider to be a dance", "Lightning Bolt"),
                    ("best describes a Dancer", "A person who dances."),
                    ("cannot be associated with Comodo", "A lot of Thieves."),
                    ("NorthEast of Pharoah's Lighthouse", "Comodo Beach"),
                    ("most beautiful dancer", "Bijou"),
                    ("most graceful dancer", "Bijou"),
                    ("dances most beautifully", "Bijou"),
                    ("perform together with a Dancer", "Bard"),
                    ("not a specialty of Comodo", "Shining Stone"),
                    ("manager of the Comodo Casino", "Moo"),
                    ("Dancer job change applications", "Aile"),
                    ("Mental Sensing", "Increases experience."),
                    ("bad etiquette", "Criticize your partner."),
                    ("a mistake while you are dancing", "Point out the mistake."),
                    ("change jobs to a Dancer", "Comodo"),
                    ("directly connected to Comodo", "3"),
                    ("not a Cute Pet monster", "Argiope"),
                    ("Kafra in Comodo", "Kafra Headquarters"),
                    ("What is my name", "Bijou"),
                    ("effect of Lullaby", "Casts the Sleep effect on the area."),
                    ("taps the ground", "Tap Dance"),
                    (
                        "not a characteristic of a Dancer",
                        "Uses Two-handed swords.",
                    ),
                    ("the most Dancers", "Comodo"),
                    ("better at than the other job classes", "Dancing"),
                    ("cannot be equipped by a Dancer", "Two-handed Sword"),
                    ("Do you think you", "Yes"),
                    ("not a Jazz musician", "Elder Willow"),
                ])
                .said("Now you only have the Dance Test")
                .said("I'll let you pass")
                .log(&[7004]),
                talk(
                    "ask Bijou for the dance test",
                    BIJOU.0,
                    BIJOU.1,
                    BIJOU.2,
                    BIJOU.3,
                )
                .menu(&["Go to the testing area."])
                .done(Check::Quest(7005))
                .log(&[7004]),
                room(
                    "wait for the stage in the waiting room",
                    SCHOOL,
                    "Waiting Room",
                )
                .log(&[7005]),
                dance(
                    "dance to the calls on the stage",
                    Dance {
                        map: SCHOOL,
                        cells: &[
                            ("up", (69, 113)),
                            ("down", (69, 107)),
                            ("left", (66, 110)),
                            ("right", (72, 110)),
                            ("center", (69, 110)),
                        ],
                        start: "let's begin",
                        finish: "finish",
                        end: (69, 110),
                    },
                )
                .area(STAGE.0, STAGE.1, STAGE.2, STAGE.3)
                .passed(Check::Quest(7006))
                .log(&[7005]),
                talk(
                    "become a Dancer at Bijou",
                    BIJOU.0,
                    BIJOU.1,
                    BIJOU.2,
                    BIJOU.3,
                )
                .done(Check::Job("Dancer"))
                .log(&[7006]),
            ],
        )
    }
}
