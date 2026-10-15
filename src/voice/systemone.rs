//! The decision model on llama.cpp's `/v1/systemone`: it reads a state and answers named, typed questions in one
//! forward pass, with a probability for every option. It writes nothing; the bot builds what it says from the options.
//!
//! Every question of a request is answered on its own: one question never sees the answer to another. The whole
//! request must fit in the server's physical batch (`-ub`), and a choice has at most 255 options.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// A request that takes longer than this is dropped: an answer takes well under a second.
const TIMEOUT: Duration = Duration::from_secs(90);
/// Most options the server takes in a choice.
pub const MAX_OPTIONS: usize = 255;

/// A question about the state.
#[derive(Debug, Clone)]
pub enum Question {
    /// How likely a yes is (0 to 1).
    Noul { instructions: String },
    /// One of the options, as (key, description).
    Choice {
        instructions: String,
        options: Vec<(String, String)>,
    },
}

impl Question {
    pub fn noul(instructions: impl Into<String>) -> Self {
        Question::Noul {
            instructions: instructions.into(),
        }
    }

    pub fn choice(instructions: impl Into<String>, options: Vec<(String, String)>) -> Self {
        Question::Choice {
            instructions: instructions.into(),
            options,
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Question::Noul { instructions } => {
                json!({"type": "noul", "instructions": instructions})
            }
            Question::Choice {
                instructions,
                options,
            } => {
                let criteria: Map<String, Value> = options
                    .iter()
                    .map(|(k, d)| (k.clone(), Value::String(d.clone())))
                    .collect();
                json!({"type": "choice", "instructions": instructions, "criteria": criteria})
            }
        }
    }
}

/// The answer to one question.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Answer {
    /// The option the model rates best, for a choice.
    pub choice: Option<String>,
    /// For a choice: per option key. For a score: per level index.
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: Option<f64>,
    /// For a noul: how likely a yes is.
    pub noul: Option<f64>,
    /// For a score: the expected level (0 = the first).
    pub score: Option<f64>,
}

impl Answer {
    /// The chance of yes of a noul, 0 when the answer has none.
    pub fn yes(&self) -> f64 {
        self.noul.unwrap_or(0.0)
    }

    /// An option drawn by its probability sharpened by `temperature` (lower: closer to the best one). Options far below
    /// the best one are never drawn, so a sentence nobody would say does not come out by luck.
    pub fn sample(&self, temperature: f64) -> Option<String> {
        let best = self.probabilities.values().cloned().fold(0.0, f64::max);
        if best <= 0.0 {
            return self.choice.clone();
        }
        let weighted: Vec<(&String, f64)> = self
            .probabilities
            .iter()
            .filter(|(_, p)| **p >= best * 0.2)
            .map(|(k, p)| (k, (p / best).powf(1.0 / temperature.max(0.05))))
            .collect();
        let total: f64 = weighted.iter().map(|(_, w)| w).sum();
        let mut roll = rand::random::<f64>() * total;
        for (key, w) in &weighted {
            roll -= w;
            if roll <= 0.0 {
                return Some((*key).clone());
            }
        }
        weighted
            .last()
            .map(|(k, _)| (*k).clone())
            .or_else(|| self.choice.clone())
    }
}

#[derive(Clone)]
pub struct SystemOne {
    url: String,
    client: reqwest::Client,
}

impl SystemOne {
    /// `base_url`: the llama.cpp server, like `http://127.0.0.1:9931`.
    pub fn new(base_url: &str) -> Result<Self> {
        let client = reqwest::Client::builder().timeout(TIMEOUT).build()?;
        Ok(Self {
            url: base_url.trim_end_matches('/').to_owned(),
            client,
        })
    }

    /// Whether the server answers `/health`.
    pub async fn healthy(&self) -> bool {
        match self
            .client
            .get(format!("{}/health", self.url))
            .timeout(Duration::from_secs(3))
            .send()
            .await
        {
            Ok(r) => r.status().is_success(),
            Err(_) => false,
        }
    }

    /// Answers the questions about `state`, by name.
    pub async fn ask(
        &self,
        state: &str,
        questions: &[(&str, Question)],
    ) -> Result<BTreeMap<String, Answer>> {
        if questions.is_empty() {
            return Ok(BTreeMap::new());
        }
        for (name, q) in questions {
            if let Question::Choice { options, .. } = q
                && (options.is_empty() || options.len() > MAX_OPTIONS)
            {
                bail!("question {name}: {} options", options.len());
            }
        }
        let qs: Map<String, Value> = questions
            .iter()
            .map(|(name, q)| ((*name).to_owned(), q.to_json()))
            .collect();
        let body = json!({"state": state, "questions": qs});
        let started = std::time::Instant::now();
        let response = self
            .client
            .post(format!("{}/v1/systemone", self.url))
            .json(&body)
            .send()
            .await
            .context("decision model")?;
        let status = response.status();
        let v: Value = response.json().await.context("decision model answer")?;
        if !status.is_success() {
            let message = v["error"]["message"].as_str().unwrap_or("no message");
            return Err(anyhow!("decision model: {status}: {message}"));
        }
        tracing::debug!(
            target: "voice",
            millis = started.elapsed().as_millis() as u64,
            tokens = v["usage"]["input_tokens"].as_u64().unwrap_or(0),
            "decision"
        );
        let answers = v
            .get("answers")
            .cloned()
            .ok_or_else(|| anyhow!("decision model: no answers in {v}"))?;
        serde_json::from_value(answers).context("decision model answers")
    }

    /// One choice, drawn at `temperature`.
    pub async fn choose(
        &self,
        state: &str,
        instructions: &str,
        options: Vec<(String, String)>,
        temperature: f64,
    ) -> Result<String> {
        let answers = self
            .ask(state, &[("pick", Question::choice(instructions, options))])
            .await?;
        answers
            .get("pick")
            .and_then(|a| a.sample(temperature))
            .ok_or_else(|| anyhow!("decision model: no choice"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draw_keeps_to_the_options_near_the_best() {
        let a = Answer {
            choice: Some("a".into()),
            probabilities: BTreeMap::from([
                ("a".into(), 0.6),
                ("b".into(), 0.35),
                ("c".into(), 0.05),
            ]),
            ..Default::default()
        };
        let draws: Vec<String> = (0..300).filter_map(|_| a.sample(1.0)).collect();
        assert!(
            draws.iter().all(|d| d != "c"),
            "an option far below the best is never drawn"
        );
        assert!(draws.iter().any(|d| d == "b") && draws.iter().any(|d| d == "a"));
        assert!(
            (0..50).all(|_| a.sample(0.05).as_deref() == Some("a")),
            "a cold draw takes the best"
        );
    }

    #[test]
    fn answers_parse_from_the_server_shape() {
        let v = json!({
            "speak": {"type": "noul", "noul": 0.37},
            "template": {"type": "choice", "choice": "unseen", "probabilities": {"spotted": 0.1, "unseen": 0.9}, "confidence": 0.28},
            "danger": {"type": "score", "score": 1.36, "legend": {"0": "Safe"}, "probabilities": {"0": 0.1}, "confidence": 0.05}
        });
        let a: BTreeMap<String, Answer> = serde_json::from_value(v).unwrap();
        assert_eq!(a["speak"].yes(), 0.37);
        assert_eq!(a["template"].choice.as_deref(), Some("unseen"));
        assert_eq!(a["danger"].score, Some(1.36));
    }
}
