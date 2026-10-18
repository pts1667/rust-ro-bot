//! The rebirth of pre-renewal and the reborn line it opens, as the server's `jobs_misc` scripts carry them out
//! (rathena `npc/jobs/valkyrie.txt`, `2-1a`, `2-2a`, and the reborn talk of each guild in `npc/pre-re/jobs/1-1`).
//!
//! A second job at base level 99 and job level 50 donates 1,285,000 zeny to Metheus Sylphe in Juno's library, reads
//! the Book of Ymir, leaves every item and all its zeny behind, and touches the Heart of Ymir, which leads to
//! Valhalla. There the Valkyrie makes it a High Novice and sends it to the town of its first job's guild, whose master
//! makes it the high first job at job level 10 with Basic Skill 9. At job level 40, the Book of Ymir leads the reborn
//! back to Valhalla, where the master of its old second job makes it the transcendent one. The scripts remember the
//! line in a variable nobody sees (`advjob`): the bot remembers its second job (`past_job`) to follow it.

use super::build::*;
use super::{Check, Quest, Reward, Stage, Unburden};
use crate::knowledge::classes::{SECOND_JOBS, TRANSCENDENT};

/// The map and region of the Heart of Ymir: the room `yun76` of the random warps of `yuno_in05` lands in. The
/// rebirth leaves everything behind on the way there, so it is only taken up when a route leads into that room.
pub const HEART_ROOM: (&str, u32) = ("yuno_in05", 3);

/// What Metheus Sylphe asks for the Book of Ymir, and the fee of Juno's Kafra storage.
const DONATION: u64 = 1_285_000;
const STORAGE_FEE: u64 = 40;

pub fn quests() -> Vec<Quest> {
    let mut v = vec![rebirth()];
    v.extend(HIGH_FIRST_JOBS.iter().map(high_first_job));
    v.extend(
        TRANSCENDENT
            .iter()
            .map(|t| transcendent(t.second, t.name, t.npc, t.x, t.y)),
    );
    v
}

fn rebirth() -> Quest {
    Quest {
        id: "rebirth",
        title: "Rebirth quest",
        reward: Reward::Rebirth,
        jobs: &SECOND_JOBS,
        job_level: 50,
        base_level: 99,
        sex: None,
        reborn: Some(false),
        past: &[],
        no_cart: true,
        skills: &[],
        stages: vec![
            gather_zeny(
                "save 1,285,040 zeny: the donation for the Book of Ymir and the storage fee",
                &[],
                DONATION + STORAGE_FEE,
            ),
            talk(
                "donate 1,285,000 zeny to Metheus Sylphe for the Book of Ymir",
                "yuno_in02",
                "Metheus Sylphe",
                88,
                164,
            )
            .menu(&["Donate."])
            .said("now go in and read the book")
            // Donated before
            .said("thank you for your generous donation"),
            talk(
                "read the Book of Ymir",
                "yuno_in02",
                "Book of Ymir",
                93,
                207,
            )
            .done(Check::Quest(1000)),
            unburden(
                "leave every item and all zeny in the Kafra storage",
                Unburden {
                    map: "yuno",
                    // Armour of up to 94,000 zeny for the bulk, potions and 1-zeny Arrows for the change
                    shops: &[("Mr. King's Shop", 205, 103), ("Tool Dealer", 218, 97)],
                    kafra: ("Kafra Employee", 152, 187),
                    fee: STORAGE_FEE,
                },
            )
            .log(&[1000]),
            // Through the random warps of the library's basement into its room
            talk(
                "touch the Heart of Ymir",
                HEART_ROOM.0,
                "Heart of Ymir",
                49,
                43,
            )
            .room(HEART_ROOM.1)
            .done(Check::OnMap("valkyrie"))
            .log(&[1000]),
            talk(
                "ask the Valkyrie for a new life",
                "valkyrie",
                "Valkyrie",
                48,
                86,
            )
            .inside()
            .done(Check::Job("Novice"))
            // Something was still carried: she sends the bot back to Juno's library
            .back_if("you cannot take anything with you", 3)
            .log(&[1000]),
        ],
    }
}

/// The high first jobs, the second jobs whose line each continues, and the guild master who changes a High Novice
/// to it without a test: `(job, past second jobs, map, npc, x, y)`. The Thief's and the Acolyte's line start at
/// their guilds too.
type HighFirstJob = (
    &'static str,
    &'static [&'static str],
    &'static str,
    &'static str,
    u16,
    u16,
);

const HIGH_FIRST_JOBS: [HighFirstJob; 6] = [
    (
        "Swordman",
        &["Knight", "Crusader"],
        "izlude_in",
        "Swordman",
        74,
        172,
    ),
    (
        "Mage",
        &["Wizard", "Sage"],
        "geffen_in",
        "Mage Guildsman",
        164,
        124,
    ),
    (
        "Archer",
        &["Hunter", "Bard", "Dancer"],
        "payon_in02",
        "Archer Guildsman",
        64,
        71,
    ),
    (
        "Acolyte",
        &["Priest", "Monk"],
        "prt_church",
        "Cleric",
        184,
        41,
    ),
    (
        "Merchant",
        &["Blacksmith", "Alchemist"],
        "alberta_in",
        "Merchant",
        53,
        43,
    ),
    (
        "Thief",
        &["Assassin", "Rogue"],
        "moc_prydb1",
        "Thief Guide",
        39,
        129,
    ),
];

fn high_first_job(&(job, past, map, npc, x, y): &HighFirstJob) -> Quest {
    let what: &'static str =
        Box::leak(format!("ask the {npc} to be a {job} again").into_boxed_str());
    Quest {
        id: Box::leak(format!("job:{job} High").into_boxed_str()),
        title: Box::leak(format!("High {job} job change").into_boxed_str()),
        reward: Reward::Job(job),
        jobs: &["Novice"],
        job_level: 10,
        base_level: 1,
        sex: None,
        reborn: Some(true),
        past,
        no_cart: false,
        skills: &[("NV_BASIC", 9)],
        stages: vec![talk(what, map, npc, x, y).done(Check::Job(job))],
    }
}

fn transcendent(
    second: &'static str,
    name: &'static str,
    npc: &'static str,
    x: u16,
    y: u16,
) -> Quest {
    let first = HIGH_FIRST_JOBS
        .iter()
        .find(|j| j.1.contains(&second))
        .map(|j| j.0)
        .expect("each second job continues a first job");
    let become_it: &'static str =
        Box::leak(format!("ask the {npc} to become a {name}").into_boxed_str());
    let stages: Vec<Stage> = vec![
        talk(
            "read the Book of Ymir to reach Valhalla",
            "yuno_in02",
            "Book of Ymir",
            93,
            207,
        )
        .menu(&["Continue reading."])
        .done(Check::OnMap("valkyrie")),
        talk(become_it, "valkyrie", npc, x, y)
            .inside()
            .menu(&["Yes."])
            .done(Check::Job(second)),
    ];
    Quest {
        id: Box::leak(format!("job:{name}").into_boxed_str()),
        title: Box::leak(format!("{name} job change").into_boxed_str()),
        reward: Reward::Job(second),
        jobs: Box::leak(vec![first].into_boxed_slice()),
        job_level: 40,
        base_level: 1,
        sex: None,
        reborn: Some(true),
        past: Box::leak(vec![second].into_boxed_slice()),
        no_cart: false,
        skills: &[],
        stages,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{QuestProgress, find};
    use crate::api::Observation;
    use crate::api::observation::{InventoryItem, QuestEntry};

    /// A character of the server's class `class` at these levels, read as the bot reads it.
    fn character(class: &str, base: u32, job: u32, past: Option<&str>) -> Observation {
        let mut obs = Observation::default();
        obs.me.job = class.into();
        obs.me.base_level = base;
        obs.me.job_level = job;
        obs.read_classes();
        obs.me.past_job = past.map(str::to_owned);
        obs
    }

    #[test]
    fn a_second_job_at_99_and_50_is_reborn_without_a_cart() {
        let q = find("rebirth").unwrap();
        assert!(q.open_to(&character("Knight", 99, 50, None)));
        assert!(!q.open_to(&character("Knight", 98, 50, None)));
        assert!(!q.open_to(&character("Swordman", 99, 50, None)));
        let mut cart = character("Blacksmith", 99, 50, None);
        cart.cart = Some(Vec::new());
        assert!(!q.open_to(&cart));
        // Once reborn, done
        let high = character("NoviceHigh", 1, 1, Some("Knight"));
        assert!(q.finished(&high) && !q.open_to(&high));
    }

    #[test]
    fn the_rebirth_goes_on_from_where_the_quest_window_stands() {
        let q = find("rebirth").unwrap();
        let mut obs = character("Priest", 99, 50, None);
        obs.me.zeny = 2_000_000;
        let p = QuestProgress::default();
        // Enough zeny saved: the donation is next
        assert_eq!(q.current_stage(&p, &obs), 1);
        // The book read: everything is left behind next, then the Heart
        obs.quests.push(QuestEntry {
            id: 1000,
            ..Default::default()
        });
        obs.inventory.push(InventoryItem {
            item_id: 2302,
            amount: 1,
            ..Default::default()
        });
        assert_eq!(q.current_stage(&p, &obs), 3);
        obs.inventory.clear();
        obs.me.zeny = 0;
        assert_eq!(q.current_stage(&p, &obs), 4);
    }

    #[test]
    fn a_reborn_character_follows_the_line_of_its_old_second_job() {
        let novice = character("NoviceHigh", 30, 10, Some("Knight"));
        let mut novice = novice;
        novice.skills.push(crate::api::observation::Skill {
            name: "NV_BASIC".into(),
            level: 9,
            ..Default::default()
        });
        assert!(find("job:Swordman High").unwrap().open_to(&novice));
        assert!(!find("job:Mage High").unwrap().open_to(&novice));
        // The guild's own test is for a Novice not reborn
        assert!(!find("job:Swordman").unwrap().open_to(&novice));

        let high = character("SwordsmanHigh", 70, 40, Some("Knight"));
        assert_eq!(high.me.job, "Swordsman");
        assert!(high.me.reborn);
        assert!(find("job:Lord Knight").unwrap().open_to(&high));
        assert!(!find("job:Paladin").unwrap().open_to(&high));
        assert!(!find("job:Knight").unwrap().open_to(&high));
        assert!(!find("job:Lord Knight").unwrap().open_to(&character(
            "SwordsmanHigh",
            70,
            39,
            Some("Knight")
        )));
        // A Knight is no Lord Knight; a Lord Knight is
        let lk = find("job:Lord Knight").unwrap();
        assert!(!lk.finished(&character("Knight", 99, 50, None)));
        assert!(lk.finished(&character("LordKnight", 80, 1, Some("Knight"))));
    }
}
