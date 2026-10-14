//! A forum visit with the decision model: the bot answers the replies to it, picks threads to read, may add a
//! sentence or two of the catalogue to each, may start a thread about something it learned, and keeps lessons from
//! the catalogue sentences others wrote. The model makes every choice; what is written is built from what the bot
//! knows, so the text checks of the LLM visit are not needed. The write rules (`may_comment`, `may_post`) are the same.

use super::*;
use crate::social::facts;
use crate::voice::known::{Known, Lesson, journal_phrases, lessons_of};
use crate::voice::phrase::{Category, Phrase, parse};
use crate::voice::systemone::{Question, SystemOne};
use crate::voice::{compose, state_of};

/// The decision model's chance of yes above which the bot writes, or keeps a lesson.
const WRITE_GATE: f64 = 0.5;
const LESSON_GATE: f64 = 0.5;
/// Lessons put to the model per visit, at most.
const LESSONS_ASKED: usize = 12;
/// What a comment may be: no greeting or goodbye on a forum.
const COMMENT_KINDS: [Category; 5] = [
    Category::Reply,
    Category::Lore,
    Category::Warning,
    Category::News,
    Category::Request,
];

impl Forum {
    pub(super) async fn visit_decided(&mut self, snap: &Snapshot, s1: &SystemOne) -> Result<()> {
        let mut v = Visit {
            threads: self.api.threads(LISTED).await?,
            ..Default::default()
        };
        let name = self.api.name().to_owned();
        let mut known = Known::from_snapshot(snap);
        let mut lessons: Vec<Lesson> = Vec::new();
        tracing::info!(target: "forum", map = %snap.obs.map.name, replies = self.memory.inbox.len(), "at the forum");
        // The replies to the bot first
        let inbox: Vec<Notice> = self.memory.inbox.iter().cloned().collect();
        let mut answered = BTreeSet::new();
        for n in inbox {
            if !answered.insert(n.thread_id) || self.read_thread(&mut v, n.thread_id).await.is_err()
            {
                continue;
            }
            learn(&v, n.thread_id, &name, &known, &mut lessons);
            let said = forum_text(&n.body, 200, true);
            let gate = format!(
                "{} wrote to {name}: \"{said}\". Should {name} write back?",
                n.from
            );
            let purpose = format!("answer {}, who wrote to {name}: \"{said}\"", n.from);
            self.maybe_comment(
                s1,
                &mut v,
                &mut known,
                n.thread_id,
                Some(n.comment_id),
                &gate,
                &purpose,
            )
            .await?;
        }
        // Then the threads that look worth it
        while v.read.len() < READS_PER_VISIT {
            let unread: Vec<&ForumThread> = v
                .threads
                .iter()
                .filter(|t| {
                    self.memory.read.get(&t.id).is_none_or(|n| t.comments > *n)
                        && !v.read.contains_key(&t.id)
                })
                .take(SHOWN)
                .collect();
            if unread.is_empty() {
                break;
            }
            let mut options: Vec<(String, String)> = unread
                .iter()
                .map(|t| {
                    (
                        format!("t{}", t.id),
                        format!(
                            "\"{}\" by {}, {} comments",
                            forum_text(&t.title, 150, true),
                            t.author,
                            t.comments
                        ),
                    )
                })
                .collect();
            options.push((
                "leave".into(),
                "Leave the forum: nothing more is worth reading".into(),
            ));
            let state = format!(
                "{}\n{name} sits at the players' forum in town for a moment, between hunts.",
                state_of(&known)
            );
            let pick = s1
                .choose(
                    &state,
                    &format!("Which thread would {name} read now, if any?"),
                    options,
                    0.7,
                )
                .await?;
            let Some(id) = pick.strip_prefix('t').and_then(|i| i.parse::<u64>().ok()) else {
                break;
            };
            if self.read_thread(&mut v, id).await.is_err() {
                break;
            }
            learn(&v, id, &name, &known, &mut lessons);
            let gate = format!("Does {name} have something useful to add to this thread?");
            self.maybe_comment(
                s1,
                &mut v,
                &mut known,
                id,
                None,
                &gate,
                "add something useful to the thread",
            )
            .await?;
        }
        self.maybe_post(s1, &mut v, &known).await?;
        let kept = keep_lessons(s1, &known, lessons).await?;
        self.memory.inbox.clear();
        let notes: Vec<String> = kept.iter().map(|l| l.note.render()).collect();
        if !kept.is_empty() {
            self.events.send(Event::ForumLessons(kept)).ok();
        }
        tracing::info!(target: "forum", did = ?v.did, ?notes, "left the forum");
        Ok(())
    }

    /// Comments on a thread read this visit, when the rules allow it and the model sees something worth adding.
    async fn maybe_comment(
        &mut self,
        s1: &SystemOne,
        v: &mut Visit,
        known: &mut Known,
        thread: u64,
        reply_to: Option<u64>,
        gate: &str,
        purpose: &str,
    ) -> Result<()> {
        if let Err(why) = self.may_comment(v, thread) {
            tracing::debug!(target: "forum", thread, %why, "no comment");
            return Ok(());
        }
        let Some(view) = v.read.get(&thread) else {
            return Ok(());
        };
        let text = thread_text(view);
        // What the reference says about what the thread names: the bot may tell it
        known.lore = facts::lore(&self.knowledge, &text).await;
        let state = format!(
            "{}\n\nThe thread {} reads:\n{text}",
            state_of(known),
            self.api.name()
        );
        let answers = s1.ask(&state, &[("write", Question::noul(gate))]).await?;
        let yes = answers.get("write").map(|a| a.yes()).unwrap_or(0.0);
        tracing::debug!(target: "forum", thread, yes, "comment?");
        if yes < WRITE_GATE {
            return Ok(());
        }
        // Nothing the thread says already
        let lower = text.to_lowercase();
        let candidates: Vec<Phrase> = known
            .candidates()
            .into_iter()
            .filter(|p| !lower.contains(&p.render().to_lowercase()))
            .collect();
        let Some(message) = compose(
            s1,
            &state,
            purpose,
            &candidates,
            Some(&COMMENT_KINDS),
            false,
        )
        .await?
        else {
            return Ok(());
        };
        let body = forum_text(&message.text, COMMENT_MAX, false);
        // Typed words are not true by construction: they may name only what the bot read or knows
        if message.typed
            && let Err(why) = check_names(&body, &state)
        {
            tracing::debug!(target: "forum", thread, %why, %body, "typed comment refused");
            return Ok(());
        }
        if let Err(why) = self.send_comment(v, thread, reply_to, &body).await {
            tracing::debug!(target: "forum", thread, %why, "comment refused");
        }
        Ok(())
    }

    /// Starts a thread about one of the findings no thread is about yet, when the model picks one.
    async fn maybe_post(&mut self, s1: &SystemOne, v: &mut Visit, known: &Known) -> Result<()> {
        if self.may_post(v).is_err() {
            return Ok(());
        }
        let posts: Vec<(Phrase, Vec<Phrase>)> = self
            .memory
            .pending
            .iter()
            .filter_map(|f| post_of(f, known))
            .filter(|(title, _)| !v.threads.iter().any(|t| similar(&t.title, &title.render())))
            .collect();
        if posts.is_empty() {
            return Ok(());
        }
        let mut options: Vec<(String, String)> = posts
            .iter()
            .enumerate()
            .map(|(i, (title, _))| (format!("p{i}"), title.render()))
            .collect();
        options.push(("none".into(), "Start no thread".into()));
        let state = format!(
            "{}\nNo thread on the forum is about these things {} learned yet.",
            state_of(known),
            self.api.name()
        );
        let pick = s1
            .choose(
                &state,
                "Which thread would other players like to read, if any?",
                options,
                0.5,
            )
            .await?;
        let Some((title, body)) = pick
            .strip_prefix('p')
            .and_then(|i| i.parse::<usize>().ok())
            .and_then(|i| posts.get(i))
        else {
            return Ok(());
        };
        let title = forum_text(&title.render(), TITLE_MAX, true);
        let body: Vec<String> = body
            .iter()
            .map(|p| format!("{}.", p.render().trim_end_matches(['.', '!'])))
            .collect();
        let body = forum_text(&body.join(" "), BODY_MAX, false);
        if let Err(why) = self.send_thread(v, &title, &body).await {
            tracing::debug!(target: "forum", %why, "thread refused");
        }
        Ok(())
    }
}

/// Takes the lessons for the bot in the catalogue sentences others wrote in a thread read.
fn learn(v: &Visit, thread: u64, me: &str, known: &Known, lessons: &mut Vec<Lesson>) {
    let Some(view) = v.read.get(&thread) else {
        return;
    };
    let mut texts: Vec<String> = Vec::new();
    if !view.thread.author.eq_ignore_ascii_case(me) {
        texts.push(format!("{}. {}", view.thread.title, view.thread.body));
    }
    texts.extend(
        view.comment_list
            .iter()
            .filter(|c| !c.author.eq_ignore_ascii_case(me))
            .map(|c| c.body.clone()),
    );
    for text in texts {
        for phrase in parse(&text) {
            for lesson in lessons_of(&phrase, known.level, &known.job) {
                if !lessons.iter().any(|l| l.note == lesson.note) {
                    lessons.push(lesson);
                }
            }
        }
    }
}

/// The lessons the model wants to keep, at most `NOTES_PER_VISIT`, those it is surest of first.
async fn keep_lessons(
    s1: &SystemOne,
    known: &Known,
    mut lessons: Vec<Lesson>,
) -> Result<Vec<Lesson>> {
    lessons.retain(|l| {
        !known
            .notes
            .iter()
            .any(|n| n.to_lowercase().ends_with(&l.note.render().to_lowercase()))
    });
    lessons.truncate(LESSONS_ASKED);
    if lessons.is_empty() {
        return Ok(Vec::new());
    }
    let keys: Vec<String> = (0..lessons.len()).map(|i| format!("l{i}")).collect();
    let questions: Vec<(&str, Question)> = keys
        .iter()
        .zip(&lessons)
        .map(|(k, l)| {
            (k.as_str(), Question::noul(format!("Other players wrote this on the forum. Should {} keep it in mind for how it plays: \"{}\"?", known.me, l.note.render())))
        })
        .collect();
    let answers = s1.ask(&state_of(known), &questions).await?;
    let mut rated: Vec<(f64, Lesson)> = keys
        .iter()
        .zip(lessons)
        .map(|(k, l)| (answers.get(k).map(|a| a.yes()).unwrap_or(0.0), l))
        .filter(|(yes, _)| *yes >= LESSON_GATE)
        .collect();
    rated.sort_by(|a, b| b.0.total_cmp(&a.0));
    Ok(rated
        .into_iter()
        .take(NOTES_PER_VISIT)
        .map(|(_, l)| l)
        .collect())
}

/// A finding as a thread: a title and a body, from what the bot knows.
fn post_of(f: &Finding, known: &Known) -> Option<(Phrase, Vec<Phrase>)> {
    let p = Phrase::new;
    let level = known.level.to_string();
    let me = p("me_level", &[&level, &known.job]);
    match f.key.split(':').next().unwrap_or("") {
        "danger" => {
            let deaths = known.died_to(&f.subject);
            let mut body = Vec::new();
            if deaths > 0 {
                body.push(p("killed_me", &[&f.subject, &deaths.to_string()]));
            }
            body.push(p("too_strong", &[&f.subject, &level]));
            Some((p("watch_out", &[&f.subject]), body))
        }
        "job" => Some((p("new_job", &[&f.subject]), vec![me])),
        "quest" => Some((p("finished", &[&f.subject]), vec![me])),
        "stuck" => Some((
            p("stuck", &[&f.subject]),
            vec![p("help_quest", &[&f.subject]), me],
        )),
        "shop" | "hunt" => {
            let mut phrases = journal_phrases(&f.line);
            if phrases.is_empty() {
                return None;
            }
            let title = phrases.remove(phrases.len() - 1);
            let body = if phrases.is_empty() {
                vec![title.clone()]
            } else {
                phrases
            };
            Some((title, body))
        }
        _ => None,
    }
}
