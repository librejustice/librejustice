//! Summaries through a ChatGPT subscription seat, on the codex responses route.
//!
//! The ceiling is neither requests nor tokens but value spent in a rolling
//! window, counted per SEAT and not per workspace: seats sharing an
//! `account_id` consume separately. The caller hands the seats in the order it
//! wants them served and this file keeps it, a refused seat being parked until
//! the reset it announces. An even rotation would waste the capacity of a seat
//! whose window expires while the others keep theirs to the end of the week.
//!
//! Two properties of the route decide the shape below. It refuses `stream:
//! false`, so the body arrives as server-sent events and is read whole rather
//! than incrementally, the answer being one short summary. And it caches the
//! prefix only for a stable routing key: under `prompt_cache_key`, never under a
//! fresh `session_id`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_tracing::{SpanBackendWithUrl, TracingMiddleware};
use serde_json::{json, Value};

use crate::ratelimit::{now_unix, Ledger, Quota, Reservation};
use crate::{codex, oauth};

const BASE: &str = "https://chatgpt.com/backend-api/codex/responses";

/// Longest wait a caller is asked to hold rather than being handed back its job.
/// Above the minute a throttle parks every seat for when a batch opens all at once.
const MAX_WAIT_S: f64 = 90.0;

/// Seeds of the bucket. They only keep the first wave from being blind; the
/// bucket learns from the outcomes.
const SEED_RATE_PER_MIN: f64 = 60.0;
const SEED_LATENCY_S: f64 = 8.0;
/// Never corrected, the door announcing no token counter, so it is a ceiling and
/// not a seed. It sits above the service's own throttle of a seat, which paces
/// the rest.
const SEED_TOKENS_PER_MIN: u32 = 2_000_000;

/// Park of a throttled seat that states no `retry-after`: the wait the one throttle
/// quoted in the Codex issues asked for.
const THROTTLE_PARK_S: f64 = 60.0;

/// Between two reads of a capped seat's windows, and the park of one whose windows
/// could not be read.
const CAP_CHECK_S: f64 = 30.0;

/// What the store hands over for one seat.
#[derive(Debug, Clone)]
pub struct LunaSeat {
    pub fingerprint: String,
    pub account_id: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Share of each window the seat may fill, its owner keeping the rest.
    pub cap_percent: Option<f64>,
}

/// A refreshed seat, for the caller to persist: OpenAI rotates the refresh token
/// on use, so one kept only in memory dies with the process.
#[derive(Debug, Clone)]
pub struct Rotation {
    pub fingerprint: String,
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Debug)]
pub enum LunaError {
    /// No seat can pay yet; the wait is how long until one could.
    Busy(f64),
    /// Every seat is inside its window, so the pool takes no further job.
    Exhausted,
    Status(u16),
    Transport(String),
    Body(String),
}

impl std::fmt::Display for LunaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy(wait) => write!(f, "luna occupé, {wait:.1} s"),
            Self::Exhausted => write!(f, "tous les sièges luna sont dans leur fenêtre"),
            Self::Status(code) => write!(f, "luna HTTP {code}"),
            Self::Transport(e) => write!(f, "luna transport : {e}"),
            Self::Body(e) => write!(f, "luna réponse : {e}"),
        }
    }
}

impl std::error::Error for LunaError {}

struct Tokens {
    access: String,
    refresh: String,
}

pub struct LunaClient {
    http: ClientWithMiddleware,
    /// The token exchange sits on `auth.openai.com` and takes no span of ours.
    plain: reqwest::Client,
    /// Same index as `fingerprints`, rotated in place on a 401.
    tokens: Vec<Mutex<Tokens>>,
    fingerprints: Vec<String>,
    accounts: Vec<String>,
    /// Same index as `fingerprints`: unix instant before which the seat is mute.
    parked_until: Mutex<Vec<f64>>,
    ledger: Ledger,
    refusals: AtomicU64,
    rotations: Mutex<Vec<Rotation>>,
    /// Same index as `fingerprints`. The door answers a percentage of a window
    /// and never a value, so a seat's cost is knowable only by counting it here.
    served: Vec<AtomicU64>,
    spent: Vec<[AtomicU64; 4]>,
    /// Same index as `fingerprints`.
    caps: Vec<Option<f64>>,
    /// Same index as `fingerprints`: unix instant of the last read of a capped seat.
    cap_checked_at: Mutex<Vec<f64>>,
}

/// Tokens of one answer, split the way the public price splits them: `cached` is
/// the share of `input` read from the prefix cache, billed at a discount, `cache_write`
/// the share written to it, billed at a premium on the platform.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub cached: u64,
    pub cache_write: u64,
    pub output: u64,
}

/// What one seat served in this process.
pub struct SeatSpent {
    pub fingerprint: String,
    pub calls: u64,
    pub usage: Usage,
}

impl LunaClient {
    pub fn new(seats: Vec<LunaSeat>) -> anyhow::Result<Self> {
        if seats.is_empty() {
            anyhow::bail!("aucun siège luna");
        }
        let n = seats.len();
        let plain = reqwest::Client::builder()
            .timeout(Duration::from_secs(600))
            .build()?;
        Ok(Self {
            http: ClientBuilder::new(plain.clone())
                .with(TracingMiddleware::<SpanBackendWithUrl>::new())
                .build(),
            plain,
            tokens: seats
                .iter()
                .map(|s| {
                    Mutex::new(Tokens {
                        access: s.access_token.clone(),
                        refresh: s.refresh_token.clone(),
                    })
                })
                .collect(),
            fingerprints: seats.iter().map(|s| s.fingerprint.clone()).collect(),
            accounts: seats.iter().map(|s| s.account_id.clone()).collect(),
            parked_until: Mutex::new(vec![0.0; n]),
            ledger: Ledger::learned(n, SEED_RATE_PER_MIN, SEED_LATENCY_S, SEED_TOKENS_PER_MIN),
            refusals: AtomicU64::new(0),
            rotations: Mutex::new(Vec::new()),
            served: (0..n).map(|_| AtomicU64::new(0)).collect(),
            spent: (0..n).map(|_| Default::default()).collect(),
            caps: seats.iter().map(|s| s.cap_percent).collect(),
            cap_checked_at: Mutex::new(vec![f64::MIN; n]),
        })
    }

    pub fn key_count(&self) -> usize {
        self.fingerprints.len()
    }

    /// Seats not parked right now.
    pub fn live_key_count(&self) -> usize {
        let now = now_unix();
        self.parked_until
            .lock()
            .expect("mutex sièges non empoisonné")
            .iter()
            .filter(|&&until| until <= now)
            .count()
    }

    pub fn refusals(&self) -> u64 {
        self.refusals.load(Ordering::Relaxed)
    }

    /// What each seat served and what it was billed, in the order the seats were
    /// handed over.
    pub fn spent_per_seat(&self) -> Vec<SeatSpent> {
        (0..self.fingerprints.len())
            .map(|i| {
                let [input, cached, cache_write, output] = &self.spent[i];
                SeatSpent {
                    fingerprint: self.fingerprints[i].clone(),
                    calls: self.served[i].load(Ordering::Relaxed),
                    usage: Usage {
                        input: input.load(Ordering::Relaxed),
                        cached: cached.load(Ordering::Relaxed),
                        cache_write: cache_write.load(Ordering::Relaxed),
                        output: output.load(Ordering::Relaxed),
                    },
                }
            })
            .collect()
    }

    pub fn rate_per_min(&self) -> f64 {
        self.ledger.rate_per_min()
    }

    pub fn demand(&self) -> (f64, usize) {
        self.ledger.demand()
    }

    /// Seats refreshed since the last call, and clears them. The caller writes
    /// them back under their fingerprint, the refresh token being single use.
    pub fn take_rotations(&self) -> Vec<Rotation> {
        std::mem::take(
            &mut *self
                .rotations
                .lock()
                .expect("mutex rotations non empoisonné"),
        )
    }

    /// Seats that could pay, and the soonest a parked one comes back.
    fn live_order(&self, now: f64) -> (Vec<usize>, f64) {
        let parked = self
            .parked_until
            .lock()
            .expect("mutex sièges non empoisonné");
        let n = self.fingerprints.len();
        let mut order = Vec::with_capacity(n);
        let mut soonest = f64::MAX;
        for i in 0..n {
            if parked[i] <= now {
                order.push(i);
            } else {
                soonest = soonest.min(parked[i]);
            }
        }
        (order, soonest)
    }

    /// Parks every capped seat whose window has reached its cap, until that window
    /// resets, each read at most once per [`CAP_CHECK_S`].
    async fn hold_caps(&self) {
        for seat in 0..self.caps.len() {
            let Some(cap) = self.caps[seat] else {
                continue;
            };
            {
                let now = now_unix();
                let mut checked = self
                    .cap_checked_at
                    .lock()
                    .expect("mutex plafonds non empoisonné");
                if now - checked[seat] < CAP_CHECK_S {
                    continue;
                }
                checked[seat] = now;
            }
            let usage = match self.usage(seat).await {
                Ok(usage) => usage,
                Err(e) => {
                    let _ = self.refresh(seat).await;
                    match self.usage(seat).await {
                        Ok(usage) => usage,
                        Err(_) => {
                            tracing::warn!(seat = %self.fingerprints[seat], error = %e, "luna_cap_unread");
                            self.park(seat, CAP_CHECK_S);
                            continue;
                        }
                    }
                }
            };
            let full = usage
                .windows()
                .into_iter()
                .filter(|w| w.used_percent >= cap)
                .map(|w| w.reset_after_seconds.unwrap_or(w.limit_window_seconds))
                .max();
            if let Some(reset_s) = full {
                tracing::warn!(seat = %self.fingerprints[seat], cap, reset_s, "luna_seat_at_cap");
                self.park(seat, reset_s as f64);
            }
        }
    }

    async fn usage(&self, seat: usize) -> anyhow::Result<codex::Usage> {
        let access = self.tokens[seat]
            .lock()
            .expect("mutex jetons non empoisonné")
            .access
            .clone();
        codex::usage(&self.plain, &access, &self.accounts[seat]).await
    }

    fn park(&self, seat: usize, delay_s: f64) {
        self.parked_until
            .lock()
            .expect("mutex sièges non empoisonné")[seat] = now_unix() + delay_s;
        tracing::warn!(seat = %self.fingerprints[seat], delay_s, "luna_seat_parked");
    }

    fn body(model: &str, effort: &str, instructions: &str, user: &str) -> Value {
        json!({
            "model": model,
            "instructions": instructions,
            "input": [{"type": "message", "role": "user",
                       "content": [{"type": "input_text", "text": user}]}],
            "tools": [],
            "tool_choice": "auto",
            "parallel_tool_calls": false,
            "reasoning": {"effort": effort},
            "store": false,
            "stream": true,
            "prompt_cache_key": cache_key(instructions),
            "include": [],
        })
    }

    /// One admission attempt and no wait, [`LunaError::Busy`] carrying how long
    /// until a seat could pay.
    pub async fn generate_now(
        &self,
        model: &str,
        effort: &str,
        instructions: &str,
        user: &str,
    ) -> Result<String, LunaError> {
        self.hold_caps().await;
        let now = now_unix();
        let (order, soonest) = self.live_order(now);
        if order.is_empty() {
            let wait = soonest - now;
            return Err(if wait > MAX_WAIT_S {
                LunaError::Exhausted
            } else {
                LunaError::Busy(wait.max(0.0))
            });
        }
        let payload_chars = instructions.len() + user.len();
        let reservation = match self.ledger.reserve(&order, payload_chars, now) {
            Ok(r) => r,
            Err(wait) => return Err(LunaError::Busy(wait.as_secs_f64())),
        };
        let seat = reservation.key_idx();
        let body = Self::body(model, effort, instructions, user);
        let (status, headers, text) = self.post(seat, &body).await?;
        if status == 401 {
            self.refresh(seat).await?;
            let (status, headers, text) = self.post(seat, &body).await?;
            return self.settle(reservation, seat, status, &headers, &text);
        }
        self.settle(reservation, seat, status, &headers, &text)
    }

    /// Waits its own turn rather than handing the job back.
    pub async fn generate(
        &self,
        model: &str,
        effort: &str,
        instructions: &str,
        user: &str,
    ) -> Result<String, LunaError> {
        loop {
            match self.generate_now(model, effort, instructions, user).await {
                Err(LunaError::Busy(wait)) => {
                    tokio::time::sleep(Duration::from_secs_f64(wait.max(0.05))).await;
                }
                other => return other,
            }
        }
    }

    fn settle(
        &self,
        reservation: Reservation<'_>,
        seat: usize,
        status: u16,
        headers: &reqwest::header::HeaderMap,
        text: &str,
    ) -> Result<String, LunaError> {
        if status == 429 {
            self.refusals.fetch_add(1, Ordering::Relaxed);
            // A 429 without `usage_limit_reached` is a throttle and not a full
            // window: parked until the window's reset, the seat would idle for hours.
            let delay = if text.contains("usage_limit_reached") {
                reset_after(headers)
            } else {
                tracing::warn!(seat = %self.fingerprints[seat], body = %truncate(text), "luna_throttled");
                retry_after(headers).unwrap_or(THROTTLE_PARK_S)
            };
            self.park(seat, delay);
            reservation.refused(Quota::default(), now_unix());
            return Err(LunaError::Busy(0.0));
        }
        if status != 200 {
            reservation.refused(Quota::default(), now_unix());
            tracing::warn!(
                seat = %self.fingerprints[seat],
                status,
                body = %truncate(text),
                "luna_refus"
            );
            return Err(LunaError::Status(status));
        }
        let (answer, usage) = parse_events(text)?;
        self.served[seat].fetch_add(1, Ordering::Relaxed);
        if let Some(usage) = usage {
            let [input, cached, cache_write, output] = &self.spent[seat];
            input.fetch_add(usage.input, Ordering::Relaxed);
            cached.fetch_add(usage.cached, Ordering::Relaxed);
            cache_write.fetch_add(usage.cache_write, Ordering::Relaxed);
            output.fetch_add(usage.output, Ordering::Relaxed);
        }
        let billed = usage.and_then(|u| u32::try_from(u.input + u.output).ok());
        reservation.settle(Quota::default(), billed, now_unix());
        if answer.trim().is_empty() {
            return Err(LunaError::Body("réponse vide".to_string()));
        }
        Ok(answer.trim().to_string())
    }

    async fn post(
        &self,
        seat: usize,
        body: &Value,
    ) -> Result<(u16, reqwest::header::HeaderMap, String), LunaError> {
        let access = self.tokens[seat]
            .lock()
            .expect("mutex jetons non empoisonné")
            .access
            .clone();
        let response = self
            .http
            .post(BASE)
            .bearer_auth(access)
            .header("chatgpt-account-id", &self.accounts[seat])
            .header("OpenAI-Beta", "responses=experimental")
            .header("originator", "codex_cli_rs")
            .header("session_id", session_id())
            .header("Accept", "text/event-stream")
            .json(body)
            .send()
            .await
            .map_err(|e| LunaError::Transport(e.to_string()))?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let text = response
            .text()
            .await
            .map_err(|e| LunaError::Transport(e.to_string()))?;
        Ok((status, headers, text))
    }

    /// Trades the refresh token of one seat. The old one dies in the exchange, so
    /// the new pair is queued for the store before the next call goes out.
    async fn refresh(&self, seat: usize) -> Result<(), LunaError> {
        let old = self.tokens[seat]
            .lock()
            .expect("mutex jetons non empoisonné")
            .refresh
            .clone();
        let fresh = oauth::refresh(&self.plain, oauth::CODEX_CLIENT_ID, &old)
            .await
            .map_err(|e| LunaError::Transport(e.to_string()))?;
        let Some(access) = fresh.access_token else {
            return Err(LunaError::Body("refresh sans access_token".to_string()));
        };
        let refresh = fresh.refresh_token.unwrap_or(old);
        {
            let mut t = self.tokens[seat]
                .lock()
                .expect("mutex jetons non empoisonné");
            t.access = access.clone();
            t.refresh = refresh.clone();
        }
        self.rotations
            .lock()
            .expect("mutex rotations non empoisonné")
            .push(Rotation {
                fingerprint: self.fingerprints[seat].clone(),
                access_token: access,
                refresh_token: refresh,
            });
        Ok(())
    }
}

/// The prefix is cached per routing key, so every call carrying the same
/// instructions shares one entry and a prompt variant opens its own.
fn cache_key(instructions: &str) -> String {
    format!(
        "{:016x}",
        xxhash_rust::xxh3::xxh3_64(instructions.as_bytes())
    )
}

fn session_id() -> String {
    let a = xxhash_rust::xxh3::xxh3_64(&now_unix().to_le_bytes());
    let b = xxhash_rust::xxh3::xxh3_64(&a.to_le_bytes());
    format!(
        "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
        a >> 32,
        (a >> 16) & 0xffff,
        a & 0xfff,
        (b >> 48) & 0xfff,
        b & 0xffff_ffff_ffff
    )
}

/// Seconds the seat says it needs, the header naming the primary window.
fn reset_after(headers: &reqwest::header::HeaderMap) -> f64 {
    ["x-codex-primary-reset-after-seconds", "retry-after"]
        .into_iter()
        .find_map(|name| header_secs(headers, name))
        .unwrap_or(5.0 * 3600.0)
}

fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<f64> {
    header_secs(headers, "retry-after")
}

fn header_secs(headers: &reqwest::header::HeaderMap, name: &str) -> Option<f64> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<f64>().ok())
}

/// The answer text and the tokens billed, out of the event stream. Reasoning is
/// not emitted as text, so what accumulates is the summary alone.
fn parse_events(raw: &str) -> Result<(String, Option<Usage>), LunaError> {
    let mut answer = String::new();
    let mut usage = None;
    let mut commentary = std::collections::HashSet::new();
    for line in raw.lines() {
        let Some(payload) = line.strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            // gpt-6-luna at effort none may write its reasoning as a
            // `commentary` message before the answer.
            Some("response.output_item.added") => {
                if event.pointer("/item/phase").and_then(Value::as_str) == Some("commentary") {
                    if let Some(id) = event.pointer("/item/id").and_then(Value::as_str) {
                        commentary.insert(id.to_string());
                    }
                }
            }
            Some("response.output_text.delta") => {
                let item = event.get("item_id").and_then(Value::as_str);
                if item.is_some_and(|id| commentary.contains(id)) {
                    continue;
                }
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    answer.push_str(delta);
                }
            }
            Some("response.completed" | "response.done") => {
                usage = event.pointer("/response/usage").map(|u| {
                    let count = |path: &str| u.pointer(path).and_then(Value::as_u64).unwrap_or(0);
                    Usage {
                        input: count("/input_tokens"),
                        cached: count("/input_tokens_details/cached_tokens"),
                        cache_write: count("/input_tokens_details/cache_write_tokens"),
                        output: count("/output_tokens"),
                    }
                });
            }
            _ => {}
        }
    }
    if answer.is_empty() && usage.is_none() {
        return Err(LunaError::Body(truncate(raw)));
    }
    Ok((answer, usage))
}

fn truncate(text: &str) -> String {
    text.chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lit_le_texte_et_le_cout_dans_le_flux() {
        let raw = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Le salarié \"}\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"conteste.\"}\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":",
            "{\"input_tokens\":5000,\"input_tokens_details\":{\"cached_tokens\":4000,",
            "\"cache_write_tokens\":100},",
            "\"output_tokens\":200,\"total_tokens\":5200}}}\n",
            "data: [DONE]\n"
        );
        let (answer, billed) = parse_events(raw).expect("flux lisible");
        assert_eq!(answer, "Le salarié conteste.");
        assert_eq!(
            billed,
            Some(Usage {
                input: 5000,
                cached: 4000,
                cache_write: 100,
                output: 200
            })
        );
    }

    #[test]
    fn le_message_commentary_ne_passe_pas_dans_le_resume() {
        let raw = concat!(
            "data: {\"type\":\"response.output_item.added\",\"item\":{\"id\":\"c1\",\"phase\":\"commentary\"}}\n",
            "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"c1\",\"delta\":\"Je dois résumer.\"}\n",
            "data: {\"type\":\"response.output_item.added\",\"item\":{\"id\":\"m1\",\"phase\":\"final_answer\"}}\n",
            "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"m1\",\"delta\":\"Le salarié conteste.\"}\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"total_tokens\":90}}}\n"
        );
        let (answer, _) = parse_events(raw).expect("flux lisible");
        assert_eq!(answer, "Le salarié conteste.");
    }

    #[test]
    fn un_flux_sans_evenement_utile_est_une_erreur() {
        assert!(parse_events("data: {\"type\":\"response.created\"}\n").is_err());
    }

    #[test]
    fn la_cle_de_cache_suit_les_instructions() {
        assert_eq!(cache_key("un prompt"), cache_key("un prompt"));
        assert_ne!(cache_key("un prompt"), cache_key("un autre"));
    }
}
