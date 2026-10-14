//! Experiment: how the decision model's pick of a sentence fares with one flat choice, with the kinds first, or with
//! finer topics first. `cargo test voice_eval -- --ignored --nocapture`.

use super::SILENCE;
use super::known::Known;
use super::menu::{Menu, Picked};
use super::phrase::{Category, TOPICS};
use std::time::Instant;

use super::systemone::{Question, SystemOne};
use crate::social::facts::Lore;

fn bot() -> Known {
    Known {
        me: "Elara".into(),
        level: 9,
        job_level: 7,
        job: "Novice".into(),
        map: "prt_fild08".into(),
        doing: "hunt on prt_fild08".into(),
        hunt_map: Some("prt_fild08".into()),
        hunting: true,
        in_sight: vec!["Poring".into(), "Lunatic".into()],
        kills: vec![
            ("Poring".into(), 40),
            ("Lunatic".into(), 12),
            ("Fabre".into(), 3),
        ],
        deaths: vec![("Fabre".into(), 2)],
        kills_total: 55,
        deaths_total: 2,
        journal: vec![
            "reached base level 9 on prt_fild08".into(),
            "bought Knife for 50 z".into(),
        ],
        party: Some(vec!["Bravo".into()]),
        other: Some("Bravo".into()),
        lore: vec![Lore::Monster {
            name: "Fabre".into(),
            level: 2,
            hp: 63,
            aggressive: false,
            maps: vec![("prt_fild05".into(), 60), ("prt_fild01".into(), 20)],
            drops: vec!["Fluff".into(), "Feather".into()],
        }],
        ..Default::default()
    }
}

#[test]
#[ignore]
fn voice_candidates() {
    for c in Category::SPOKEN {
        let of: Vec<String> = bot()
            .candidates()
            .iter()
            .filter(|p| p.category() == c)
            .map(|p| format!("{}={}", p.template, p.render()))
            .collect();
        println!("{:?} ({}): {}", c, of.len(), of.join(" | "));
    }
}

/// A chat line (Bravo's, in party chat) and the answers that fit; "silence" for saying nothing.
const CASES: &[(&[&str], &[&str])] = &[
    (
        &["Bravo: hi Elara!"],
        &[
            "Hello!",
            "Hi Bravo!",
            "Good to see you, Bravo",
            "Hey everyone!",
        ],
    ),
    (
        &["Bravo: anyone seen Fabres around?"],
        &[
            "Fabre lives on prt_fild05",
            "Fabre lives on prt_fild01",
            "Lots of Fabre on prt_fild05",
            "Lots of Fabre on prt_fild01",
            "Killed 3 Fabre",
            "Watch out for Fabre",
            "Fabre killed me 2 times",
        ],
    ),
    (
        &["Bravo: what level are you?"],
        &["I'm a level 9 Novice", "Level 9 now!"],
    ),
    (
        &["Bravo: where are you hunting?"],
        &["Hunting on prt_fild08", "Good spot for level 9: prt_fild08"],
    ),
    (
        &["Bravo: is Fabre strong?"],
        &[
            "Fabre is level 2",
            "Fabre has 63 HP",
            "Fabre is passive",
            "Fabre killed me 2 times",
            "Fabre hits hard",
            "Watch out for Fabre",
        ],
    ),
    (
        &["Bravo: what does Fabre drop?"],
        &["Fabre drops Fluff", "Fabre drops Feather"],
    ),
    (
        &["Bravo: I just reached level 10!"],
        &["Congrats!", "Nice!"],
    ),
    (&["Bravo: thanks for the help"], &["No problem"]),
    (
        &["Bravo: ok, I have to go now, bye"],
        &["Bye!", "See you!", "Take care!", "Good night"],
    ),
    (
        &["Bravo: good luck out there"],
        &["Thanks!", "Thank you!", "You too!"],
    ),
    (
        &["Bravo: can you wait for me at the bridge?"],
        &["Sure", "Got it", "No problem", "Yes"],
    ),
    (&["Bravo: argh, I died again"], &["Sorry", "Be careful"]),
    (
        &["Bravo: how many Porings did you kill?"],
        &["Killed 40 Poring"],
    ),
    (
        &["Bravo: have you died much?"],
        &["Died 2 times so far", "Fabre killed me 2 times"],
    ),
    (
        &["Bravo: what monsters are around you?"],
        &[
            "Poring is here on prt_fild08",
            "Lunatic is here on prt_fild08",
        ],
    ),
    (
        &["Bravo: see you later", "you: Bye!", "Bravo: ok"],
        &["silence"],
    ),
    (
        &["Bravo: is prt_fild08 a good map?"],
        &[
            "Good spot for level 9: prt_fild08",
            "Yes",
            "Poring is easy at level 9",
        ],
    ),
    (&["Bravo: do you have potions to spare?"], &["No", "Sorry"]),
    (&["Bravo: I love this game"], &["Me too", "Same here"]),
    (&["Bravo: did you buy any gear?"], &["Bought Knife", "Yes"]),
    (
        &["Bravo: how many monsters have you killed?"],
        &["Killed 55 monsters so far"],
    ),
    (
        &["Bravo: are Porings easy for you?"],
        &["Poring is easy at level 9", "Yes", "Killed 40 Poring"],
    ),
    (&["Bravo: what's your job level?"], &["My job level is 7"]),
    (
        &["Bravo: Lunatics are so cute lol"],
        &["Haha", "Me too", "Same here"],
    ),
];

/// A chat line, a topic opened that does not fit it, and the answers that fit.
const WRONG_STARTS: &[(&str, &str, &[&str])] = &[
    (
        "Bravo: what does Fabre drop?",
        "bye",
        &["Fabre drops Fluff", "Fabre drops Feather"],
    ),
    (
        "Bravo: what level are you?",
        "danger",
        &["I'm a level 9 Novice", "Level 9 now!"],
    ),
    (
        "Bravo: hi Elara!",
        "items",
        &[
            "Hello!",
            "Hi Bravo!",
            "Good to see you, Bravo",
            "Hey everyone!",
        ],
    ),
    (
        "Bravo: how many Porings did you kill?",
        "greet",
        &["Killed 40 Poring"],
    ),
    (
        "Bravo: ok, I have to go now, bye",
        "my_maps",
        &["Bye!", "See you!", "Take care!", "Good night"],
    ),
    (
        "Bravo: where are you hunting?",
        "monster_strength",
        &["Hunting on prt_fild08", "Good spot for level 9: prt_fild08"],
    ),
];

const ASK_KIND: &str = "What kind of message fits best to answer Bravo's last line?";
const ASK_SENTENCE: &str = "Which message should be sent to answer Bravo's last line?";

/// One way of picking: the expected share of right answers (from the model's probabilities), how often the top path
/// is right, and the time of the calls the bot would make.
#[derive(Default)]
struct Score {
    mass: f64,
    top: usize,
    ms: u128,
}

/// A group of sentences: (key, description, sentences).
type Group = (String, String, Vec<String>);

/// Groups first. P(right) = sum of P(group) P(right | group), over the groups with at least 3% and the top one; the
/// time is that of the group call and the top group's sentence call.
async fn grouped(
    s1: &SystemOne,
    state: &str,
    groups: &[Group],
    good: &[&str],
) -> (f64, bool, u128) {
    let mut options: Vec<(String, String)> = groups
        .iter()
        .map(|(k, d, _)| (k.clone(), d.clone()))
        .collect();
    options.push(("silence".into(), SILENCE.into()));
    let start = Instant::now();
    let a = s1
        .ask(state, &[("q", Question::choice(ASK_KIND, options))])
        .await
        .unwrap();
    let mut ms = start.elapsed().as_millis();
    let a = &a["q"];
    let top_group = a.choice.clone().unwrap_or_default();
    let p_silence = a.probabilities.get("silence").copied().unwrap_or(0.0);
    let mut seen = p_silence;
    let mut mass = if good.contains(&"silence") {
        p_silence
    } else {
        0.0
    };
    let mut top_right = top_group == "silence" && good.contains(&"silence");
    if top_group == "silence" {
        print!(" [silence]");
    }
    for (key, _, sentences) in groups {
        let p = a.probabilities.get(key).copied().unwrap_or(0.0);
        if p < 0.03 && *key != top_group {
            continue;
        }
        seen += p;
        let (right, top) = if sentences.len() == 1 {
            (
                if good.contains(&sentences[0].as_str()) {
                    1.0
                } else {
                    0.0
                },
                sentences[0].clone(),
            )
        } else {
            let opts: Vec<(String, String)> = sentences
                .iter()
                .enumerate()
                .map(|(i, s)| (format!("s{i}"), s.clone()))
                .collect();
            let start = Instant::now();
            let b = s1
                .ask(state, &[("q", Question::choice(ASK_SENTENCE, opts))])
                .await
                .unwrap();
            if *key == top_group {
                ms += start.elapsed().as_millis();
            }
            let b = &b["q"];
            let right: f64 = sentences
                .iter()
                .enumerate()
                .filter(|(_, s)| good.contains(&s.as_str()))
                .map(|(i, _)| {
                    b.probabilities
                        .get(&format!("s{i}"))
                        .copied()
                        .unwrap_or(0.0)
                })
                .sum();
            let top = b
                .choice
                .as_deref()
                .and_then(|k| k.strip_prefix('s'))
                .and_then(|i| i.parse::<usize>().ok())
                .map(|i| sentences[i].clone())
                .unwrap_or_default();
            (right, top)
        };
        mass += p * right;
        if *key == top_group {
            top_right = good.contains(&top.as_str());
            print!(" [{key}: {top}]");
        }
    }
    (if seen > 0.0 { mass / seen } else { 0.0 }, top_right, ms)
}

#[tokio::test]
#[ignore]
async fn voice_eval() {
    let s1 = SystemOne::new("http://127.0.0.1:9931").unwrap();
    let known = bot();
    let candidates = known.candidates();
    let sentences: Vec<(String, &str)> = candidates
        .iter()
        .map(|p| (p.render(), p.template))
        .collect();
    let kinds: Vec<Group> = Category::SPOKEN
        .iter()
        .map(|c| {
            (
                c.key().to_owned(),
                c.description().to_owned(),
                candidates
                    .iter()
                    .filter(|p| p.category() == *c)
                    .map(|p| p.render())
                    .collect(),
            )
        })
        .filter(|(_, _, s): &Group| !s.is_empty())
        .collect();
    let topics: Vec<Group> = TOPICS
        .iter()
        .map(|t| {
            (
                t.key.to_owned(),
                t.description.to_owned(),
                sentences
                    .iter()
                    .filter(|(_, id)| t.templates.contains(id))
                    .map(|(s, _)| s.clone())
                    .collect(),
            )
        })
        .filter(|(_, _, s): &Group| !s.is_empty())
        .collect();
    let missing: Vec<&str> = sentences
        .iter()
        .filter(|(s, _)| !topics.iter().any(|(_, _, of)| of.contains(s)))
        .map(|(_, t)| *t)
        .collect();
    assert!(missing.is_empty(), "templates without a topic: {missing:?}");
    let (mut flat, mut coarse, mut fine) = (Score::default(), Score::default(), Score::default());
    // The menu, at the model's best picks: the right answers it ends on, the time, and how often it went back
    let (mut menu_right, mut menu_ms, mut menu_backs, mut menu_back_cases) = (0, 0u128, 0, 0);
    let menu = Menu {
        phrases: candidates.iter().collect(),
        extra: vec![("silence".to_owned(), SILENCE.to_owned())],
        ask_topic: ASK_KIND.to_owned(),
        ask_sentence: ASK_SENTENCE.to_owned(),
        best: true,
    };
    for (lines, good) in CASES {
        let state = format!(
            "{}\n\nConversation with Bravo (party chat), oldest first (\"you\" is Elara):\n{}",
            super::state_of(&known),
            lines.join("\n")
        );
        println!("{}", lines.last().unwrap());
        // Flat: every sentence and silence in one choice
        let mut options: Vec<(String, String)> = sentences
            .iter()
            .enumerate()
            .map(|(i, (s, _))| (format!("s{i}"), s.clone()))
            .collect();
        options.push(("silence".into(), SILENCE.into()));
        let start = Instant::now();
        let a = s1
            .ask(&state, &[("q", Question::choice(ASK_SENTENCE, options))])
            .await
            .unwrap();
        flat.ms += start.elapsed().as_millis();
        let a = &a["q"];
        let text = |k: &str| {
            if k == "silence" {
                "silence".to_owned()
            } else {
                k.strip_prefix('s')
                    .and_then(|i| i.parse::<usize>().ok())
                    .map(|i| sentences[i].0.clone())
                    .unwrap_or_default()
            }
        };
        let mass: f64 = a
            .probabilities
            .iter()
            .filter(|(k, _)| good.contains(&text(k).as_str()))
            .map(|(_, p)| p)
            .sum();
        let top = text(a.choice.as_deref().unwrap_or(""));
        flat.mass += mass;
        flat.top += good.contains(&top.as_str()) as usize;
        println!("  flat   {mass:.2} [{top}]");
        for (name, score, groups) in [
            ("kinds ", &mut coarse, &kinds),
            ("topics", &mut fine, &topics),
        ] {
            print!("  {name}");
            let (mass, right, ms) = grouped(&s1, &state, groups, good).await;
            score.mass += mass;
            score.top += right as usize;
            score.ms += ms;
            println!(" {mass:.2}");
        }
        let start = Instant::now();
        let walk = menu.walk(&s1, &state, None).await.unwrap();
        menu_ms += start.elapsed().as_millis();
        let ended = match &walk.picked {
            Some(Picked::Phrase(p)) => p.render(),
            Some(Picked::Extra(k)) => k.clone(),
            None => String::new(),
        };
        let right = good.contains(&ended.as_str());
        menu_right += right as usize;
        menu_backs += walk.backs;
        menu_back_cases += (walk.backs > 0) as usize;
        println!(
            "  menu   {} {}",
            if right { "right" } else { "WRONG" },
            walk.steps.join(" > ")
        );
    }
    let n = CASES.len() as f64;
    for (name, s, groups) in [
        ("flat", &flat, 1),
        ("kinds", &coarse, kinds.len()),
        ("topics", &fine, topics.len()),
    ] {
        println!(
            "{name:<7} groups {groups:>2}  expected right {:.0}%  top right {}/{}  {:.0} ms per message",
            100.0 * s.mass / n,
            s.top,
            CASES.len(),
            s.ms as f64 / n
        );
    }
    // Started inside a topic that does not fit: does the model select Back and find the right one?
    let (mut wrong_backs, mut wrong_right) = (0, 0);
    for (line, start, good) in WRONG_STARTS {
        let state = format!(
            "{}

Conversation with Bravo (party chat), oldest first (\"you\" is Elara):
{line}",
            super::state_of(&known)
        );
        let open = TOPICS.iter().find(|t| t.key == *start).unwrap();
        let walk = menu.walk(&s1, &state, Some(open)).await.unwrap();
        let ended = match &walk.picked {
            Some(Picked::Phrase(p)) => p.render(),
            Some(Picked::Extra(k)) => k.clone(),
            None => String::new(),
        };
        let right = good.contains(&ended.as_str());
        wrong_right += right as usize;
        wrong_backs += (walk.backs > 0) as usize;
        println!(
            "{line}
  opened {start}: {} {}",
            if right { "right" } else { "WRONG" },
            walk.steps.join(" > ")
        );
    }
    println!(
        "wrong start: Back selected in {wrong_backs}/{n}, ends right {wrong_right}/{n}",
        n = WRONG_STARTS.len()
    );
    println!(
        "menu    groups {:>2}  ends right {menu_right}/{}  {:.0} ms per message  {menu_backs} backs in {menu_back_cases} messages",
        topics.len(),
        CASES.len(),
        menu_ms as f64 / n
    );
}
