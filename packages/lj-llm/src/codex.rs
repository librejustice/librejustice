//! Live quota of a ChatGPT account: the usage windows, and the credits that reset
//! one before its term. Undocumented surface, read off the codex source
//! (`backend-client/src/client.rs`, `client/rate_limit_resets.rs`): a
//! base on `chatgpt.com` takes the `/wham` paths, and a header names the account.

use anyhow::{bail, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;

const BASE: &str = "https://chatgpt.com/backend-api/wham";

/// Length of the window a ChatGPT plan shows as weekly.
pub const WEEKLY_WINDOW_SECS: i64 = 7 * 24 * 3600;

/// One quota window: how much of it is spent, and when it comes back.
#[derive(Debug, Deserialize)]
pub struct Window {
    pub used_percent: f64,
    pub limit_window_seconds: i64,
    pub reset_after_seconds: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct Limits {
    pub limit_reached: Option<bool>,
    pub primary_window: Option<Window>,
    pub secondary_window: Option<Window>,
}

#[derive(Debug, Deserialize)]
pub struct CreditsSummary {
    #[serde(default)]
    pub available_count: i64,
}

#[derive(Debug, Deserialize)]
pub struct Usage {
    pub plan_type: Option<String>,
    pub rate_limit: Option<Limits>,
    pub rate_limit_reset_credits: Option<CreditsSummary>,
}

impl Usage {
    /// The windows the account carries, shortest first.
    pub fn windows(&self) -> Vec<&Window> {
        let Some(limits) = &self.rate_limit else {
            return Vec::new();
        };
        let mut windows: Vec<&Window> = [&limits.primary_window, &limits.secondary_window]
            .into_iter()
            .flatten()
            .collect();
        windows.sort_by_key(|w| w.limit_window_seconds);
        windows
    }

    /// The window of that length, the weekly one being `WEEKLY_WINDOW_SECS`.
    pub fn window(&self, seconds: i64) -> Option<&Window> {
        self.windows()
            .into_iter()
            .find(|w| w.limit_window_seconds == seconds)
    }

    pub fn credits_available(&self) -> i64 {
        self.rate_limit_reset_credits
            .as_ref()
            .map_or(0, |c| c.available_count)
    }
}

/// A credit that resets a window. It expires, so one left unspent is lost.
#[derive(Debug, Deserialize)]
pub struct ResetCredit {
    pub id: String,
    pub reset_type: Option<String>,
    pub status: Option<String>,
    pub expires_at: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ResetCredits {
    #[serde(default)]
    pub credits: Vec<ResetCredit>,
    #[serde(default)]
    pub available_count: i64,
}

pub async fn usage(http: &reqwest::Client, access_token: &str, account: &str) -> Result<Usage> {
    read(http.get(format!("{BASE}/usage")), access_token, account).await
}

pub async fn reset_credits(
    http: &reqwest::Client,
    access_token: &str,
    account: &str,
) -> Result<ResetCredits> {
    read(
        http.get(format!("{BASE}/rate-limit-reset-credits")),
        access_token,
        account,
    )
    .await
}

/// Burns one credit. `redeem_request_id` is the idempotency key: the same value
/// twice is one redemption, so a retry carries the one its attempt was given.
pub async fn consume_reset_credit(
    http: &reqwest::Client,
    access_token: &str,
    account: &str,
    redeem_request_id: &str,
) -> Result<serde_json::Value> {
    read(
        http.post(format!("{BASE}/rate-limit-reset-credits/consume"))
            .json(&serde_json::json!({ "redeem_request_id": redeem_request_id })),
        access_token,
        account,
    )
    .await
}

async fn read<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
    access_token: &str,
    account: &str,
) -> Result<T> {
    let response = request
        .bearer_auth(access_token)
        .header("ChatGPT-Account-Id", account)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        bail!("chatgpt {status} : {body}");
    }
    Ok(serde_json::from_str(&body)?)
}
