//! Client for the rust-ro bot REST API (see `bots/SKILL.md` on the server).

use std::fmt;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::action::Action;
use super::observation::Observation;

/// A non-2xx answer from the server. The server's `{"error": "..."}` message is kept when there is one,
/// otherwise the raw body.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.status, self.message)
    }
}

impl std::error::Error for ApiError {}

impl ApiError {
    /// The status of an `ApiError` inside an `anyhow::Error`, if it is one.
    pub fn status_of(error: &anyhow::Error) -> Option<StatusCode> {
        error.downcast_ref::<ApiError>().map(|e| e.status)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct BotSummary {
    pub name: String,
    #[serde(default)]
    pub connected: bool,
}

#[derive(Clone)]
pub struct BotApi {
    http: reqwest::Client,
    base: String,
    key: String,
}

impl BotApi {
    pub fn new(base: &str, key: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            // Actions may wait up to `bots.action_timeout_secs` (30 s) on the server.
            .timeout(Duration::from_secs(60))
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            key,
        })
    }

    pub async fn list(&self) -> Result<Vec<BotSummary>> {
        let value = self.send(self.http.get(self.url("/bots"))).await?;
        serde_json::from_value(value).context("parsing bot list")
    }

    /// Creates a new character and puts it in the game.
    pub async fn create(&self, name: &str) -> Result<BotSummary> {
        let req = self
            .http
            .post(self.url("/bots"))
            .json(&json!({ "name": name }));
        let value = self.send(req).await?;
        serde_json::from_value(value).context("parsing created bot")
    }

    /// Puts an existing bot back in the game.
    pub async fn connect(&self, name: &str) -> Result<BotSummary> {
        let req = self.http.post(self.url(&format!("/bots/{name}/connect")));
        let value = self.send(req).await?;
        serde_json::from_value(value).context("parsing connected bot")
    }

    /// Takes the bot out of the game (it keeps everything; `connect` brings it back).
    pub async fn disconnect(&self, name: &str) -> Result<Value> {
        let req = self
            .http
            .post(self.url(&format!("/bots/{name}/disconnect")));
        self.send(req).await
    }

    pub async fn observation(&self, name: &str) -> Result<Observation> {
        let req = self
            .http
            .get(self.url(&format!("/bots/{name}/observation")));
        let value = self.send(req).await?;
        serde_json::from_value(value).context("parsing observation")
    }

    pub async fn settings(&self) -> Result<Value> {
        self.send(self.http.get(self.url("/bots/server/settings")))
            .await
    }

    /// Runs one action and returns the server's result.
    pub async fn act(&self, name: &str, action: &Action) -> Result<Value> {
        tracing::debug!(action = ?action, "act");
        let req = self
            .http
            .post(self.url(&format!("/bots/{name}/actions")))
            .json(action);
        // No context wrapper: its Display would hide the server's reason, and callers know the action.
        self.send(req).await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<Value> {
        let resp = req
            .bearer_auth(&self.key)
            .send()
            .await
            .context("sending request to the bot API")?;
        let status = resp.status();
        let text = resp.text().await.context("reading response body")?;

        if status.is_success() {
            return serde_json::from_str(&text).context("server sent invalid JSON");
        }
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_owned))
            .unwrap_or(text);
        Err(ApiError { status, message }.into())
    }
}
