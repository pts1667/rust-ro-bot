//! The first-job quests of the guilds, as the scripts this server runs carry them out (rathena `npc/pre-re/jobs/1-1`,
//! the Mage's solvents in `npc/quests/mage_solution.txt`). A Novice needs job level 10 and Basic Skill 9 for each.

use super::build::*;
use super::{Check, Quest, Reward, Stage, Trap};

pub fn quests() -> Vec<Quest> {
    vec![swordman(), mage(), archer(), acolyte(), merchant(), thief()]
}

fn first_job(job: &'static str, stages: Vec<Stage>) -> Quest {
    Quest {
        id: Box::leak(format!("job:{job}").into_boxed_str()),
        title: Box::leak(format!("{job} job quest").into_boxed_str()),
        reward: Reward::Job(job),
        jobs: &["Novice"],
        job_level: 10,
        base_level: 1,
        sex: None,
        reborn: Some(false),
        past: &[],
        no_cart: false,
        skills: &[("NV_BASIC", 9)],
        stages,
    }
}

/// The Swordman Association of Izlude (`swordman.txt`): an obstacle course of three halls on `job_sword1`, each
/// crossed without stepping into a pit, then the Swordman who signed the bot up. Quest window: 1014.
fn swordman() -> Quest {
    const SWORDMAN: (&str, &str, u16, u16) = ("izlude_in", "Swordman", 74, 172);
    const MAP: &str = "job_sword1";
    // The waiting room of the test hall, which only the Swordman at the door leads into.
    const ROOM: (u16, u16, u16, u16) = (20, 155, 50, 185);
    const COURSE_1: (u16, u16, u16, u16) = (9, 230, 194, 256);
    const REST_1: (u16, u16, u16, u16) = (195, 230, 250, 256);
    const COURSE_2: (u16, u16, u16, u16) = (9, 195, 194, 218);
    const REST_2: (u16, u16, u16, u16) = (195, 195, 250, 218);
    const COURSE_3: (u16, u16, u16, u16) = (9, 155, 194, 180);
    const REST_3: (u16, u16, u16, u16) = (195, 155, 250, 180);
    const PITS: (u16, u16, u16, u16) = (0, 0, 255, 125);
    // A course is crossed by asking the staff at its start whether to quit: that one is avoided too.
    const NO: &[&str] = &["No."];
    // The stage after Mae's, when the guild's Swordman changes the job.
    const PASSED: usize = 10;
    first_job(
        "Swordman",
        vec![
            talk(
                "sign up at the Swordman Association",
                SWORDMAN.0,
                SWORDMAN.1,
                SWORDMAN.2,
                SWORDMAN.3,
            )
            .menu(&["Job Change", "Sign up.", "Yes."])
            .done(Check::Quest(1014)),
            talk("go into the test hall", "izlude_in", "Swordman", 62, 170)
                .done(Check::At("izlude_in", ROOM))
                // The course is passed: the Swordman of the guild is next
                .back_if(
                    "talk to the Swordman in the center of the room, not me",
                    PASSED,
                )
                .log(&[1014]),
            talk(
                "ask the Test Hall Staff to start the test",
                "izlude_in",
                "Test Hall Staff",
                30,
                163,
            )
            .inside()
            .area(ROOM.0, ROOM.1, ROOM.2, ROOM.3)
            .done(Check::OnMap(MAP))
            .log(&[1014]),
            walk("cross the first hall, keeping off the pits", MAP, 192, 244)
                .avoid(SWORD_TRAPS)
                .menu(NO)
                .inside()
                .area(COURSE_1.0, COURSE_1.1, COURSE_1.2, COURSE_1.3)
                .done(Check::At(MAP, REST_1))
                .log(&[1014]),
            walk("step onto the way to the second hall", MAP, 223, 243)
                .avoid(SWORD_TRAPS)
                .inside()
                .area(REST_1.0, REST_1.1, REST_1.2, REST_1.3)
                .done(Check::At(MAP, COURSE_2))
                .log(&[1014]),
            walk("cross the second hall, keeping off the pits", MAP, 193, 207)
                .avoid(SWORD_TRAPS)
                .menu(NO)
                .inside()
                .area(COURSE_2.0, COURSE_2.1, COURSE_2.2, COURSE_2.3)
                .done(Check::At(MAP, REST_2))
                .log(&[1014]),
            walk("step onto the way to the third hall", MAP, 223, 205)
                .avoid(SWORD_TRAPS)
                .inside()
                .area(REST_2.0, REST_2.1, REST_2.2, REST_2.3)
                .done(Check::At(MAP, COURSE_3))
                .log(&[1014]),
            walk("cross the third hall, keeping off the pits", MAP, 193, 168)
                .avoid(SWORD_TRAPS)
                .menu(NO)
                .inside()
                .area(COURSE_3.0, COURSE_3.1, COURSE_3.2, COURSE_3.3)
                .done(Check::At(MAP, REST_3))
                .log(&[1014]),
            // Fallen into a pit: its warp leads back to the start of the hall. Before Mae, so that once she sends the bot out
            // the stage after hers is the last.
            walk("climb out of the pit", MAP, 65, 117)
                .avoid(SWORD_TRAPS)
                .inside()
                .area(PITS.0, PITS.1, PITS.2, PITS.3)
                .log(&[1014]),
            talk("tell Mae the course is done", MAP, "Mae", 223, 167)
                .inside()
                .area(REST_3.0, REST_3.1, REST_3.2, REST_3.3)
                .done(Check::OnMap("izlude_in"))
                .log(&[1014]),
            talk(
                "become a Swordman",
                SWORDMAN.0,
                SWORDMAN.1,
                SWORDMAN.2,
                SWORDMAN.3,
            )
            .menu(&["Job Change"])
            .done(Check::Job("Swordman"))
            .back_if("didn't take the test yet", 1)
            .log(&[1014]),
        ],
    )
}

/// The pits of the three halls of `job_sword1`, and the staff at the start of each that asks whether to quit.
const SWORD_TRAPS: &[Trap] = &[
    (7, 245, 1, 3),
    (8, 207, 1, 6),
    (8, 169, 1, 6),
    (16, 251, 0, 1),
    (19, 251, 0, 1),
    (17, 250, 1, 0),
    (16, 238, 0, 1),
    (19, 238, 0, 1),
    (17, 239, 0, 1),
    (28, 247, 4, 0),
    (33, 245, 0, 2),
    (29, 242, 4, 0),
    (24, 244, 0, 2),
    (38, 251, 0, 1),
    (41, 251, 0, 1),
    (39, 250, 1, 0),
    (38, 238, 0, 1),
    (41, 238, 0, 1),
    (39, 239, 1, 0),
    (54, 251, 0, 1),
    (71, 251, 0, 1),
    (62, 250, 9, 0),
    (62, 247, 8, 0),
    (71, 244, 0, 2),
    (63, 242, 8, 0),
    (54, 244, 0, 2),
    (54, 238, 0, 1),
    (71, 238, 0, 1),
    (62, 239, 9, 0),
    (102, 247, 2, 0),
    (105, 245, 0, 2),
    (103, 242, 2, 0),
    (100, 244, 0, 2),
    (156, 249, 14, 0),
    (156, 248, 14, 0),
    (170, 249, 1, 0),
    (170, 248, 1, 0),
    (156, 245, 14, 0),
    (156, 244, 14, 0),
    (170, 245, 1, 0),
    (170, 244, 1, 0),
    (156, 241, 14, 0),
    (156, 240, 14, 0),
    (170, 241, 1, 0),
    (170, 240, 1, 0),
    (180, 251, 0, 1),
    (183, 251, 0, 1),
    (181, 250, 1, 0),
    (180, 238, 0, 1),
    (183, 238, 0, 1),
    (181, 239, 1, 0),
    (56, 212, 40, 0),
    (95, 212, 2, 0),
    (56, 210, 40, 0),
    (95, 210, 2, 0),
    (16, 206, 0, 3),
    (97, 206, 0, 3),
    (56, 203, 40, 0),
    (95, 203, 2, 0),
    (56, 201, 40, 0),
    (95, 201, 2, 0),
    (113, 212, 14, 0),
    (125, 212, 2, 0),
    (113, 210, 14, 0),
    (125, 210, 2, 0),
    (100, 206, 0, 3),
    (127, 206, 0, 3),
    (113, 203, 14, 0),
    (125, 210, 2, 0),
    (113, 201, 14, 0),
    (113, 201, 2, 0),
    (155, 212, 21, 0),
    (181, 212, 2, 0),
    (155, 210, 21, 0),
    (181, 210, 2, 0),
    (130, 206, 0, 3),
    (183, 206, 0, 3),
    (155, 203, 21, 0),
    (181, 203, 2, 0),
    (155, 201, 40, 0),
    (181, 201, 2, 0),
    (17, 174, 2, 0),
    (17, 163, 2, 0),
    (29, 171, 2, 0),
    (31, 168, 0, 2),
    (28, 166, 2, 0),
    (26, 168, 0, 2),
    (36, 169, 0, 0),
    (37, 169, 0, 0),
    (37, 168, 0, 0),
    (36, 168, 0, 0),
    (40, 175, 0, 1),
    (41, 175, 0, 1),
    (41, 171, 1, 0),
    (41, 170, 1, 0),
    (41, 167, 1, 0),
    (41, 166, 1, 0),
    (42, 169, 0, 1),
    (43, 170, 0, 1),
    (43, 167, 0, 1),
    (40, 162, 0, 1),
    (41, 162, 0, 1),
    (46, 175, 0, 1),
    (51, 175, 0, 1),
    (47, 174, 1, 0),
    (50, 174, 1, 0),
    (48, 173, 0, 1),
    (49, 173, 0, 1),
    (46, 162, 0, 1),
    (51, 162, 0, 1),
    (47, 163, 1, 0),
    (50, 163, 1, 0),
    (48, 164, 0, 1),
    (49, 164, 0, 1),
    (54, 170, 0, 1),
    (55, 170, 0, 1),
    (54, 167, 0, 1),
    (55, 167, 0, 1),
    (53, 169, 1, 0),
    (53, 168, 1, 0),
    (56, 169, 1, 0),
    (56, 168, 1, 0),
    (58, 175, 0, 1),
    (59, 174, 1, 0),
    (60, 173, 0, 1),
    (61, 172, 1, 0),
    (58, 162, 0, 1),
    (59, 163, 1, 0),
    (60, 164, 0, 1),
    (61, 165, 1, 0),
    (76, 172, 1, 0),
    (77, 173, 0, 1),
    (78, 174, 1, 0),
    (79, 175, 0, 1),
    (76, 165, 1, 0),
    (77, 164, 0, 1),
    (78, 163, 1, 0),
    (79, 162, 0, 1),
    (94, 175, 0, 1),
    (95, 174, 1, 0),
    (98, 174, 1, 0),
    (99, 175, 0, 1),
    (96, 169, 0, 0),
    (97, 169, 0, 0),
    (97, 168, 0, 0),
    (96, 168, 0, 0),
    (94, 162, 0, 1),
    (95, 163, 1, 0),
    (98, 163, 1, 0),
    (99, 162, 0, 1),
    (114, 175, 0, 1),
    (115, 175, 0, 1),
    (114, 162, 0, 1),
    (115, 162, 0, 1),
    (126, 175, 0, 1),
    (127, 175, 0, 1),
    (126, 162, 0, 1),
    (127, 162, 0, 1),
    (160, 174, 0, 2),
    (161, 174, 0, 2),
    (160, 163, 0, 2),
    (161, 163, 0, 2),
    (168, 175, 0, 2),
    (169, 175, 0, 2),
    (168, 162, 0, 2),
    (169, 162, 0, 2),
    (176, 174, 0, 2),
    (177, 174, 0, 2),
    (178, 173, 1, 0),
    (178, 172, 1, 0),
    (181, 174, 2, 0),
    (179, 169, 3, 0),
    (179, 168, 3, 0),
    (182, 169, 0, 2),
    (183, 169, 0, 2),
    (181, 167, 1, 0),
    (181, 166, 1, 0),
    (183, 167, 0, 1),
    (176, 163, 0, 2),
    (177, 163, 0, 2),
    (181, 163, 2, 0),
];

/// The Mage Guild of Geffen (`mage.txt`): the Guildsman hands out one of four solutions to mix, which the quest
/// window shows (1005 to 1008) once the bot asks for the requirements. The Mixing Machine is worked step by step;
/// its catalyst comes with the machine. The solvents are sold at springs for 50 zeny and the Empty Test Tube.
fn mage() -> Quest {
    const GUILDSMAN: (&str, &str, u16, u16) = ("geffen_in", "Mage Guildsman", 164, 124);
    const MACHINE: (&str, &str, u16, u16) = ("geffen_in", "Mixing Machine", 164, 112);
    const TUBE: u32 = 1092;
    const PAYON_SOLUTION: u32 = 1089;
    const MOROCC_SOLUTION: u32 = 1088;
    let payon = |log: &'static [u32], branch: &'static str| {
        talk(
            "buy Payon Solution at the spring",
            "pay_arche",
            "Dollshoi",
            122,
            100,
        )
        .menu(&["Alright, Deal."])
        .done(Check::Item(PAYON_SOLUTION))
        .log(log)
        .branch(branch)
    };
    let mix =
        |steps: &'static [&'static str], made: u32, log: &'static [u32], branch: &'static str| {
            talk(
                "mix the solution at the Mixing Machine",
                MACHINE.0,
                MACHINE.1,
                MACHINE.2,
                MACHINE.3,
            )
            .steps(steps)
            .done(Check::Item(made))
            .log(log)
            .branch(branch)
        };
    first_job(
        "Mage",
        vec![
            talk(
                "sign up at the Mage Guild",
                GUILDSMAN.0,
                GUILDSMAN.1,
                GUILDSMAN.2,
                GUILDSMAN.3,
            )
            .menu(&["I want to be a Mage.", "Sign Up."])
            .done(Check::Item(TUBE)),
            talk(
                "ask the Mage Guildsman which solution to mix",
                GUILDSMAN.0,
                GUILDSMAN.1,
                GUILDSMAN.2,
                GUILDSMAN.3,
            )
            .menu(&["Tell me the Requirements."])
            .done(Check::Quest(1005))
            .done(Check::Quest(1006))
            .done(Check::Quest(1007))
            .done(Check::Quest(1008)),
            gather(
                "gather 2 Jellopy, 3 Fluff and 1 Milk",
                &[(909, 2), (914, 3), (519, 1)],
            )
            .log(&[1005])
            .branch("1"),
            payon(&[1005], "1"),
            mix(
                &[
                    "Use Machine.",
                    "Payon Solution.",
                    "Jellopy.",
                    "2",
                    "Fluff.",
                    "3",
                    "Milk.",
                    "1",
                    "Ready to Mix.",
                    "Begin Mixing.",
                    "Press 'Mix' Button.",
                    "8472",
                    "Confirm.",
                    "Yellow Gemstone.",
                ],
                1071,
                &[1005],
                "1",
            ),
            gather(
                "gather 3 Jellopy, 1 Fluff and 1 Milk",
                &[(909, 3), (914, 1), (519, 1)],
            )
            .log(&[1006])
            .branch("2"),
            mix(
                &[
                    "Use Machine.",
                    "No Solvent.",
                    "Jellopy.",
                    "3",
                    "Fluff.",
                    "1",
                    "Milk.",
                    "1",
                    "Ready to Mix.",
                    "Begin Mixing.",
                    "Press 'Mix' Button.",
                    "3735",
                    "Confirm.",
                    "Red Gemstone.",
                ],
                1085,
                &[1006],
                "2",
            ),
            gather("gather 6 Jellopy and 1 Fluff", &[(909, 6), (914, 1)])
                .log(&[1007])
                .branch("3"),
            payon(&[1007], "3"),
            mix(
                &[
                    "Use Machine.",
                    "Payon Solution.",
                    "Jellopy.",
                    "6",
                    "Fluff.",
                    "1",
                    "Ready to Mix.",
                    "Begin Mixing.",
                    "Press 'Mix' Button.",
                    "2750",
                    "Confirm.",
                    "Blue Gemstone.",
                ],
                1086,
                &[1007],
                "3",
            ),
            gather("gather 2 Jellopy and 3 Fluff", &[(909, 2), (914, 3)])
                .log(&[1008])
                .branch("4"),
            talk(
                "buy Morocc Solution at the spring",
                "moc_ruins",
                "Ponka-Hontas",
                91,
                150,
            )
            .menu(&["Alright, Deal."])
            .done(Check::Item(MOROCC_SOLUTION))
            .log(&[1008])
            .branch("4"),
            mix(
                &[
                    "Use Machine.",
                    "Morocc Solution.",
                    "Jellopy.",
                    "2",
                    "Fluff.",
                    "3",
                    "Ready to Mix.",
                    "Begin Mixing.",
                    "Press 'Mix' Button.",
                    "5429",
                    "Confirm.",
                    "1carat Diamond.",
                ],
                1087,
                &[1008],
                "4",
            ),
            talk(
                "bring the solution to the Mage Guildsman",
                GUILDSMAN.0,
                GUILDSMAN.1,
                GUILDSMAN.2,
                GUILDSMAN.3,
            )
            .menu(&["I want to be a Mage."])
            .done(Check::Job("Mage"))
            .log(&[1005, 1006, 1007, 1008]),
        ],
    )
}

/// The Archer Guild of Payon (`archer.txt`): Trunks graded 25 points or more. Willows drop the Trunks (1 point
/// each); the better woods are worth more. Quest window: 1004.
fn archer() -> Quest {
    const GUILDSMAN: (&str, &str, u16, u16) = ("payon_in02", "Archer Guildsman", 64, 71);
    const MENU: &[&str] = &["I want to be an Archer.", "Apply.", "Yes, I am."];
    first_job(
        "Archer",
        vec![
            talk(
                "apply at the Archer Guild",
                GUILDSMAN.0,
                GUILDSMAN.1,
                GUILDSMAN.2,
                GUILDSMAN.3,
            )
            .menu(MENU)
            .done(Check::Quest(1004))
            .done(Check::Job("Archer")),
            gather("gather 25 Trunks", &[(1019, 25)]).log(&[1004]),
            talk(
                "bring the Trunks to the Archer Guildsman",
                GUILDSMAN.0,
                GUILDSMAN.1,
                GUILDSMAN.2,
                GUILDSMAN.3,
            )
            .menu(MENU)
            .done(Check::Job("Archer"))
            .log(&[1004]),
        ],
    )
}

/// The Prontera Church (`acolyte.txt`): a pilgrimage to one of three ascetics, which the quest window shows (1001
/// to 1003). Each ascetic sets the bot's save point by them.
fn acolyte() -> Quest {
    const CLERIC: (&str, &str, u16, u16) = ("prt_church", "Cleric", 184, 41);
    const MENU: &[&str] = &["Father, I want to be a Acolyte.", "Yes Father, I do."];
    first_job(
        "Acolyte",
        vec![
            talk(
                "ask the Cleric to become an Acolyte",
                CLERIC.0,
                CLERIC.1,
                CLERIC.2,
                CLERIC.3,
            )
            .menu(MENU)
            .done(Check::Quest(1001))
            .done(Check::Quest(1002))
            .done(Check::Quest(1003)),
            talk(
                "visit Father Rubalkabara",
                "prt_fild03",
                "Ascetic",
                365,
                255,
            )
            .said("return to the Prontera Sanctuary")
            .said("carrier pigeon")
            .log(&[1001])
            .branch("rubalkabara"),
            talk("visit Mother Mathilda", "moc_fild07", "Ascetic", 41, 355)
                .said("return to the Prontera Sanctuary")
                .said("carrier pigeon")
                .log(&[1002])
                .branch("mathilda"),
            talk("visit Father Yosuke", "prt_fild00", "Ascetic", 208, 218)
                .said("go back to the Santuary")
                .said("Go back to the Sanctuary now")
                .log(&[1003])
                .branch("yosuke"),
            talk(
                "become an Acolyte at the Cleric",
                CLERIC.0,
                CLERIC.1,
                CLERIC.2,
                CLERIC.3,
            )
            .menu(MENU)
            .done(Check::Job("Acolyte"))
            .log(&[1001, 1002, 1003]),
        ],
    )
}

/// Serial numbers Chief Mahnsoo gives, two for each destination.
const SERIALS: &[&str] = &[
    "2485741", "2328137", "2989396", "2191737", "3012685", "3487372", "3318702", "3543625",
];

/// The Merchant Guild of Alberta (`merchant.txt`): the fee, then a package to take from the storehouse (told by
/// its serial number) to one of four places, which the quest window shows (1009 to 1012), and its receipt back.
fn merchant() -> Quest {
    const CHIEF: (&str, &str, u16, u16) = ("alberta_in", "Merchant", 53, 43);
    const STOREKEEPER: (&str, &str, u16, u16) = ("alberta_in", "Merchant Guildsman", 28, 29);
    const PACKAGES: [Check; 2] = [Check::Item(1081), Check::Item(1082)];
    let package = |town: &'static str, log: &'static [u32], branch: &'static str| {
        let steps: &'static [&'static str] = match branch {
            "prontera" => &[
                "My Merchant License test.",
                "Prontera.",
                "{remembered}",
                "Positive.",
            ],
            "geffen" => &[
                "My Merchant License test.",
                "Geffen.",
                "{remembered}",
                "Positive.",
            ],
            "morocc" => &[
                "My Merchant License test.",
                "Morocc.",
                "{remembered}",
                "Positive.",
            ],
            _ => &[
                "My Merchant License test.",
                "Byalan Island.",
                "{remembered}",
                "Positive.",
            ],
        };
        let mut stage = talk(
            town,
            STOREKEEPER.0,
            STOREKEEPER.1,
            STOREKEEPER.2,
            STOREKEEPER.3,
        )
        .steps(steps)
        .log(log)
        .branch(branch);
        for check in PACKAGES {
            stage = stage.done(check);
        }
        stage
    };
    first_job(
        "Merchant",
        vec![
            gather_zeny("save the 1,000 zeny of the Merchant Guild fee", &[], 1000),
            talk(
                "apply to Chief Mahnsoo and pay the fee",
                CHIEF.0,
                CHIEF.1,
                CHIEF.2,
                CHIEF.3,
            )
            .menu(&[
                "I want to be a Merchant.",
                "Yes, I will.",
                "Pay all 1,000 Zeny now!",
            ])
            .remembers(SERIALS)
            .done(Check::Quest(1009))
            .done(Check::Quest(1010))
            .done(Check::Quest(1011))
            .done(Check::Quest(1012)),
            package(
                "take the package for Prontera from the storehouse",
                &[1009],
                "prontera",
            ),
            talk(
                "deliver the package to Blossom in Prontera",
                "prontera",
                "Kafra Employee",
                248,
                42,
            )
            .done(Check::Item(1073))
            .done(Check::Item(1074))
            .log(&[1009])
            .branch("prontera"),
            package(
                "take the package for Geffen from the storehouse",
                &[1010],
                "geffen",
            ),
            talk(
                "deliver the package to the Mage Guild in Geffen",
                "geffen_in",
                "Guild Staff",
                155,
                122,
            )
            .done(Check::Item(1075))
            .done(Check::Item(1076))
            .log(&[1010])
            .branch("geffen"),
            package(
                "take the package for Morocc from the storehouse",
                &[1011],
                "morocc",
            ),
            talk(
                "deliver the package to the dyemaker's student in Morocc",
                "morocc_in",
                "Student",
                140,
                102,
            )
            .done(Check::Item(1077))
            .done(Check::Item(1078))
            .log(&[1011])
            .branch("morocc"),
            package(
                "take the package for Byalan Island from the storehouse",
                &[1012],
                "byalan",
            ),
            talk(
                "deliver the package to Blossom on Byalan Island",
                "izlu2dun",
                "Kafra Employee",
                106,
                58,
            )
            .menu(&["This is from Chief Mahnsoo"])
            .done(Check::Item(1079))
            .done(Check::Item(1080))
            .log(&[1012])
            .branch("byalan"),
            talk(
                "hand the receipt to the storekeeper",
                STOREKEEPER.0,
                STOREKEEPER.1,
                STOREKEEPER.2,
                STOREKEEPER.3,
            )
            .said("Now go talk to Chief Mahnsoo")
            .log(&[1009, 1010, 1011, 1012]),
            talk(
                "become a Merchant at Chief Mahnsoo",
                CHIEF.0,
                CHIEF.1,
                CHIEF.2,
                CHIEF.3,
            )
            .menu(&["Pay the rest of the 500 Zeny"])
            .done(Check::Job("Merchant"))
            .log(&[1009, 1010, 1011, 1012]),
        ],
    )
}

/// The Thief Guild under the Pyramid of Morocc (`thief.txt`): mushrooms picked on a farm, 25 points or more (the
/// first kind 3 points each, the second 1). Quest window: 1013.
fn thief() -> Quest {
    const MUSHROOMS: super::Items = &[(1069, 3), (1070, 1)];
    first_job(
        "Thief",
        vec![
            talk(
                "ask the Thief Guide to join the guild",
                "moc_prydb1",
                "Thief Guide",
                39,
                129,
            )
            .menu(&[
                "Hey, I came here to be a Thief!",
                "Yeah.",
                "Yes, I will.",
                "Yeah, I'm ready.",
                "Yes, I do.",
            ])
            .done(Check::Quest(1013)),
            talk(
                "ask Mr. Irrelevant for the way to the farm",
                "moc_ruins",
                "Mr. Irrelevant",
                141,
                125,
            )
            .done(Check::OnMap("job_thief1"))
            .log(&[1013]),
            fight("pick mushrooms worth 25 points on the farm", "job_thief1")
                .until(Check::Points(MUSHROOMS, 25))
                .passed(Check::Points(MUSHROOMS, 25))
                .log(&[1013]),
            talk(
                "bring the mushrooms to the Comrade",
                "moc_prydb1",
                "Comrade",
                42,
                133,
            )
            .done(Check::Job("Thief"))
            .log(&[1013]),
        ],
    )
}
