//! Gemini `generateContent` client over a key pool.
//!
//! No response carries a quota counter: the only quota data is in the body of a
//! 429, in two shapes, one naming its family and one bare. So the pace is learnt
//! from the outcomes rather than configured.
//!
//! Gemini 3.x dropped `temperature`, `top_p` and `top_k`, and replaced
//! `thinkingBudget` with `thinkingLevel`; setting both gives a 400.

use anyhow::{Context, Result};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_tracing::{SpanBackendWithUrl, TracingMiddleware};
use serde_json::json;

use crate::ratelimit::{now_unix, Ledger, Quota};

const API_ROOT: &str = "https://generativelanguage.googleapis.com/v1beta/models";

/// Round trip of a summary call, seconds. A seed: the bucket replaces it with
/// what it observes, and Little turns it into an admission rate.
const LATENCY_SEED: f64 = 2.6;

/// A refusal naming a daily family states a `retryDelay` of a minute all the
/// same, so that path computes the reset itself.
const DAILY_QUOTA_TZ: &str = "America/Los_Angeles";

/// What one key of the free tier may spend on one model. Every ceiling is held
/// per project and per model, so a second model is capacity the first one does
/// not have.
///
/// Only `req_per_day` is a wall, a refusal naming it parking the key until the
/// Pacific reset. The other two are seeds the bucket moves on its own outcomes.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub req_per_day: u32,
    pub req_per_min: f64,
    pub tok_per_min: u32,
}

/// Free-tier ceilings, read off the console rate limit page and not
/// inferred from the name: `gemini-2.5-flash-lite` carries the day of a `flash`,
/// so a rule on the suffix gets it wrong. An unknown model takes the `flash` row,
/// where understating a ceiling costs throughput and overstating it buys refusals.
pub fn limits(model: &str) -> Limits {
    match model {
        "gemini-3.1-flash-lite" | "gemini-3.5-flash-lite" => Limits {
            req_per_day: 500,
            req_per_min: 15.0,
            tok_per_min: 250_000,
        },
        "gemini-2.5-flash-lite" => Limits {
            req_per_day: 20,
            req_per_min: 10.0,
            tok_per_min: 250_000,
        },
        m if m.starts_with("antigravity") => Limits {
            req_per_day: 100,
            req_per_min: 60.0,
            tok_per_min: 100_000,
        },
        _ => Limits {
            req_per_day: 20,
            req_per_min: 5.0,
            tok_per_min: 250_000,
        },
    }
}

/// A pool parked under this waits; past it it hands over. The caller runs under a
/// chain watchdog, where waiting for a saturated pool costs the whole step.
const MAX_WAIT_S: f64 = 45.0;

pub struct GeminiClient {
    http: ClientWithMiddleware,
    keys: Vec<String>,
    /// Same index as `keys`.
    fingerprints: Vec<String>,
    model: String,
    /// `thinkingLevel`, left out when the caller wants the model's own default.
    thinking: Option<String>,
    idx: std::sync::atomic::AtomicUsize,
    /// Same index as `keys`: unix instant before which the key does not emit.
    parked_until: std::sync::Mutex<Vec<f64>>,
    ledger: Ledger,
    refusals: std::sync::atomic::AtomicU64,
    /// `quotaId` of the last rate refusal: the only place naming the ceiling held, and whether it is per key or per project.
    refused_quota: std::sync::Mutex<Option<String>>,
    /// Refusals naming no quota and carrying no `retryDelay`. The bucket ratchets down on
    /// them with nothing to aim at.
    bare_refusals: std::sync::atomic::AtomicU64,
    day: std::sync::Mutex<DayBudget>,
    limits: Limits,
}

/// The day's request count, per key, which no response announces. `charged` is
/// what the store held at the last flush, `pending` what this process has sent
/// since: the budget is their sum, so a bench sharing the keys is seen.
struct DayBudget {
    day: jiff::civil::Date,
    charged: Vec<u32>,
    pending: Vec<u32>,
}

impl DayBudget {
    fn new(n: usize, day: jiff::civil::Date) -> Self {
        Self {
            day,
            charged: vec![0; n],
            pending: vec![0; n],
        }
    }

    /// Zeroes the counters when the Pacific day has rolled under a running process.
    fn roll(&mut self, today: jiff::civil::Date) {
        if self.day == today {
            return;
        }
        self.day = today;
        self.charged.fill(0);
        self.pending.fill(0);
    }

    fn spent(&self, i: usize) -> u32 {
        self.charged[i] + self.pending[i]
    }
}

impl GeminiClient {
    /// `keys` must be non-empty.
    pub fn new(keys: Vec<String>, model: String, thinking: Option<String>) -> Result<Self> {
        if keys.is_empty() {
            anyhow::bail!("GeminiClient: keys must be non-empty");
        }
        let http = ClientBuilder::new(
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .context("gemini: HTTP client")?,
        )
        .with(TracingMiddleware::<SpanBackendWithUrl>::new())
        .build();
        // Two instances do not start on the same key.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as usize)
            .unwrap_or(0);
        let n = keys.len();
        let limits = limits(&model);
        Ok(Self {
            http,
            idx: std::sync::atomic::AtomicUsize::new(seed % n),
            fingerprints: keys
                .iter()
                .map(|k| crate::mistral::key_fingerprint(k))
                .collect(),
            keys,
            model,
            thinking,
            parked_until: std::sync::Mutex::new(vec![0.0; n]),
            refusals: std::sync::atomic::AtomicU64::new(0),
            refused_quota: std::sync::Mutex::new(None),
            bare_refusals: std::sync::atomic::AtomicU64::new(0),
            ledger: Ledger::learned(n, limits.req_per_min, LATENCY_SEED, limits.tok_per_min),
            day: std::sync::Mutex::new(DayBudget::new(n, pacific_today(now_unix()))),
            limits,
        })
    }

    /// Same index as `keys`, and what the store keys the day's count on.
    pub fn fingerprints(&self) -> &[String] {
        &self.fingerprints
    }

    /// Seeds the day from what the store already holds. It reports and paces, it
    /// never closes a key: only a named refusal does.
    pub fn seed_day(&self, spent: &[(String, i32)]) {
        let now = now_unix();
        let mut day = self.day.lock().expect("day mutex not poisoned");
        day.roll(pacific_today(now));
        for (print, count) in spent {
            let Some(i) = self.fingerprints.iter().position(|f| f == print) else {
                continue;
            };
            day.charged[i] = u32::try_from(*count).unwrap_or(0);
        }
        drop(day);
        tracing::info!(
            model = %self.model,
            keys = self.keys.len(),
            budget_left = self.day_left(),
            "gemini_day_budget"
        );
    }

    /// Requests the day still allows across the pool, for this model.
    /// A key parked for the day has nothing left whatever the count says.
    pub fn day_left(&self) -> u32 {
        let now = now_unix();
        let live: Vec<bool> = {
            let parked = self.parked_until.lock().expect("parked mutex not poisoned");
            parked.iter().map(|&until| until <= now).collect()
        };
        let day = self.day.lock().expect("day mutex not poisoned");
        (0..self.keys.len())
            .filter(|&i| live[i])
            .map(|i| self.limits.req_per_day.saturating_sub(day.spent(i)))
            .sum()
    }

    /// What this process has sent and not yet handed to the store, per fingerprint.
    pub fn day_pending(&self) -> Vec<(String, i32)> {
        let day = self.day.lock().expect("day mutex not poisoned");
        (0..self.keys.len())
            .filter(|&i| day.pending[i] > 0)
            .map(|i| (self.fingerprints[i].clone(), day.pending[i] as i32))
            .collect()
    }

    /// Books what the store has just written: `totals` is authoritative, so the
    /// deltas it carried leave `pending` and the rest of it lands in `charged`.
    pub fn day_flushed(&self, flushed: &[(String, i32)], totals: &[(String, i32)]) {
        let mut day = self.day.lock().expect("day mutex not poisoned");
        for (print, delta) in flushed {
            let Some(i) = self.fingerprints.iter().position(|f| f == print) else {
                continue;
            };
            day.pending[i] = day.pending[i].saturating_sub(u32::try_from(*delta).unwrap_or(0));
        }
        for (print, total) in totals {
            let Some(i) = self.fingerprints.iter().position(|f| f == print) else {
                continue;
            };
            day.charged[i] = u32::try_from(*total).unwrap_or(0);
        }
    }

    fn note_served(&self, key_idx: usize, now: f64) {
        let mut day = self.day.lock().expect("day mutex not poisoned");
        day.roll(pacific_today(now));
        day.pending[key_idx] += 1;
    }

    pub fn key_count(&self) -> usize {
        self.keys.len()
    }

    /// Keys that can emit now, in round-robin order, and when the earliest of the
    /// others wakes up.
    fn live_order(&self, now: f64) -> (Vec<usize>, f64) {
        use std::sync::atomic::Ordering::Relaxed;
        let n = self.keys.len();
        let start = self.idx.fetch_add(1, Relaxed) % n;
        let parked = self.parked_until.lock().expect("parked mutex not poisoned");
        let order: Vec<usize> = (0..n)
            .map(|k| (start + k) % n)
            .filter(|&i| parked[i] <= now)
            .collect();
        let soonest = parked.iter().copied().fold(f64::MAX, f64::min);
        (order, soonest)
    }

    pub fn live_key_count(&self) -> usize {
        let now = now_unix();
        self.parked_until
            .lock()
            .expect("parked mutex not poisoned")
            .iter()
            .filter(|&&until| until <= now)
            .count()
    }

    /// Refusals met since the client was built: what the learning cost.
    pub fn refusals(&self) -> u64 {
        self.refusals.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Ceiling the last rate refusal named, `None` while none has come.
    pub fn refused_quota(&self) -> Option<String> {
        self.refused_quota
            .lock()
            .expect("refused_quota mutex not poisoned")
            .clone()
    }

    /// Refusals that named no ceiling, out of [`Self::refusals`].
    pub fn bare_refusals(&self) -> u64 {
        self.bare_refusals
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Calls a minute the pool admits as it stands, which is what its buckets have
    /// converged to.
    pub fn rate_per_min(&self) -> f64 {
        self.ledger.rate_per_min()
    }

    /// Calls in flight the pool justifies holding, and the keys still mute.
    pub fn demand(&self) -> (f64, usize) {
        self.ledger.demand()
    }

    fn park(&self, key_idx: usize, delay_s: f64, quota_id: Option<&str>) {
        self.parked_until.lock().expect("parked mutex not poisoned")[key_idx] =
            now_unix() + delay_s;
        tracing::warn!(
            key = %self.fingerprints[key_idx],
            delay_s,
            quota = quota_id.unwrap_or("?"),
            "gemini_key_parked"
        );
    }

    /// The answer text, thinking parts dropped. `max_output_tokens` bounds the
    /// output; a response cut by that bound is an error rather than a summary
    /// persisted truncated.
    pub async fn generate(
        &self,
        system: &str,
        user: &str,
        max_output_tokens: u32,
    ) -> Result<String, GeminiError> {
        loop {
            match self.generate_now(system, user, max_output_tokens).await {
                Err(GeminiError::Busy(wait)) => {
                    tokio::time::sleep(std::time::Duration::from_secs_f64(wait.max(0.05))).await;
                }
                other => return other,
            }
        }
    }

    /// One admission attempt and no wait, [`GeminiError::Busy`] carrying how long
    /// until a key could pay. What lets a caller hold one queue across several pools
    /// rather than a worker per call in flight.
    pub async fn generate_now(
        &self,
        system: &str,
        user: &str,
        max_output_tokens: u32,
    ) -> Result<String, GeminiError> {
        let payload_chars = system.len() + user.len();
        let now = now_unix();
        let (order, soonest) = self.live_order(now);
        if order.is_empty() {
            let wait = soonest - now;
            return Err(if wait > MAX_WAIT_S {
                GeminiError::Exhausted
            } else {
                GeminiError::Busy(wait)
            });
        }
        let reservation = match self.ledger.reserve(&order, payload_chars, now) {
            Ok(reservation) => reservation,
            Err(wait) => return Err(GeminiError::Busy(wait.as_secs_f64())),
        };
        let key_idx = reservation.key_idx();
        let mut config = json!({"maxOutputTokens": max_output_tokens});
        if let Some(level) = &self.thinking {
            config["thinkingConfig"] = json!({"thinkingLevel": level});
        }
        let payload = json!({
            "systemInstruction": {"parts": [{"text": keyed_system(system, &self.fingerprints[key_idx])}]},
            "contents": [{"parts": [{"text": user}]}],
            "generationConfig": config,
        });
        let url = format!("{API_ROOT}/{}:generateContent", self.model);
        let response = self
            .http
            .post(&url)
            .header("x-goog-api-key", &self.keys[key_idx])
            .json(&payload)
            .send()
            .await
            .map_err(GeminiError::Middleware)?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            let now = now_unix();
            if status == 429 {
                self.refusals
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let refusal = parse_quota_refusal(&body);
                if refusal.per_day() {
                    // A closed day says nothing about the minute, so the rate learns
                    // nothing here: the key simply leaves until the reset.
                    drop(reservation);
                    self.park(
                        key_idx,
                        seconds_to_daily_reset(now),
                        refusal.quota_id.as_deref(),
                    );
                } else {
                    match &refusal.quota_id {
                        Some(id) => {
                            *self
                                .refused_quota
                                .lock()
                                .expect("refused_quota mutex not poisoned") = Some(id.clone())
                        }
                        None => {
                            self.bare_refusals
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    reservation.refused_rate(
                        refusal.per_minute_ceiling(),
                        refusal.retry_delay,
                        now,
                    );
                }
            } else {
                drop(reservation);
            }
            return Err(GeminiError::Status(status));
        }
        self.note_served(key_idx, now_unix());
        let body = response.json::<serde_json::Value>().await;
        reservation.settle(
            Quota::default(),
            billed_tokens(body.as_ref().ok()),
            now_unix(),
        );
        let body = body.map_err(GeminiError::Http)?;
        let candidate = &body["candidates"][0];
        match candidate["finishReason"].as_str() {
            Some("STOP") => Ok(answer_text(candidate)),
            other => Err(GeminiError::Unfinished(
                other.unwrap_or("absent").to_string(),
            )),
        }
    }
}

/// The head of the system instruction is a throttle key shared by every project of
/// the account, so naming the key there gives each project its own. Only the
/// head counts; appending to the tail is refused.
fn keyed_system(system: &str, fingerprint: &str) -> String {
    format!("Réf. interne : {fingerprint}.\n\n{system}")
}

/// Rest for a key out for the day: the refusal's own delay, or an hour, after which
/// it comes back and finds out.
fn pacific(now: f64) -> jiff::Zoned {
    jiff::Timestamp::from_second(now as i64)
        .expect("now_unix within Timestamp range")
        .in_tz(DAILY_QUOTA_TZ)
        .expect("tzdb bundled")
}

fn pacific_today(now: f64) -> jiff::civil::Date {
    pacific(now).date()
}

fn seconds_to_daily_reset(now: f64) -> f64 {
    let here = pacific(now);
    let reset = here
        .tomorrow()
        .and_then(|z| z.start_of_day())
        .expect("next Pacific midnight in range");
    (reset.timestamp().as_second() - here.timestamp().as_second()) as f64
}

/// Tokens billed for a served call, which the ledger uses to correct its cost model.
fn billed_tokens(body: Option<&serde_json::Value>) -> Option<u32> {
    u32::try_from(body?["usageMetadata"]["totalTokenCount"].as_u64()?).ok()
}

/// Answer text: the thinking parts (`thought`) are not part of it.
fn answer_text(candidate: &serde_json::Value) -> String {
    let mut text = String::new();
    for part in candidate["content"]["parts"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if part["thought"].as_bool().unwrap_or(false) {
            continue;
        }
        if let Some(t) = part["text"].as_str() {
            text.push_str(t);
        }
    }
    text.trim().to_string()
}

/// What a refusal says about itself, found wherever the keys sit: they move around
/// inside `error.details` from one quota to the next, and a parser tied to one
/// nesting reads a daily exhaustion as a nameless refusal.
#[derive(Debug, Default, PartialEq)]
struct Refusal {
    quota_id: Option<String>,
    /// `QuotaFailure.violations[].quotaValue`, the ceiling the service states.
    quota_value: Option<f64>,
    /// `RetryInfo.retryDelay`, which the rate families state and the bucket obeys.
    /// A daily one announces a minute all the same, so that path computes its own.
    retry_delay: Option<std::time::Duration>,
}

impl Refusal {
    /// Only a named daily family takes a key out of the pool. Everything else is
    /// minute-scale, the bare `RESOURCE_EXHAUSTED` included, which carries no
    /// `details` at all.
    fn per_day(&self) -> bool {
        self.quota_id
            .as_deref()
            .is_some_and(|q| q.contains("PerDay"))
    }

    /// Calls a minute the refusal states, when it states a per-minute request family.
    /// This is the adapter the ledger needs, and the whole of what it knows of Gemini.
    fn per_minute_ceiling(&self) -> Option<f64> {
        let id = self.quota_id.as_deref()?;
        (id.contains("PerMinute") && id.contains("Requests")).then_some(())?;
        self.quota_value.filter(|v| *v > 0.0)
    }
}

/// `RetryInfo` states a protobuf duration, which on this API is seconds: `"35s"`.
fn parse_retry_delay(raw: &str) -> Option<std::time::Duration> {
    let secs: f64 = raw.trim_end_matches('s').parse().ok()?;
    (secs.is_finite() && secs >= 0.0).then(|| std::time::Duration::from_secs_f64(secs))
}

fn parse_quota_refusal(body: &str) -> Refusal {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return Refusal::default();
    };
    let mut r = Refusal::default();
    let mut stack = vec![&v];
    while let Some(node) = stack.pop() {
        match node {
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    match (key.as_str(), value.as_str()) {
                        ("quotaId", Some(s)) => r.quota_id = Some(s.to_string()),
                        ("quotaValue", Some(s)) => r.quota_value = s.parse().ok(),
                        ("retryDelay", Some(s)) => r.retry_delay = parse_retry_delay(s),
                        _ => stack.push(value),
                    }
                }
            }
            serde_json::Value::Array(items) => stack.extend(items),
            _ => {}
        }
    }
    r
}

#[derive(Debug)]
pub enum GeminiError {
    /// Non-2xx HTTP response.
    Status(u16),
    /// No key can pay yet, and one will in this many seconds. Nothing was sent.
    Busy(f64),
    /// Every key is parked: hand over, do not retry.
    Exhausted,
    /// `finishReason` other than `STOP` (output cut, filtered).
    Unfinished(String),
    Http(reqwest::Error),
    Middleware(reqwest_middleware::Error),
}

impl std::fmt::Display for GeminiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GeminiError::Status(code) => write!(f, "gemini status {code}"),
            GeminiError::Busy(wait) => write!(f, "gemini: pool busy for {wait:.1}s"),
            GeminiError::Exhausted => write!(f, "gemini: every key parked"),
            GeminiError::Unfinished(reason) => write!(f, "gemini finishReason {reason}"),
            GeminiError::Http(e) => write!(f, "gemini transport: {e}"),
            GeminiError::Middleware(e) => write!(f, "gemini transport: {e}"),
        }
    }
}

impl std::error::Error for GeminiError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A per-minute refusal in the shape the API answers it.
    const REFUSAL: &str = r#"{"error":{"code":429,"status":"RESOURCE_EXHAUSTED",
        "message":"Quota exceeded for metric: generate_content_free_tier_requests, limit: 10",
        "details":[
          {"@type":"type.googleapis.com/google.rpc.Help","links":[{"url":"https://ai.google.dev/"}]},
          {"@type":"type.googleapis.com/google.rpc.QuotaFailure","violations":[{
             "quotaMetric":"generativelanguage.googleapis.com/generate_content_free_tier_requests",
             "quotaId":"GenerateRequestsPerMinutePerProjectPerModel-FreeTier",
             "quotaDimensions":{"location":"global","model":"gemini-flash-lite"},
             "quotaValue":"10"}]},
          {"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"30s"}]}}"#;

    fn famille(quota_id: &str) -> Refusal {
        Refusal {
            quota_value: None,
            quota_id: Some(quota_id.to_string()),
            retry_delay: None,
        }
    }

    #[test]
    fn refusal_carries_its_quota_family() {
        let r = parse_quota_refusal(REFUSAL);
        assert_eq!(
            r.quota_id.as_deref(),
            Some("GenerateRequestsPerMinutePerProjectPerModel-FreeTier")
        );
        assert!(!r.per_day());
    }

    /// The second refusal shape, a bare `RESOURCE_EXHAUSTED` with no `details`,
    /// which is what the pool meets under load.
    #[test]
    fn a_nameless_refusal_is_a_minute_not_a_day() {
        let bare = parse_quota_refusal(
            r#"{"error":{"code":429,"message":"Resource has been exhausted (e.g. check quota).",
                "status":"RESOURCE_EXHAUSTED"}}"#,
        );
        assert_eq!(bare, Refusal::default());
        assert!(!bare.per_day());
    }

    /// The local count reports, it does not close: only a served request is
    /// debited, and what takes a key out is the named refusal.
    #[test]
    fn the_local_count_does_not_park_a_key() {
        let day = limits("gemini-3.5-flash-lite").req_per_day;
        let c = GeminiClient::new(
            vec!["k0".into(), "k1".into()],
            "gemini-3.5-flash-lite".into(),
            None,
        )
        .expect("two keys is non-empty");
        let prints = c.fingerprints().to_vec();
        c.seed_day(&[(prints[0].clone(), day as i32)]);
        assert_eq!(c.live_key_count(), 2);
        assert_eq!(c.day_left(), day);
    }

    /// The day is the one ceiling a key is closed on, and the two families are
    /// rationed apart: a `flash` pool sized on the `flash-lite` figure would
    /// claim twenty-five times the capacity it has.
    #[test]
    fn each_family_carries_its_own_day() {
        assert_eq!(limits("gemini-3.5-flash-lite").req_per_day, 500);
        assert_eq!(limits("gemini-3.1-flash-lite").req_per_day, 500);
        assert_eq!(limits("gemini-3.5-flash").req_per_day, 20);
        assert_eq!(limits("gemini-3-flash-preview").req_per_day, 20);
        assert_eq!(limits("antigravity-preview-latest").req_per_day, 100);
    }

    /// The suffix is not the family: this one reads as a `flash-lite` and is
    /// rationed like a `flash`, at a minute of its own.
    #[test]
    fn the_lite_of_the_older_family_is_not_a_volume_door() {
        let old = limits("gemini-2.5-flash-lite");
        assert_eq!((old.req_per_day, old.req_per_min), (20, 10.0));
    }

    /// The store's total wins: another process sharing the keys can spend from the
    /// day between two flushes, and a local count would paper over it.
    #[test]
    fn the_store_total_wins_over_the_local_count() {
        let c = GeminiClient::new(vec!["k0".into()], "gemini-3.5-flash-lite".into(), None)
            .expect("one key is non-empty");
        let p = c.fingerprints()[0].clone();
        c.note_served(0, now_unix());
        c.note_served(0, now_unix());
        assert_eq!(c.day_pending(), vec![(p.clone(), 2)]);
        c.day_flushed(&[(p.clone(), 2)], &[(p.clone(), 7)]);
        assert!(c.day_pending().is_empty());
        assert_eq!(
            c.day_left(),
            limits("gemini-3.5-flash-lite").req_per_day - 7
        );
    }

    /// The rate families state their own wait, and the bucket obeys it rather than a
    /// back-off of its own: 30 s here, 7.5 s at another depth.
    #[test]
    fn a_rate_refusal_carries_its_retry_delay() {
        assert_eq!(
            parse_quota_refusal(REFUSAL).retry_delay,
            Some(std::time::Duration::from_secs(30))
        );
        assert_eq!(
            parse_retry_delay("7.5s"),
            Some(std::time::Duration::from_secs_f64(7.5))
        );
        assert_eq!(parse_retry_delay(""), None);
        assert_eq!(parse_retry_delay("later"), None);
    }

    #[test]
    fn only_a_daily_family_parks_a_key() {
        assert!(famille("GenerateRequestsPerDayPerProjectPerModel-FreeTier").per_day());
        assert!(!famille("GenerateRequestsPerMinutePerProjectPerModel-FreeTier").per_day());
    }

    /// The rest lands on Pacific midnight and follows the offset: a fixed 07:00 UTC
    /// would wake every key an hour early half the year, to be refused again.
    #[test]
    fn a_daily_rest_runs_to_the_next_pacific_midnight() {
        let at = |s: &str| {
            seconds_to_daily_reset(
                s.parse::<jiff::Timestamp>()
                    .expect("literal parses")
                    .as_second() as f64,
            )
        };
        // 21:32 PDT the day before, reset at 07:00Z.
        assert_eq!(at("2026-09-10T04:32:00Z"), 2.0 * 3600.0 + 28.0 * 60.0);
        // 20:32 PST the day before, reset at 08:00Z.
        assert_eq!(at("2026-01-15T04:32:00Z"), 3.0 * 3600.0 + 28.0 * 60.0);
        // A minute past the reset waits the whole next day, never a minute.
        assert_eq!(at("2026-09-10T07:01:00Z"), 24.0 * 3600.0 - 60.0);
    }

    #[test]
    fn refusal_reads_the_same_keys_at_another_depth() {
        let body = r#"{"error":{"status":"RESOURCE_EXHAUSTED","details":[
            {"metadata":{"violations":[{"quotaDimensions":{"model":"flash-lite"},
             "quotaId":"GenerateRequestsPerMinutePerProjectPerModel-FreeTier"}]},
             "info":{"retryDelay":"7.5s"}}]}}"#;
        let r = parse_quota_refusal(body);
        assert_eq!(
            r.quota_id.as_deref(),
            Some("GenerateRequestsPerMinutePerProjectPerModel-FreeTier")
        );
    }

    #[test]
    fn a_refusal_that_says_nothing_reads_as_nothing() {
        assert_eq!(parse_quota_refusal(r#"{"error":{}}"#), Refusal::default());
        assert_eq!(parse_quota_refusal("not json"), Refusal::default());
    }

    #[test]
    fn thought_parts_are_not_the_answer() {
        let candidate = json!({"content": {"parts": [
            {"text": "reasoning", "thought": true},
            {"text": "  Un locataire conteste. "}]}});
        assert_eq!(answer_text(&candidate), "Un locataire conteste.");
    }
}
