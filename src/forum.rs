//! The players' forum, for a bot that plays with the LLM. When the bot arrives in a town it may sit down at the forum
//! for a moment, as a player would between hunts: by chance, or always when someone answered it (the server reports
//! replies on `/forum/ws`). There the model does what it likes with a few tools: read threads, comment, start a
//! thread about what it learned, take notes for its strategy (Σ `notes`), leave.
//!
//! Code keeps it honest and polite: what it writes may name only maps and names it read or saw this visit, it
//! comments only on threads it read, never twice in a row nor more than a few times per thread, and writes a handful of
//! times per visit at most. It runs beside the agent on the snapshots the agent publishes, and never blocks it. What
//! it read and wrote is kept in `state/<name>.forum.json`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use smolgent::{ChatMessage, ChatSession, Tool, ToolCall, ToolDefinition, ToolRegistry};
use tokio::sync::{mpsc, watch};

use crate::api::ApiError;
use crate::api::forum::{
    BODY_MAX, COMMENT_MAX, ForumApi, ForumThread, Notice, TITLE_MAX, ThreadView, forum_text,
};
use crate::behaviors::travel::GO_TOWNS;
use crate::knowledge::Knowledge;
use crate::strategist::llm::{LLM_TIMEOUT, Llm, parse_json_loose};
use crate::strategist::skill_state::note_refusal;
use crate::strategist::{Event, Snapshot};
use crate::voice::Voice;

/// Chance that the bot visits the forum when it arrives in a town, and the least time between two such visits.
/// Replies waiting for it make a visit sure.
const VISIT_CHANCE: f64 = 0.25;
const VISIT_GAP: Duration = Duration::from_secs(10 * 60);
/// Threads fetched on arrival (a new thread must not repeat one of their titles), and the most shown: the latest
/// active ones, without those the bot read that have no new comments since.
const LISTED: u32 = 30;
const SHOWN: usize = 12;
/// Model turns per visit, and what a visit may do at most.
const ROUNDS: usize = 8;
const READS_PER_VISIT: usize = 4;
const WRITES_PER_VISIT: usize = 5;
const NOTES_PER_VISIT: usize = 4;
/// Comments the bot writes per thread: two bots answering each other stop.
const COMMENTS_PER_THREAD: u32 = 3;
/// Least time between two threads the bot starts (the server allows one per 15 minutes).
const THREAD_GAP: Duration = Duration::from_secs(60 * 60);
/// Findings waiting to be shared, replies waiting to be seen.
const PENDING_KEPT: usize = 8;
const INBOX_KEPT: usize = 10;
/// Comments of a thread the model reads: the last ones.
const COMMENTS_READ: usize = 8;
/// Sentences per comment or post.
const SENTENCES: usize = 4;
/// Wait before opening the reply socket again.
const RECONNECT: Duration = Duration::from_secs(30);

/// Something the bot learned that others may use, from a line of its journal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    /// The same finding is shared once.
    pub key: String,
    /// The monster, map or quest it is about: a post that names it shares it.
    pub subject: String,
    /// The journal line.
    pub line: String,
    /// The bot then: level, job, map.
    pub context: String,
}

/// What the bot read and wrote, saved between runs.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct ForumMemory {
    /// Comments of each thread when the bot last read it.
    read: BTreeMap<u64, u32>,
    /// Comments the bot wrote per thread.
    replies: BTreeMap<u64, u32>,
    /// Threads the bot started.
    mine: BTreeSet<u64>,
    /// Keys of the findings shared.
    shared: BTreeSet<String>,
    /// Findings not shared yet, oldest first.
    pending: VecDeque<Finding>,
    /// Replies to the bot not seen on a visit yet.
    inbox: VecDeque<Notice>,
    /// The largest `seq` of the replies received.
    ws_seq: u64,
    /// Unix seconds of the last visit and of the last thread started.
    last_visit: u64,
    last_thread: u64,
    /// Journal lines already looked at.
    #[serde(skip)]
    journal_seen: BTreeSet<String>,
}

impl ForumMemory {
    fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
    }

    /// Takes the findings in new journal lines.
    fn note_journal(&mut self, snap: &Snapshot) {
        for line in &snap.journal {
            if !self.journal_seen.insert(line.clone()) {
                continue;
            }
            let Some(mut finding) = finding_of(line) else {
                continue;
            };
            if self.shared.contains(&finding.key)
                || self.pending.iter().any(|f| f.key == finding.key)
            {
                continue;
            }
            let me = &snap.obs.me;
            finding.context = format!(
                "a level {}/{} {} on {}",
                me.base_level, me.job_level, me.job, snap.obs.map.name
            );
            self.pending.push_back(finding);
            while self.pending.len() > PENDING_KEPT {
                self.pending.pop_front();
            }
        }
    }

    /// A reply from the socket, once.
    fn receive(&mut self, notice: Notice) -> bool {
        if notice.seq <= self.ws_seq {
            return false;
        }
        self.ws_seq = notice.seq;
        self.inbox.push_back(notice);
        while self.inbox.len() > INBOX_KEPT {
            self.inbox.pop_front();
        }
        true
    }

    /// Marks the findings a post names as shared.
    fn shared_in(&mut self, text: &str) {
        let (shared, pending): (Vec<Finding>, Vec<Finding>) = self
            .pending
            .drain(..)
            .partition(|f| mentions(text, &f.subject));
        self.pending = pending.into();
        self.shared.extend(shared.into_iter().map(|f| f.key));
    }

    /// Whether to sit down at the forum, having just arrived in a town.
    fn wants_visit(&self, roll: f64) -> bool {
        !self.inbox.is_empty()
            || (now().saturating_sub(self.last_visit) >= VISIT_GAP.as_secs() && roll < VISIT_CHANCE)
    }
}

/// The finding in a journal line, if it is one worth telling others.
pub fn finding_of(line: &str) -> Option<Finding> {
    let finding = |kind: &str, subject: &str| Finding {
        key: format!("{kind}:{}", subject.to_lowercase()),
        subject: subject.to_owned(),
        line: line.to_owned(),
        context: String::new(),
    };
    if let Some(mob) = line.strip_suffix(" keeps killing me: leaving it alone for a few levels") {
        return Some(finding("danger", mob));
    }
    if let Some(rest) = line.strip_prefix("changed job: ") {
        let job = rest.rsplit(" -> ").next()?;
        return Some(finding("job", job));
    }
    if let Some(quest) = line.strip_suffix(" finished") {
        return Some(finding("quest", quest));
    }
    if let Some((quest, _)) = line.split_once(": stuck at '") {
        return Some(finding("stuck", quest));
    }
    for prefix in ["could not open the shop of ", "cannot reach "] {
        if let Some(rest) = line.strip_prefix(prefix)
            && let Some((npc, rest)) = rest.split_once(" on ")
        {
            let map = rest.split([',', ' ']).next()?;
            return Some(Finding {
                subject: map.to_owned(),
                ..finding("shop", &format!("{npc} on {map}"))
            });
        }
    }
    if let Some(rest) = line.strip_prefix("reached base level ")
        && let Some((level, map)) = rest.split_once(" on ")
        && level.parse::<u32>().is_ok_and(|l| l % 5 == 0)
        // Towns and inner maps say nothing about hunting
        && (map.contains("_fild") || map.contains("_dun"))
    {
        return Some(finding("hunt", map));
    }
    None
}

/// Starts the forum task and the watch of the replies to the bot. Notes taken there go to the strategist as
/// `Event::ForumNotes`.
pub fn spawn(
    api: ForumApi,
    voice: Voice,
    knowledge: Knowledge,
    snapshots: watch::Receiver<Option<Snapshot>>,
    events: mpsc::UnboundedSender<Event>,
    state_dir: &Path,
) {
    let path = state_dir.join(format!("{}.forum.json", api.name().to_lowercase()));
    let memory = ForumMemory::load(&path);
    let (notices_tx, notices) = mpsc::unbounded_channel();
    tokio::spawn(listen(api.clone(), memory.ws_seq, notices_tx));
    let forum = Forum {
        api,
        voice,
        knowledge,
        events,
        path,
        memory,
    };
    tokio::spawn(forum.run(snapshots, notices));
}

/// Keeps the reply socket open, opening it again when it closes.
async fn listen(api: ForumApi, mut after: u64, notices: mpsc::UnboundedSender<Notice>) {
    loop {
        match api.listen(after, &notices).await {
            Ok(seq) => after = seq,
            Err(error) => {
                tracing::debug!(target: "forum", error = format!("{error:#}"), "forum socket")
            }
        }
        if notices.is_closed() {
            return;
        }
        tokio::time::sleep(RECONNECT).await;
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

struct Forum {
    api: ForumApi,
    voice: Voice,
    knowledge: Knowledge,
    events: mpsc::UnboundedSender<Event>,
    path: PathBuf,
    memory: ForumMemory,
}

/// One sitting at the forum.
#[derive(Default)]
struct Visit {
    /// Everything the bot saw: what it may name in what it writes.
    seen: String,
    /// What the bot did itself: what it may say it did.
    own: String,
    /// Tool calls refused, as `name args`: the model tends to send the same one again.
    refused: BTreeSet<String>,
    threads: Vec<ForumThread>,
    /// The threads read, by id.
    read: BTreeMap<u64, ThreadView>,
    writes: usize,
    notes: Vec<String>,
    /// What the bot did, for the log.
    did: Vec<String>,
}

impl Forum {
    async fn run(
        mut self,
        mut snapshots: watch::Receiver<Option<Snapshot>>,
        mut notices: mpsc::UnboundedReceiver<Notice>,
    ) {
        let mut last_map: Option<String> = None;
        let mut in_town = false;
        loop {
            tokio::select! {
                Some(notice) = notices.recv() => {
                    tracing::info!(target: "forum", from = %notice.from, thread = notice.thread_id, "reply on the forum");
                    if self.memory.receive(notice) {
                        self.save();
                    }
                }
                changed = snapshots.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let Some(snap) = snapshots.borrow_and_update().clone() else { continue };
                    self.memory.note_journal(&snap);
                    let map = snap.obs.map.name.clone();
                    if last_map.as_deref() == Some(map.as_str()) {
                        continue;
                    }
                    // Into a town or one of its houses from outside; the first map seen is no arrival
                    let first = last_map.is_none();
                    last_map = Some(map.clone());
                    let was_in_town = in_town;
                    in_town = self.in_town(&map).await;
                    if !first && in_town && !was_in_town && self.memory.wants_visit(rand::random()) {
                        if let Err(error) = self.visit(&snap).await {
                            tracing::warn!(target: "forum", error = format!("{error:#}"), "forum visit failed");
                        }
                        self.memory.last_visit = now();
                        self.save();
                    }
                }
            }
        }
    }

    fn save(&self) {
        if let Err(error) = self.memory.save(&self.path) {
            tracing::warn!(target: "forum", %error, "saving the forum memory failed");
        }
    }

    async fn is_town(&self, map: &str) -> bool {
        GO_TOWNS.contains(&map) || self.knowledge.db.map(map).await.is_ok_and(|m| m.is_town())
    }

    /// A town, or a house of one (prt_in, alberta_in): no monsters, and a warp out to a town. Bots often end a step
    /// inside, at a shop or a quest NPC.
    async fn in_town(&self, map: &str) -> bool {
        if self.is_town(map).await {
            return true;
        }
        let Ok(data) = self.knowledge.db.map(map).await else {
            return false;
        };
        if !data.mob_spawns.is_empty() {
            return false;
        }
        for to in data
            .warps
            .iter()
            .map(|w| w.to.map.id_str())
            .filter(|to| to != map)
        {
            if self.is_town(&to).await {
                return true;
            }
        }
        false
    }

    /// One sitting, with the voice of the bot.
    async fn visit(&mut self, snap: &Snapshot) -> Result<()> {
        match self.voice.clone() {
            Voice::Llm(llm) => self.visit_llm(snap, &llm).await,
            Voice::SystemOne(s1) => self.visit_decided(snap, &s1).await,
        }
    }

    /// One sitting: the model reads, writes and takes notes with the tools until it leaves.
    async fn visit_llm(&mut self, snap: &Snapshot, llm: &Llm) -> Result<()> {
        let mut v = Visit {
            threads: self.api.threads(LISTED).await?,
            ..Default::default()
        };
        let opening = self.opening(snap, &v.threads);
        v.seen.push_str(&opening);
        v.own = format!(
            "{}
{}",
            snap.obs.map.name,
            own_facts(snap)
        );
        for f in &self.memory.pending {
            v.own.push_str(&format!(
                "
{} {}",
                f.context,
                happened(snap, f)
            ));
        }
        tracing::info!(target: "forum", map = %snap.obs.map.name, replies = self.memory.inbox.len(), "at the forum");
        let mut session = ChatSession::with_system_prompt(VISIT_SYSTEM.to_owned());
        session.push_user(opening);
        let tools = tools();
        let mut nudged = false;
        let mut force = false;
        'visit: for _ in 0..ROUNDS {
            let provider = if std::mem::take(&mut force) {
                &llm.forced
            } else {
                &llm.free
            };
            let response =
                tokio::time::timeout(LLM_TIMEOUT, session.complete_with_tools(provider, &tools))
                    .await
                    .map_err(|_| anyhow!("LLM timed out"))?
                    .map_err(|e| anyhow!("LLM request failed: {e}"))?;
            let calls = response.message.tool_calls.clone();
            tracing::debug!(
                target: "forum",
                thought = response.message.reasoning_content.as_deref().map(str::len).unwrap_or(0),
                said = %response.message.content.text(),
                calls = calls.len(),
                "turn"
            );
            if calls.is_empty() {
                // The model tends to write its comment as its answer, which posts nothing: once, it is told so and
                // must call a tool next
                if nudged {
                    break;
                }
                nudged = true;
                force = true;
                session.push_user(NUDGE.to_owned());
                continue;
            }
            let mut leave = false;
            for call in &calls {
                let result = match call.function.name.as_str() {
                    "leave" => {
                        leave = true;
                        "You leave the forum.".to_owned()
                    }
                    _ => self.use_tool(&mut v, call).await,
                };
                tracing::debug!(target: "forum", tool = %call.function.name, args = %call.function.arguments, %result, "tool");
                session.push(ChatMessage::tool_result(
                    &call.id,
                    &call.function.name,
                    result,
                ));
            }
            if leave {
                break 'visit;
            }
        }
        self.memory.inbox.clear();
        if !v.notes.is_empty() {
            // Marked as hearsay for the strategist
            self.events
                .send(Event::ForumNotes(
                    v.notes.iter().map(|n| format!("forum: {n}")).collect(),
                ))
                .ok();
        }
        tracing::info!(target: "forum", did = ?v.did, notes = ?v.notes, "left the forum");
        Ok(())
    }

    async fn use_tool(&mut self, v: &mut Visit, call: &ToolCall) -> String {
        let key = format!("{} {}", call.function.name, call.function.arguments);
        if v.refused.contains(&key) {
            return "error: that was refused already: do something else, or leave".into();
        }
        let args = parse_json_loose(&call.function.arguments).unwrap_or(Value::Null);
        let text = |key: &str| {
            args.get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_owned()
        };
        let id = |key: &str| {
            args.get(key).and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            })
        };
        let result = match call.function.name.as_str() {
            "read_thread" => match id("thread_id") {
                Some(thread) => self.read_thread(v, thread).await,
                None => Err("give the thread_id of a thread in THREADS".to_owned()),
            },
            "comment" => match id("thread_id") {
                Some(thread) => self.comment(v, thread, id("reply_to"), &text("body")).await,
                None => Err("give the thread_id of a thread you read".to_owned()),
            },
            "post_thread" => self.post_thread(v, &text("title"), &text("body")).await,
            "take_note" => take_note(v, &text("note")),
            other => Err(format!("there is no tool {other}")),
        };
        result.unwrap_or_else(|e| {
            v.refused.insert(key);
            format!("error: {e}")
        })
    }

    async fn read_thread(&mut self, v: &mut Visit, id: u64) -> Result<String, String> {
        if let Some(view) = v.read.get(&id) {
            return Ok(thread_text(view));
        }
        if v.read.len() >= READS_PER_VISIT {
            return Err("you have read enough for this visit".into());
        }
        let view = self.api.thread(id).await.map_err(|e| format!("{e:#}"))?;
        self.memory.read.insert(id, view.comment_list.len() as u32);
        let text = thread_text(&view);
        v.seen.push_str(&text);
        // What the bot wrote before is what it did
        let name = self.api.name();
        if view.thread.author.eq_ignore_ascii_case(name) {
            v.own
                .push_str(&format!("\n{} {}", view.thread.title, view.thread.body));
        }
        for c in view
            .comment_list
            .iter()
            .filter(|c| c.author.eq_ignore_ascii_case(name))
        {
            v.own.push_str(&format!("\n{}", c.body));
        }
        v.did.push(format!("read {id}"));
        v.read.insert(id, view);
        Ok(text)
    }

    async fn comment(
        &mut self,
        v: &mut Visit,
        thread: u64,
        reply_to: Option<u64>,
        body: &str,
    ) -> Result<String, String> {
        self.may_comment(v, thread)?;
        let body = forum_text(&first_sentences(body, SENTENCES), COMMENT_MAX, false);
        if body.is_empty() {
            return Err("the comment is empty".into());
        }
        check_names(&body, &v.seen)?;
        check_claims(&body, &v.own)?;
        self.send_comment(v, thread, reply_to, &body).await
    }

    /// Whether the bot may comment on `thread` now: it read it, the last word is not its own, and it has not said
    /// too much there nor on this visit.
    fn may_comment(&self, v: &Visit, thread: u64) -> Result<(), String> {
        if v.writes >= WRITES_PER_VISIT {
            return Err("you wrote enough for this visit".into());
        }
        let Some(view) = v.read.get(&thread) else {
            return Err(format!("read thread {thread} first with read_thread"));
        };
        let name = self.api.name();
        let last = view
            .comment_list
            .last()
            .map(|c| &c.author)
            .unwrap_or(&view.thread.author);
        if last.eq_ignore_ascii_case(name) {
            return Err("the last word in that thread is yours: wait for an answer".into());
        }
        if self.memory.replies.get(&thread).copied().unwrap_or(0) >= COMMENTS_PER_THREAD {
            return Err("you said enough in that thread".into());
        }
        Ok(())
    }

    /// Posts a comment `may_comment` allowed; `reply_to` is kept when it is a comment of the thread.
    async fn send_comment(
        &mut self,
        v: &mut Visit,
        thread: u64,
        reply_to: Option<u64>,
        body: &str,
    ) -> Result<String, String> {
        let parent = reply_to.filter(|p| {
            v.read
                .get(&thread)
                .is_some_and(|view| view.comment_list.iter().any(|c| c.id == *p))
        });
        match self.api.comment(thread, body, parent).await {
            Ok(comment) => {
                // The thread as it is now: the last word is the bot's
                if let Some(view) = v.read.get_mut(&thread) {
                    view.comment_list.push(comment.clone());
                }
                *self.memory.replies.entry(thread).or_default() += 1;
                *self.memory.read.entry(thread).or_default() += 1;
                self.memory.shared_in(body);
                v.writes += 1;
                v.did.push(format!("commented on {thread}: {body}"));
                Ok(format!("Your comment {} is posted.", comment.id))
            }
            Err(error) => Err(refusal(&error)),
        }
    }

    async fn post_thread(
        &mut self,
        v: &mut Visit,
        title: &str,
        body: &str,
    ) -> Result<String, String> {
        self.may_post(v)?;
        let title = forum_text(title, TITLE_MAX, true);
        let body = forum_text(&first_sentences(body, SENTENCES), BODY_MAX, false);
        if title.is_empty() || body.is_empty() {
            return Err("a thread needs a title and a body".into());
        }
        if let Some(same) = v.threads.iter().find(|t| similar(&t.title, &title)) {
            return Err(format!(
                "thread {} \"{}\" is about that: read it and comment there",
                same.id, same.title
            ));
        }
        check_names(&format!("{title}. {body}"), &v.seen)?;
        check_claims(&body, &v.own)?;
        self.send_thread(v, &title, &body).await
    }

    /// Whether the bot may start a thread now.
    fn may_post(&self, v: &Visit) -> Result<(), String> {
        if v.writes >= WRITES_PER_VISIT {
            return Err("you wrote enough for this visit".into());
        }
        if now().saturating_sub(self.memory.last_thread) < THREAD_GAP.as_secs() {
            return Err("you started a thread lately: comment on one instead".into());
        }
        Ok(())
    }

    async fn send_thread(
        &mut self,
        v: &mut Visit,
        title: &str,
        body: &str,
    ) -> Result<String, String> {
        match self.api.post_thread(title, body).await {
            Ok(thread) => {
                self.memory.mine.insert(thread.id);
                self.memory.read.insert(thread.id, 0);
                self.memory.last_thread = now();
                self.memory.shared_in(&format!("{title} {body}"));
                v.writes += 1;
                v.did
                    .push(format!("started {} \"{title}\": {body}", thread.id));
                Ok(format!("Your thread {} is posted.", thread.id))
            }
            Err(error) => Err(refusal(&error)),
        }
    }

    /// What the bot sees when it sits down: the replies to it, the threads, its notes and what it knows.
    fn opening(&self, snap: &Snapshot, threads: &[ForumThread]) -> String {
        let me = &snap.obs.me;
        let name = self.api.name();
        let mut o = format!(
            "You are {name}, a level {}/{} {}. You just arrived in {} and sit down at the players' forum for a moment.\n\n",
            me.base_level, me.job_level, me.job, snap.obs.map.name
        );
        o.push_str("REPLIES TO YOU:\n");
        if self.memory.inbox.is_empty() {
            o.push_str("- none\n");
        }
        for n in &self.memory.inbox {
            let title = threads
                .iter()
                .find(|t| t.id == n.thread_id)
                .map(|t| format!(" \"{}\"", t.title))
                .unwrap_or_default();
            let _ = writeln!(
                o,
                "- {} answered you in thread {}{title}, comment {}: \"{}\"",
                n.from,
                n.thread_id,
                n.comment_id,
                forum_text(&n.body, 300, true)
            );
        }
        o.push_str(
            "\nTHREADS, latest activity first (those you read with nothing new left out):\n",
        );
        let fresh: Vec<&ForumThread> = threads
            .iter()
            .filter(|t| self.memory.read.get(&t.id).is_none_or(|n| t.comments > *n))
            .take(SHOWN)
            .collect();
        if threads.is_empty() {
            o.push_str("- none yet\n");
        } else if fresh.is_empty() {
            o.push_str("- nothing new since you last read them\n");
        }
        for t in fresh {
            let yours = if t.author.eq_ignore_ascii_case(name) {
                ", yours"
            } else {
                ""
            };
            let mark = if self.memory.read.contains_key(&t.id) {
                "new comments"
            } else {
                "new"
            };
            let _ = writeln!(
                o,
                "- thread {}: \"{}\" by {}, {} comments ({mark}{yours})",
                t.id,
                forum_text(&t.title, 150, true),
                t.author,
                t.comments
            );
        }
        o.push_str("\nYOUR NOTES:\n");
        if snap.notes.is_empty() {
            o.push_str("- none\n");
        }
        for note in &snap.notes {
            let _ = writeln!(o, "- {note}");
        }
        o.push_str("\nYOUR OWN EXPERIENCE:\n");
        o.push_str(&own_facts(snap));
        if !self.memory.pending.is_empty() {
            o.push_str("\n\nTHINGS YOU LEARNED THAT OTHERS COULD USE:\n");
            for f in &self.memory.pending {
                let _ = writeln!(o, "- when you were {}: {}", f.context, happened(snap, f));
            }
        }
        o
    }
}

fn take_note(v: &mut Visit, note: &str) -> Result<String, String> {
    if v.notes.len() >= NOTES_PER_VISIT {
        return Err("you took enough notes for this visit".into());
    }
    if let Some(why) = note_refusal(note) {
        return Err(why);
    }
    check_names(note, &v.seen)?;
    if !v.notes.iter().any(|n| n.eq_ignore_ascii_case(note)) {
        v.notes.push(note.to_owned());
        v.did.push(format!("noted: {note}"));
    }
    Ok("Noted.".into())
}

/// What the forum's refusal of a write tells the model.
fn refusal(error: &anyhow::Error) -> String {
    match ApiError::status_of(error).map(|s| s.as_u16()) {
        Some(429) => "the forum asks you to wait before writing again".into(),
        _ => format!("{error:#}"),
    }
}

/// Refuses a text that names maps or names the bot did not read or see this visit.
fn check_names(text: &str, seen: &str) -> Result<(), String> {
    let unknown: Vec<String> = names(text)
        .into_iter()
        .filter(|n| !mentions(seen, n))
        .collect();
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "nothing you read or know mentions {}: write only what you know",
            unknown.join(", ")
        ))
    }
}

/// Words that make a sentence say what the bot did.
const DID: [&str; 16] = [
    "tried", "checked", "went", "hunted", "visited", "did", "found", "saw", "explored", "been",
    "leveled", "levelled", "farmed", "fought", "killed", "bought",
];

/// Refuses a sentence where the bot says it did something ("I tried the fields around Payon") at a place or with a
/// monster its own experience does not name: the small model makes up its past from what it read.
fn check_claims(text: &str, own: &str) -> Result<(), String> {
    for sentence in text.split_inclusive(['.', '!', '?']) {
        let words: Vec<String> = sentence
            .split_whitespace()
            .map(|w| {
                w.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase()
            })
            .collect();
        let claims = words.iter().enumerate().any(|(i, w)| {
            (w == "i" || w == "i've" || w == "ive")
                && words[i + 1..]
                    .iter()
                    .take(3)
                    .any(|n| DID.contains(&n.as_str()))
        });
        if !claims {
            continue;
        }
        let unknown: Vec<String> = names(sentence)
            .into_iter()
            .filter(|n| !mentions(own, n))
            .collect();
        if !unknown.is_empty() {
            return Err(format!(
                "YOUR OWN EXPERIENCE does not say you did anything with {}: say only what you did, or who told you",
                unknown.join(", ")
            ));
        }
    }
    Ok(())
}

/// Whether two thread titles are about the same: one holds the other.
fn similar(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == ' ')
            .collect::<String>()
            .trim()
            .to_owned()
    };
    let (a, b) = (norm(a), norm(b));
    !a.is_empty() && !b.is_empty() && (a == b || a.contains(&b) || b.contains(&a))
}

const VISIT_SYSTEM: &str = "You play a character in the game Ragnarok Online (pre-renewal). Between hunts you visit \
the players' forum for a moment, as a player would. Use the tools:
- read_thread: read a thread. Read one before you comment on it.
- comment: write in a thread you read; reply_to answers one comment. Answer the REPLIES TO YOU first.
- post_thread: start a thread to share something from YOUR OWN EXPERIENCE or THINGS YOU LEARNED.
- take_note: keep a short lesson from what you read that will change how you play: a map to try or to avoid, a \
monster to avoid, where to buy something, how a quest goes. No numbers except map names.
- leave: when you are done.
Nothing forces you to write: reading what interests you and leaving is fine. When THINGS YOU LEARNED has something no \
thread is about yet, other players would like a thread on it. Write plainly and briefly, like a player. \
YOUR OWN EXPERIENCE is all you have done: you have not been anywhere or tried anything it does not say, so never write \
that you did. When you repeat what someone wrote, say who wrote it. Never make up maps, monsters or numbers. Never share passwords, keys or real-life details. If someone asks \
whether you are a bot, answer honestly.";

const NUDGE: &str = "(Only you can see your answers. To write on the forum, call comment or post_thread with just the words for the other players; take_note keeps a lesson; leave when you are done.)";

/// The forum tools; the visit runs them itself, so the executors do nothing.
fn tools() -> ToolRegistry {
    let def = |name: &str, description: &str, parameters: Value| {
        Tool::new(ToolDefinition::new(name, description, parameters), |_| {
            Box::pin(async { Ok(Value::Null) })
        })
    };
    let thread_id =
        json!({"type": "integer", "description": "A thread id from THREADS or REPLIES TO YOU"});
    ToolRegistry::new()
        .with_tool(def(
            "read_thread",
            "Read a thread: its post and its last comments, with their ids.",
            json!({"type": "object", "properties": {"thread_id": thread_id}, "required": ["thread_id"]}),
        ))
        .with_tool(def(
            "comment",
            "Write a comment in a thread you read. reply_to: the id of the comment you answer, if any.",
            json!({
                "type": "object",
                "properties": {
                    "thread_id": thread_id,
                    "reply_to": {"type": "integer"},
                    "body": {"type": "string", "maxLength": 400}
                },
                "required": ["thread_id", "body"]
            }),
        ))
        .with_tool(def(
            "post_thread",
            "Start a thread: a title naming what it is about, and a body of a few sentences.",
            json!({
                "type": "object",
                "properties": {"title": {"type": "string", "maxLength": 80}, "body": {"type": "string", "maxLength": 600}},
                "required": ["title", "body"]
            }),
        ))
        .with_tool(def(
            "take_note",
            "Keep a short lesson for how you play, at most 100 characters.",
            json!({"type": "object", "properties": {"note": {"type": "string", "maxLength": 100}}, "required": ["note"]}),
        ))
        .with_tool(def("leave", "Leave the forum.", json!({"type": "object", "properties": {}})))
}

/// A finding in plain words, with the numbers the bot has: a journal line is written for the bot, not for a reader.
fn happened(snap: &Snapshot, finding: &Finding) -> String {
    let count = |list: &[(String, u64)]| {
        list.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(&finding.subject))
            .map(|(_, n)| *n)
            .unwrap_or(0)
    };
    match finding.key.split(':').next().unwrap_or("") {
        "danger" => format!(
            "{} killed you {} times, and you killed only {} of them. You stopped hunting {} until you gain a few levels.",
            finding.subject,
            count(&snap.deaths_by_cause),
            count(&snap.kills_by_mob),
            finding.subject
        ),
        "shop" => {
            format!(
                "{}: the shop window never opened, so you bought elsewhere.",
                finding.line.split(',').next().unwrap_or(&finding.line)
            )
        }
        "hunt" => format!("{}: you hunted there to level up.", finding.line),
        "stuck" => format!(
            "{}. You could not finish that step and left the quest for a while.",
            finding.line
        ),
        _ => finding.line.clone(),
    }
}

/// What the bot knows from playing: kills, deaths and recent happenings.
fn own_facts(snap: &Snapshot) -> String {
    let top = |m: &[(String, u64)]| {
        m.iter()
            .take(5)
            .map(|(k, v)| format!("{k} {v}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut lines = vec![
        format!("- kills: {} ({})", snap.kills, top(&snap.kills_by_mob)),
        format!("- deaths: {} ({})", snap.deaths, top(&snap.deaths_by_cause)),
    ];
    if !snap.losing_to.is_empty() {
        lines.push(format!(
            "- monsters that keep killing you: {}",
            snap.losing_to.join(", ")
        ));
    }
    lines.extend(
        snap.journal
            .iter()
            .rev()
            .take(6)
            .rev()
            .map(|l| format!("- {l}")),
    );
    lines.join("\n")
}

/// The first `n` sentences of a text.
fn first_sentences(text: &str, n: usize) -> String {
    let mut end = text.len();
    let mut seen = 0;
    for (i, c) in text.char_indices() {
        // "1." starts a list item, it ends no sentence
        let numbered = c == '.'
            && text[..i]
                .chars()
                .next_back()
                .is_some_and(|d| d.is_ascii_digit());
        if matches!(c, '.' | '!' | '?')
            && !numbered
            && text[i + 1..].chars().next().is_none_or(char::is_whitespace)
        {
            seen += 1;
            if seen == n {
                end = i + 1;
                break;
            }
        }
    }
    text[..end].to_owned()
}

/// The names in a text: map ids (`prt_fild08`) and capitalised words inside a sentence (a monster, a town, an NPC).
fn names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut sentence_start = true;
    for word in text.split_whitespace() {
        // "Spore-infested" names Spore
        for (i, part) in word.split(['-', '/', '—', '–']).enumerate() {
            let clean = part.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
            let is_name = clean.contains('_')
                || (!(sentence_start && i == 0)
                    && clean.len() >= 4
                    && clean.chars().next().is_some_and(char::is_uppercase)
                    && !clean.contains('\''));
            if is_name {
                out.push(clean.to_owned());
            }
        }
        sentence_start = word.ends_with(['.', '!', '?', ':']);
    }
    out
}

/// Whether `source` mentions a name, plural or not.
fn mentions(source: &str, name: &str) -> bool {
    let (source, name) = (source.to_lowercase(), name.to_lowercase());
    source.contains(&name)
        || name
            .strip_suffix('s')
            .is_some_and(|n| n.len() >= 4 && source.contains(n))
}

/// A thread as the model reads it: the title, the post and the last comments with their ids.
fn thread_text(view: &ThreadView) -> String {
    let t = &view.thread;
    let short = |s: &str, n: usize| forum_text(s, n, false);
    let mut text = format!(
        "Thread {}: {} wrote \"{}\":\n{}\n",
        t.id,
        t.author,
        short(&t.title, 150),
        short(&t.body, 1200)
    );
    let skip = view.comment_list.len().saturating_sub(COMMENTS_READ);
    if skip > 0 {
        text.push_str(&format!("({skip} older comments not shown)\n"));
    }
    for c in &view.comment_list[skip..] {
        let to = c
            .parent
            .map(|p| format!(", answering comment {p}"))
            .unwrap_or_default();
        text.push_str(&format!(
            "[comment {}] {}{to}: {}\n",
            c.id,
            c.author,
            short(&c.body, 400)
        ));
    }
    text
}

mod decided;
#[cfg(test)]
mod tests;
