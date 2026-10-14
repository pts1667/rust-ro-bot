use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::knowledge::db::Db;

#[test]
fn findings_come_from_the_lines_worth_telling() {
    let f = finding_of("Spore keeps killing me: leaving it alone for a few levels").unwrap();
    assert_eq!(
        (f.key.as_str(), f.subject.as_str()),
        ("danger:spore", "Spore")
    );
    assert_eq!(
        finding_of("changed job: Novice -> Acolyte").unwrap().key,
        "job:acolyte"
    );
    assert_eq!(
        finding_of("Stone Fling finished").unwrap().key,
        "quest:stone fling"
    );
    let shop =
        finding_of("could not open the shop of Tool Dealer on ein_in01, looking for another")
            .unwrap();
    assert_eq!(
        (shop.key.as_str(), shop.subject.as_str()),
        ("shop:tool dealer on ein_in01", "ein_in01")
    );
    assert_eq!(
        finding_of("reached base level 15 on prt_fild08")
            .unwrap()
            .key,
        "hunt:prt_fild08"
    );
    assert!(finding_of("reached base level 16 on prt_fild08").is_none());
    assert!(finding_of("reached base level 15 on prontera").is_none());
    assert!(finding_of("sold loot for 300 z, now 900 z").is_none());
}

#[test]
fn a_finding_is_kept_until_a_post_names_it() {
    let mut m = ForumMemory::default();
    let snap = Snapshot {
        journal: vec!["Spore keeps killing me: leaving it alone for a few levels".into()],
        ..Default::default()
    };
    m.note_journal(&snap);
    m.note_journal(&snap);
    assert_eq!(m.pending.len(), 1);
    m.shared_in("Watch out for Poring");
    assert_eq!(m.pending.len(), 1);
    m.shared_in("Spores hit hard on prt_fild05");
    assert!(m.pending.is_empty());
    m.journal_seen.clear();
    m.note_journal(&snap);
    assert!(
        m.pending.is_empty(),
        "a finding shared is not offered again"
    );
}

#[test]
fn a_reply_makes_the_visit_sure_and_comes_once() {
    let mut m = ForumMemory::default();
    assert!(m.wants_visit(0.1) && !m.wants_visit(0.9));
    m.last_visit = now();
    assert!(!m.wants_visit(0.0), "not again right after a visit");
    let notice = Notice {
        event: "reply".into(),
        seq: 4,
        thread_id: 1,
        from: "Ann".into(),
        ..Default::default()
    };
    assert!(m.receive(notice.clone()));
    assert!(
        !m.receive(notice),
        "the socket sends again what came after `after`"
    );
    assert!(m.wants_visit(0.99));
}

#[test]
fn what_the_bot_writes_names_only_what_it_saw() {
    let seen = "Ann wrote \"Spores\": Watch out, Spores on prt_fild05 hit hard.";
    assert!(check_names("Spores on prt_fild05 hit hard for low levels.", seen).is_ok());
    assert!(
        check_names("Avoid the Spore there.", seen).is_ok(),
        "a plural in the source covers the singular"
    );
    assert!(
        check_names("prt_fild08 is a good map for level 15.", seen)
            .unwrap_err()
            .contains("prt_fild08")
    );
    assert!(check_names("Try the Payon fields.", seen).is_err());
    assert_eq!(
        names("Try the Payon fields near prt_fild08. Spores hit hard."),
        vec!["Payon", "prt_fild08"]
    );
    assert_eq!(
        first_sentences("One. Two! Three? Four. Five.", 4),
        "One. Two! Three? Four."
    );
    assert_eq!(first_sentences("No end", 4), "No end");
    assert_eq!(
        first_sentences("Try:\n1. Payon\n2. Morocc. Bye.", 1),
        "Try:\n1. Payon\n2. Morocc."
    );
    assert_eq!(
        names("Spores on prt_fild05—they hit hard"),
        vec!["prt_fild05"]
    );
    assert!(
        similar("Spores on prt_fild05!", "spores on prt_fild05")
            && similar("Spores", "Beware: Spores on prt_fild05")
    );
    assert!(!similar("Spores", "Porings"));
    let own = "prontera
- deaths: 37 (Spore 34)
- reached base level 15 on prt_fild08";
    assert!(
        check_claims(
            "I did check the fields around Payon, but Spores were strong.",
            own
        )
        .unwrap_err()
        .contains("Payon")
    );
    assert!(
        check_claims(
            "Dee says Payon is better. I hunted on prt_fild08 and Spore killed me a lot.",
            own
        )
        .is_ok()
    );
    assert!(
        check_claims("I should try Payon next.", own).is_ok(),
        "a plan is no claim"
    );
    assert!(
        check_claims("Thanks! Yeah, I tried Spore-infested prt_fild08.", own).is_ok(),
        "first words and hyphens are no names"
    );
    assert_eq!(
        names("Avoid Spore-infested fields. Yeah, Payon."),
        vec!["Spore", "Payon"]
    );
}

/// A forum on a local port that answers the four requests a visit makes, and keeps what was written.
struct MockForum {
    url: String,
    writes: Arc<Mutex<Vec<String>>>,
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn route(
    method: &str,
    path: &str,
    body: &str,
    threads: &Value,
    views: &HashMap<u64, Value>,
    writes: &Mutex<Vec<String>>,
) -> (u16, String) {
    let request: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    if path == "/v1/systemone" {
        return (200, decide(&request).to_string());
    }
    let rest = path.strip_prefix("/forum/threads").unwrap_or("");
    match (method, rest) {
        ("GET", r) if r.is_empty() || r.starts_with('?') => (200, threads.to_string()),
        ("POST", "") => {
            writes.lock().unwrap().push(format!(
                "thread: {} / {}",
                request["title"].as_str().unwrap_or(""),
                request["body"].as_str().unwrap_or("")
            ));
            (200, json!({"id": 99, "author": "Mercy", "title": request["title"], "body": request["body"], "comments": 0}).to_string())
        }
        ("POST", r) if r.ends_with("/comments") => {
            let thread: u64 = r
                .trim_start_matches('/')
                .trim_end_matches("/comments")
                .parse()
                .unwrap_or(0);
            writes.lock().unwrap().push(format!(
                "comment on {thread} (parent {}): {}",
                request["parent"],
                request["body"].as_str().unwrap_or("")
            ));
            (
                200,
                json!({"id": 500, "thread_id": thread, "parent": request["parent"], "author": "Mercy", "body": request["body"]})
                    .to_string(),
            )
        }
        ("GET", r) => match r
            .trim_start_matches('/')
            .parse::<u64>()
            .ok()
            .and_then(|id| views.get(&id))
        {
            Some(view) => (200, view.to_string()),
            None => (404, json!({"error": "There is no thread"}).to_string()),
        },
        _ => (400, json!({"error": "unexpected"}).to_string()),
    }
}

/// A decision model that is sure of everything: yes to every noul, and the first option of a choice that is not to
/// leave or do nothing.
fn decide(request: &Value) -> Value {
    let mut answers = serde_json::Map::new();
    for (name, q) in request["questions"].as_object().into_iter().flatten() {
        let answer = match q["type"].as_str() {
            Some("noul") => json!({"type": "noul", "noul": 0.9}),
            _ => {
                let keys: Vec<&String> = q["criteria"]
                    .as_object()
                    .map(|c| c.keys().collect())
                    .unwrap_or_default();
                let pick = keys
                    .iter()
                    .find(|k| !["leave", "none"].contains(&k.as_str()))
                    .or(keys.first())
                    .map(|k| k.as_str())
                    .unwrap_or("");
                json!({"type": "choice", "choice": pick, "probabilities": {pick: 1.0}, "confidence": 1.0})
            }
        };
        answers.insert(name.clone(), answer);
    }
    json!({"answers": answers, "usage": {"input_tokens": 1}})
}

async fn mock_forum(threads: Value, views: HashMap<u64, Value>) -> MockForum {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let writes = Arc::new(Mutex::new(Vec::new()));
    let w = writes.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let (threads, views, writes) = (threads.clone(), views.clone(), w.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = socket.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let Some(end) = find(&buf, b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&buf[..end]).to_string();
                    let len = head
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if buf.len() < end + 4 + len {
                        continue;
                    }
                    let body = String::from_utf8_lossy(&buf[end + 4..end + 4 + len]).to_string();
                    let mut first = head.lines().next().unwrap_or("").split(' ');
                    let (method, path) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
                    let (status, reply) = route(method, path, &body, &threads, &views, &writes);
                    let response = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                        reply.len()
                    );
                    socket.write_all(response.as_bytes()).await.ok();
                    return;
                }
            });
        }
    });
    MockForum { url, writes }
}

fn thread(id: u64, author: &str, title: &str, comments: u32) -> Value {
    json!({"id": id, "author": author, "title": title, "body": "", "created_at": 0, "bumped": id, "comments": comments})
}

fn view(
    id: u64,
    author: &str,
    title: &str,
    body: &str,
    comments: &[(u64, Option<u64>, &str, &str)],
) -> Value {
    let comments: Vec<Value> = comments
        .iter()
        .map(|(cid, parent, who, text)| json!({"id": cid, "thread_id": id, "parent": parent, "author": who, "body": text, "depth": 0}))
        .collect();
    json!({"id": id, "author": author, "title": title, "body": body, "created_at": 0, "bumped": id, "comments": comments})
}

async fn world() -> MockForum {
    let threads = json!([
        thread(1, "Ann", "Spores near Prontera", 1),
        thread(2, "Ann", "Selling Jellopy", 1),
        thread(3, "Mercy", "Spore killed me 34 times", 1),
        thread(4, "Bravo", "Watch out for Poison Spore", 1)
    ]);
    let views = HashMap::from([
        (
            1,
            view(
                1,
                "Ann",
                "Spores near Prontera",
                "Watch out, Spores on prt_fild05 hit hard for low levels.",
                &[(
                    10,
                    None,
                    "Cid",
                    "Are they bad for an Acolyte around level 15?",
                )],
            ),
        ),
        (
            2,
            view(
                2,
                "Ann",
                "Selling Jellopy",
                "100 Jellopy, whisper me.",
                &[(20, None, "Mercy", "How much each?")],
            ),
        ),
        (
            4,
            view(
                4,
                "Bravo",
                "Watch out for Poison Spore",
                "Poison Spore killed me 5 times. Poison Spore is too strong for a level 14.",
                &[(
                    40,
                    None,
                    "Cid",
                    "Good spot for level 15: prt_fild08. New job: Acolyte!",
                )],
            ),
        ),
        (
            3,
            view(
                3,
                "Mercy",
                "Spore killed me 34 times",
                "On prt_fild05 as an Acolyte.",
                &[(
                    30,
                    None,
                    "Dee",
                    "Did you try the fields around Payon instead?",
                )],
            ),
        ),
    ]);
    mock_forum(threads, views).await
}

fn forum(url: &str, llm_url: &str) -> (Forum, mpsc::UnboundedReceiver<Event>) {
    let llm =
        Voice::Llm(Llm::new(llm_url, "local", crate::config::DEFAULT_THINK, &sampling()).unwrap());
    forum_with(url, llm, "http://127.0.0.1:9")
}

fn sampling() -> crate::config::Sampling {
    crate::config::Sampling {
        temperature: 0.3,
        top_p: None,
        top_k: None,
        min_p: None,
        repeat_penalty: None,
        presence_penalty: None,
        frequency_penalty: None,
    }
}

fn forum_with(url: &str, voice: Voice, db_url: &str) -> (Forum, mpsc::UnboundedReceiver<Event>) {
    let (events, rx) = mpsc::unbounded_channel();
    let dir = std::env::temp_dir().join(format!("rro-forum-{}", std::process::id()));
    let forum = Forum {
        api: ForumApi::new(url, String::new(), "Mercy").unwrap(),
        voice,
        knowledge: Knowledge::new(Db::new(db_url, Some(dir.clone())).unwrap()),
        events,
        path: dir.join("mercy.forum.json"),
        memory: ForumMemory::default(),
    };
    (forum, rx)
}

#[tokio::test]
async fn comments_go_only_where_the_bot_read_and_may_speak() {
    let mock = world().await;
    let (mut f, _) = forum(&mock.url, "http://127.0.0.1:9");
    let mut v = Visit {
        threads: f.api.threads(LISTED).await.unwrap(),
        ..Default::default()
    };
    assert!(
        f.comment(&mut v, 1, None, "Spores hurt.")
            .await
            .unwrap_err()
            .contains("read thread 1 first")
    );
    f.read_thread(&mut v, 1).await.unwrap();
    assert!(
        f.comment(&mut v, 1, Some(10), "Try the Payon fields.")
            .await
            .unwrap_err()
            .contains("Payon")
    );
    f.comment(
        &mut v,
        1,
        Some(10),
        "Yes, Spores on prt_fild05 killed me a lot. Avoid them.",
    )
    .await
    .unwrap();
    let again = f
        .comment(
            &mut v,
            1,
            Some(10),
            "Yes, Spores on prt_fild05 killed me a lot. Avoid them.",
        )
        .await;
    assert!(
        again.unwrap_err().contains("last word"),
        "the same comment twice in a turn"
    );
    f.read_thread(&mut v, 2).await.unwrap();
    assert!(
        f.comment(&mut v, 2, None, "Still selling?")
            .await
            .unwrap_err()
            .contains("last word")
    );
    f.read_thread(&mut v, 3).await.unwrap();
    f.comment(&mut v, 3, Some(30), "Not yet, Dee.")
        .await
        .unwrap();
    v.writes = WRITES_PER_VISIT;
    assert!(
        f.comment(&mut v, 3, None, "More.")
            .await
            .unwrap_err()
            .contains("enough"),
        "a few writes per visit"
    );
    let writes = mock.writes.lock().unwrap().clone();
    assert_eq!(
        writes,
        vec![
            "comment on 1 (parent 10): Yes, Spores on prt_fild05 killed me a lot. Avoid them.",
            "comment on 3 (parent 30): Not yet, Dee."
        ]
    );
    assert_eq!(f.memory.replies.get(&1), Some(&1));
}

#[tokio::test]
async fn a_thread_on_a_subject_already_there_is_refused_and_notes_are_lessons() {
    let mock = world().await;
    let (mut f, _) = forum(&mock.url, "http://127.0.0.1:9");
    let mut v = Visit {
        threads: f.api.threads(LISTED).await.unwrap(),
        seen: "Spores on prt_fild05".into(),
        ..Default::default()
    };
    assert!(
        f.post_thread(&mut v, "Spores near Prontera", "They hit hard.")
            .await
            .unwrap_err()
            .contains("thread 1")
    );
    f.post_thread(&mut v, "Spores on prt_fild05", "They hit hard.")
        .await
        .unwrap();
    assert!(
        f.post_thread(&mut v, "Another one", "Text.")
            .await
            .unwrap_err()
            .contains("lately")
    );
    assert!(
        take_note(&mut v, "level 15 is too low for Spores")
            .unwrap_err()
            .contains("numbers")
    );
    take_note(&mut v, "Avoid Spores on prt_fild05").unwrap();
    assert!(
        take_note(&mut v, "Hunt on gef_fild01")
            .unwrap_err()
            .contains("gef_fild01")
    );
    assert_eq!(v.notes, vec!["Avoid Spores on prt_fild05"]);
}

/// A whole visit with the local LLM on the mock forum: `cargo test forum_visit -- --ignored --nocapture`.
#[tokio::test]
#[ignore]
async fn forum_visit_on_the_local_llm() {
    live_visit(true).await;
}

/// The same without a reply waiting: `cargo test forum_quiet -- --ignored --nocapture`.
#[tokio::test]
#[ignore]
async fn forum_quiet_visit_on_the_local_llm() {
    live_visit(false).await;
}

async fn live_visit(replied: bool) {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("forum=debug")
        .with_test_writer()
        .try_init();
    let mock = world().await;
    let (mut f, mut events) = forum(&mock.url, "http://127.0.0.1:8080");
    if replied {
        f.memory.inbox.push_back(Notice {
            event: "reply".into(),
            seq: 1,
            kind: "thread_reply".into(),
            thread_id: 3,
            comment_id: 30,
            from: "Dee".into(),
            body: "Did you try the fields around Payon instead?".into(),
            ..Default::default()
        });
    }
    f.memory.read.insert(3, 0);
    let mut snap = Snapshot {
        kills: 410,
        deaths: 37,
        kills_by_mob: vec![
            ("Poring".into(), 160),
            ("Fabre".into(), 120),
            ("Spore".into(), 2),
        ],
        deaths_by_cause: vec![("Spore".into(), 34)],
        losing_to: vec!["Spore".into()],
        journal: vec![
            "reached base level 15 on prt_fild08".into(),
            "changed job: Novice -> Acolyte".into(),
            "could not open the shop of Ammunition Dealer on prontera, looking for another".into(),
        ],
        notes: vec!["Porings are easy kills".into()],
        ..Default::default()
    };
    snap.obs.me.name = "Mercy".into();
    snap.obs.me.job = "Acolyte".into();
    snap.obs.me.base_level = 15;
    snap.obs.me.job_level = 8;
    snap.obs.map.name = "prontera".into();
    f.memory.note_journal(&snap);
    println!(
        "--- opening ---\n{}",
        f.opening(&snap, &f.api.threads(LISTED).await.unwrap())
    );
    f.visit(&snap).await.unwrap();
    println!("--- writes ---\n{:#?}", mock.writes.lock().unwrap());
    while let Ok(ev) = events.try_recv() {
        println!("--- event ---\n{ev:?}");
    }
    println!(
        "--- pending findings ---\n{:?}",
        f.memory.pending.iter().map(|f| &f.key).collect::<Vec<_>>()
    );
}

/// A snapshot of Mercy in prontera after some hunting, with a finding to share.
fn mercy() -> Snapshot {
    let mut snap = Snapshot {
        kills: 410,
        deaths: 37,
        heal_items: 10,
        kills_by_mob: vec![
            ("Poring".into(), 160),
            ("Fabre".into(), 120),
            ("Spore".into(), 2),
        ],
        deaths_by_cause: vec![("Spore".into(), 34)],
        losing_to: vec!["Spore".into()],
        journal: vec![
            "reached base level 15 on prt_fild08".into(),
            "Spore keeps killing me: leaving it alone for a few levels".into(),
        ],
        ..Default::default()
    };
    snap.obs.me.name = "Mercy".into();
    snap.obs.me.job = "Novice".into();
    snap.obs.me.base_level = 12;
    snap.obs.me.job_level = 8;
    snap.obs.map.name = "prontera".into();
    snap
}

fn reply_from_dee() -> Notice {
    Notice {
        event: "reply".into(),
        seq: 1,
        kind: "thread_reply".into(),
        thread_id: 3,
        comment_id: 30,
        from: "Dee".into(),
        body: "Did you try the fields around Payon instead?".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_decided_visit_answers_reads_posts_and_keeps_lessons() {
    let mock = world().await;
    let s1 = crate::voice::systemone::SystemOne::new(&mock.url).unwrap();
    let (mut f, mut events) = forum_with(&mock.url, Voice::SystemOne(s1), &mock.url);
    f.memory.inbox.push_back(reply_from_dee());
    let snap = mercy();
    f.memory.note_journal(&snap);
    f.visit(&snap).await.unwrap();
    let writes = mock.writes.lock().unwrap().clone();
    assert!(
        writes
            .iter()
            .any(|w| w.starts_with("comment on 3 (parent 30): ")),
        "the reply is answered: {writes:?}"
    );
    assert!(
        writes.iter().any(|w| w.starts_with("thread: ")),
        "a finding becomes a thread: {writes:?}"
    );
    assert!(writes.len() <= WRITES_PER_VISIT);
    for w in &writes {
        let text = w.split_once(": ").map(|(_, t)| t).unwrap_or("");
        let text = text.split(" / ").collect::<Vec<_>>().join(". ");
        assert!(
            !crate::voice::phrase::parse(&text).is_empty(),
            "written from the catalogue: {w}"
        );
    }
    assert!(f.memory.inbox.is_empty());
    let Ok(Event::ForumLessons(lessons)) = events.try_recv() else {
        panic!("no lessons")
    };
    let notes: Vec<String> = lessons.iter().map(|l| l.note.render()).collect();
    assert!(
        notes.contains(&"Avoid Poison Spore while weak".to_owned()),
        "{notes:?}"
    );
    assert!(
        notes.contains(&"Try hunting on prt_fild08".to_owned()),
        "{notes:?}"
    );
    assert!(notes.len() <= NOTES_PER_VISIT);
}

/// A decided visit with the decision model on the mock forum and the live reference:
/// `cargo test forum_decided -- --ignored --nocapture`.
#[tokio::test]
#[ignore]
async fn forum_decided_visit_on_the_decision_model() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("forum=debug,voice=debug")
        .with_test_writer()
        .try_init();
    let mock = world().await;
    let s1 = crate::voice::systemone::SystemOne::new("http://127.0.0.1:9931").unwrap();
    let (mut f, mut events) = forum_with(&mock.url, Voice::SystemOne(s1), "http://127.0.0.1:6904");
    f.memory.inbox.push_back(reply_from_dee());
    let snap = mercy();
    f.memory.note_journal(&snap);
    let started = std::time::Instant::now();
    f.visit(&snap).await.unwrap();
    println!(
        "--- writes ({} ms) ---\n{:#?}",
        started.elapsed().as_millis(),
        mock.writes.lock().unwrap()
    );
    while let Ok(ev) = events.try_recv() {
        println!("--- event ---\n{ev:?}");
    }
}
