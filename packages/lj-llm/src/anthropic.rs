//! Claude through the Message Batches API: half the price of a call, the answer
//! within the hour as a rule and never past a day.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_tracing::{SpanBackendWithUrl, TracingMiddleware};
use serde::Deserialize;
use serde_json::{json, Value};

const BATCHES: &str = "https://api.anthropic.com/v1/messages/batches";
const VERSION: &str = "2023-06-01";

/// A summary is short; the cap leaves room for the reasoning at `medium`, many times longer.
const MAX_TOKENS: u32 = 16_000;

/// Every request of a batch carries the same system prompt, cached for the hour.
pub struct BatchRequest<'a> {
    pub custom_id: String,
    pub model: &'a str,
    pub effort: &'a str,
    pub system: &'a str,
    pub user: String,
}

impl BatchRequest<'_> {
    fn to_json(&self) -> Value {
        json!({
            "custom_id": self.custom_id,
            "params": {
                "model": self.model,
                "max_tokens": MAX_TOKENS,
                "output_config": {"effort": self.effort},
                "system": [{
                    "type": "text",
                    "text": self.system,
                    "cache_control": {"type": "ephemeral", "ttl": "1h"},
                }],
                "messages": [{"role": "user", "content": self.user}],
            },
        })
    }

    /// Bytes the request adds to a batch body, whose size the API caps.
    pub fn wire_len(&self) -> usize {
        self.to_json().to_string().len() + 1
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub output_tokens_details: OutputDetails,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct OutputDetails {
    #[serde(default)]
    pub thinking_tokens: u64,
}

#[derive(Debug)]
pub struct Answer {
    pub text: String,
    pub stop_reason: String,
    pub usage: Usage,
}

#[derive(Debug)]
pub enum Outcome {
    Succeeded(Answer),
    Errored(String),
    Canceled,
    Expired,
}

#[derive(Debug)]
pub struct BatchResult {
    pub custom_id: String,
    pub outcome: Outcome,
}

#[derive(Debug, Deserialize)]
pub struct RequestCounts {
    pub processing: u64,
    pub succeeded: u64,
    pub errored: u64,
    pub canceled: u64,
    pub expired: u64,
}

#[derive(Debug, Deserialize)]
pub struct Batch {
    pub id: String,
    pub processing_status: String,
    pub request_counts: RequestCounts,
    pub results_url: Option<String>,
}

impl Batch {
    pub fn ended(&self) -> bool {
        self.processing_status == "ended"
    }
}

pub struct AnthropicBatches {
    http: ClientWithMiddleware,
    key: String,
}

impl AnthropicBatches {
    pub fn new(key: String) -> Result<Self> {
        let plain = reqwest::Client::builder()
            .timeout(Duration::from_secs(900))
            .build()?;
        Ok(Self {
            http: ClientBuilder::new(plain)
                .with(TracingMiddleware::<SpanBackendWithUrl>::new())
                .build(),
            key,
        })
    }

    fn request(&self, method: reqwest::Method, url: &str) -> reqwest_middleware::RequestBuilder {
        self.http
            .request(method, url)
            .header("x-api-key", &self.key)
            .header("anthropic-version", VERSION)
    }

    async fn read(response: reqwest::Response) -> Result<String> {
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            bail!(
                "anthropic HTTP {status}: {}",
                text.chars().take(500).collect::<String>()
            );
        }
        Ok(text)
    }

    pub async fn create(&self, requests: &[BatchRequest<'_>]) -> Result<Batch> {
        let mut body = String::from("{\"requests\":[");
        for (i, request) in requests.iter().enumerate() {
            if i > 0 {
                body.push(',');
            }
            body.push_str(&request.to_json().to_string());
        }
        body.push_str("]}");
        let response = self
            .request(reqwest::Method::POST, BATCHES)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await?;
        Ok(serde_json::from_str(&Self::read(response).await?)?)
    }

    pub async fn retrieve(&self, id: &str) -> Result<Batch> {
        let response = self
            .request(reqwest::Method::GET, &format!("{BATCHES}/{id}"))
            .send()
            .await?;
        Ok(serde_json::from_str(&Self::read(response).await?)?)
    }

    pub async fn results(&self, batch: &Batch) -> Result<Vec<BatchResult>> {
        let url = batch
            .results_url
            .as_deref()
            .with_context(|| format!("batch {} has no results_url", batch.id))?;
        let response = self.request(reqwest::Method::GET, url).send().await?;
        Self::read(response)
            .await?
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(parse_result)
            .collect()
    }
}

fn parse_result(line: &str) -> Result<BatchResult> {
    let v: Value = serde_json::from_str(line)?;
    let custom_id = v["custom_id"]
        .as_str()
        .context("batch result without custom_id")?
        .to_string();
    let result = &v["result"];
    let outcome = match result["type"].as_str() {
        Some("succeeded") => {
            let message = &result["message"];
            let text = message["content"]
                .as_array()
                .context("succeeded result without content")?
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<String>();
            Outcome::Succeeded(Answer {
                text,
                stop_reason: message["stop_reason"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                usage: serde_json::from_value(message["usage"].clone())?,
            })
        }
        Some("errored") => Outcome::Errored(result["error"].to_string()),
        Some("canceled") => Outcome::Canceled,
        Some("expired") => Outcome::Expired,
        other => bail!("batch result of unknown type {other:?}"),
    };
    Ok(BatchResult { custom_id, outcome })
}
