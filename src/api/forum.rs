//! Client for the forum of the rust-ro server (`/forum` on the bot API port, see `forum/SKILL.md` on the server).
//!
//! A bot writes with the bot API key and `X-Forum-As: <its name>`: the server takes the key as any character, the one
//! named. Reading needs the same credentials.

use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use super::client::ApiError;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ForumThread {
    pub id: u64,
    pub author: String,
    pub title: String,
    pub body: String,
    pub created_at: i64,
    /// Its place in the viewing order, higher is newer.
    pub bumped: u64,
    /// Every comment in it, replies included.
    pub comments: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ForumComment {
    pub id: u64,
    pub thread_id: u64,
    /// The comment this one answers, `None` for a comment on the thread.
    pub parent: Option<u64>,
    pub author: String,
    pub body: String,
    pub created_at: i64,
    pub depth: u32,
}

/// A thread with its comments, in the order they were written.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ThreadView {
    #[serde(flatten)]
    pub thread: ForumThread,
    #[serde(rename = "comments")]
    pub comment_list: Vec<ForumComment>,
}

/// A reply to the bot, from `/forum/ws`: a comment on a thread it started, or an answer to a comment it wrote.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notice {
    pub event: String,
    pub seq: u64,
    /// `thread_reply` or `comment_reply`.
    pub kind: String,
    pub thread_id: u64,
    pub comment_id: u64,
    pub parent: Option<u64>,
    pub from: String,
    pub body: String,
    pub created_at: i64,
}

/// Longest title, body and comment the server takes, in characters.
pub const TITLE_MAX: usize = 150;
pub const BODY_MAX: usize = 10_000;
pub const COMMENT_MAX: usize = 2_000;

#[derive(Clone)]
pub struct ForumApi {
    http: reqwest::Client,
    base: String,
    key: String,
    /// The character the bot writes as.
    name: String,
}

impl ForumApi {
    pub fn new(base: &str, key: String, name: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            key,
            name: name.to_owned(),
        })
    }

    /// The threads on top of the viewing order, newest first.
    pub async fn threads(&self, limit: u32) -> Result<Vec<ForumThread>> {
        self.send(
            self.http
                .get(self.url(&format!("/forum/threads?limit={}", limit.clamp(1, 100)))),
        )
        .await
    }

    pub async fn thread(&self, id: u64) -> Result<ThreadView> {
        self.send(self.http.get(self.url(&format!("/forum/threads/{id}"))))
            .await
    }

    pub async fn post_thread(&self, title: &str, body: &str) -> Result<ForumThread> {
        let req = self
            .http
            .post(self.url("/forum/threads"))
            .json(&json!({ "title": title, "body": body }));
        self.send(req).await
    }

    /// A comment on the thread, or a reply to the comment `parent` of it.
    pub async fn comment(
        &self,
        thread: u64,
        body: &str,
        parent: Option<u64>,
    ) -> Result<ForumComment> {
        let mut payload = json!({ "body": body });
        if let Some(parent) = parent {
            payload["parent"] = json!(parent);
        }
        self.send(
            self.http
                .post(self.url(&format!("/forum/threads/{thread}/comments")))
                .json(&payload),
        )
        .await
    }

    /// The character the bot writes as.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Watches the replies to the bot on `/forum/ws` until the socket closes, sending each to `notices`. The server
    /// first sends every notice after `after`. Returns the largest `seq` sent.
    pub async fn listen(
        &self,
        mut after: u64,
        notices: &mpsc::UnboundedSender<Notice>,
    ) -> Result<u64> {
        let base = self
            .base
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        let mut request = format!("{base}/forum/ws?after={after}").into_client_request()?;
        request.headers_mut().insert(
            "X-API-Key",
            self.key.parse().context("the key is no header value")?,
        );
        request.headers_mut().insert(
            "X-Forum-As",
            self.name.parse().context("the name is no header value")?,
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(request)
            .await
            .context("opening the forum socket")?;
        while let Some(message) = socket.next().await {
            match message.context("reading the forum socket")? {
                Message::Text(text) => {
                    let Ok(notice) = serde_json::from_str::<Notice>(&text) else {
                        continue;
                    };
                    if notice.event == "reply" && notice.seq > after {
                        after = notice.seq;
                        if notices.send(notice).is_err() {
                            break;
                        }
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        Ok(after)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    async fn send<T: DeserializeOwned>(&self, req: reqwest::RequestBuilder) -> Result<T> {
        let resp = req
            .header("X-API-Key", &self.key)
            .header("X-Forum-As", &self.name)
            .send()
            .await
            .context("sending request to the forum")?;
        let status = resp.status();
        let text = resp.text().await.context("reading response body")?;
        if status.is_success() {
            return serde_json::from_str(&text).context("the forum sent unexpected JSON");
        }
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_owned))
            .unwrap_or(text);
        Err(ApiError { status, message }.into())
    }
}

/// Text the forum takes: no control characters but line breaks and tabs, trimmed, at most `max` characters, cut at the
/// last full sentence when it was longer.
pub fn forum_text(text: &str, max: usize, one_line: bool) -> String {
    let text: String = text
        .chars()
        .map(|c| {
            if one_line && c.is_whitespace() {
                ' '
            } else {
                c
            }
        })
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max).collect();
    match cut.rfind(['.', '!', '?']) {
        Some(end) if end > max / 2 => cut[..=end].to_owned(),
        _ => cut.trim_end().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_view_keeps_the_count_and_the_comments_apart() {
        let v: ThreadView = serde_json::from_str(
            r#"{"id": 4, "author": "Ann", "title": "Spores", "body": "Avoid them", "created_at": 1, "bumped": 9,
                "comments": [{"id": 7, "thread_id": 4, "parent": null, "author": "Bob", "body": "why?", "created_at": 2, "depth": 0}]}"#,
        )
        .unwrap();
        assert_eq!(v.thread.title, "Spores");
        assert_eq!(v.comment_list.len(), 1);
        assert_eq!(v.comment_list[0].parent, None);
    }

    #[test]
    fn forum_text_is_cleaned_and_cut() {
        assert_eq!(forum_text("  Hi\u{7}\nthere ", 50, true), "Hi there");
        assert_eq!(forum_text("One.\nTwo.", 50, false), "One.\nTwo.");
        assert_eq!(
            forum_text("A long first sentence. And then more words", 30, false),
            "A long first sentence."
        );
    }
}
