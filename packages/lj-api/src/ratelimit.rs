//! Per-caller allowance: a bounded token bucket, keyed by account or by network.
//!
//! Invariant: a handler that touches the database, hashes a password or sends a
//! mail takes one token from exactly one [`Cost`] class first, so `grep
//! budgets.take` is the complete map of what the application bounds. The edge
//! bounds the rest by IP, for the whole host.
//!
//! Every outside caller is named, an account by its subject, an anonymous caller by its
//! network: [`caller_key`] returns `None` for the in-process SSR render alone. The refusal
//! is the caller's: this module returns a verdict.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::HeaderMap;

use crate::typed_cache::TypedCache;
use lj_identity::Identity;
use tokio::sync::Mutex;

/// Cible tracing des dépassements de budget.
pub const LOG_TARGET: &str = "lj_api::ratelimit";

/// Nombre de seaux gardés en mémoire. Au-delà, moka évince les moins récemment
/// utilisés : le `time_to_idle` valant le temps de recharge complète, un seau
/// évincé serait de toute façon revenu plein.
const MAX_KEYS: u64 = 100_000;

/// What a handler spends. The key is the caller's choice: account when one is
/// known, network otherwise; `Auth` and `Porte` run before any account exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cost {
    /// A computed search on either engine: Postgres CPU, lexical as much as hybrid.
    Search,
    /// A decision reference settled by its shape: half a `Search`, from its bucket.
    Reference,
    /// A Mistral rerank on a `rerank_cache` miss.
    Rerank,
    /// One row by identifier on the machine surfaces (API, MCP), where loops run.
    Read,
    /// A decision composed into a PDF, the one route rendering a document per hit.
    Export,
    /// A password hash, a row created or a mail sent, before any session exists.
    Auth,
    /// An unauthenticated crossing of `/api` or `/mcp`: the `SELECT` of a refusal.
    Porte,
    /// A client incident report.
    ClientIncident,
    /// A page reporting what it shows.
    ReadReport,
    LandingReport,
    ActionReport,
    /// A support report, which is a mail.
    Signalement,
    /// A directory query that can name a natural person.
    Annuaire,
    /// A decision file submitted for review.
    Contribution,
}

impl Cost {
    fn surface(self) -> &'static str {
        match self {
            Cost::Search | Cost::Reference => "search",
            Cost::Rerank => "rerank",
            Cost::Read => "read",
            Cost::Export => "export",
            Cost::Auth => "auth",
            Cost::Porte => "porte",
            Cost::ClientIncident => "client",
            Cost::ReadReport => "lecture",
            Cost::LandingReport => "arrivee",
            Cost::ActionReport => "action",
            Cost::Signalement => "signalement",
            Cost::Annuaire => "annuaire",
            Cost::Contribution => "contribution",
        }
    }

    fn weight(self) -> f64 {
        match self {
            Cost::Reference => 0.5,
            _ => 1.0,
        }
    }
}

// Sustained allowance per minute of the classes without an environment setting.
// Read: 5 rows/s, above an agent reading a code article by article; a corpus loop
// still takes days.
// Auth: 20 keeps an office behind one NAT logging in at 9am, and caps argon2.
// Export: 10 lets a reader save a batch and bounds the cost of a render loop.
pub const READ_PER_MIN: u32 = 300;
pub const EXPORT_PER_MIN: u32 = 10;
pub const AUTH_PER_MIN: u32 = 20;
pub const PORTE_PER_MIN: u32 = 120;
pub const CLIENT_INCIDENTS_PER_MIN: u32 = 5;
/// A reader opens a page every few seconds at most.
pub const READ_REPORTS_PER_MIN: u32 = 30;
pub const ACTION_REPORTS_PER_MIN: u32 = 60;
pub const SIGNALEMENTS_PER_MIN: u32 = 3;
// Annuaire: 20 leaves a firm its conflict checks and puts a full harvest of the
// directory, 25 rows at a time, past hours on one nameable account.
pub const ANNUAIRE_PER_MIN: u32 = 20;
pub const CONTRIBUTIONS_PER_MIN: u32 = 5;

/// Burst tolerated, in minutes of allowance: only the sustained regime meets the refill.
const BURST_FACTOR: f64 = 2.0;

/// Verdict d'un prélèvement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Jeton prélevé : la dépense peut avoir lieu.
    Allowed,
    /// Seau vide : l'appelant a dépassé son débit soutenu.
    Exhausted,
}

impl Verdict {
    /// Valeur d'attribut pour la télémétrie.
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Allowed => "allowed",
            Verdict::Exhausted => "exhausted",
        }
    }
}

/// Clé de comptage d'un appelant. Construite par [`caller_key`] uniquement : le
/// type est opaque pour qu'aucun appelant ne bricole une clé à la main.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerKey(String);

impl CallerKey {
    /// Nature de l'appelant pour la télémétrie — jamais la clé elle-même, qui porte
    /// un `sub` ou une adresse.
    fn kind(&self) -> &'static str {
        if self.0.starts_with("u:") {
            "account"
        } else {
            "network"
        }
    }
}

/// The key a call is counted on: the account when authenticated, the network otherwise.
/// The identity never reaches telemetry: it lives in this module and in the buckets.
///
/// Anonymous: the IP read from `CF-Connecting-IP`, trusted because the origin accepts
/// `:443` from Cloudflare's ranges alone.
///
/// `None` for the in-process SSR render (`lj_web::api::client`), which has no client socket.
pub fn caller_key(user: Option<&str>, headers: &HeaderMap) -> Option<CallerKey> {
    key_of(&Identity::resolve(user, headers))
}

/// Projection « quota » d'une identité déjà résolue. Grossière par construction :
/// l'User-Agent n'y entre pas, sinon un seau neuf s'obtiendrait en changeant
/// d'en-tête. C'est ce qui la sépare du pseudonyme de télémétrie.
pub fn key_of(id: &Identity) -> Option<CallerKey> {
    match id {
        Identity::Account(sub) => Some(CallerKey(format!("u:{sub}"))),
        Identity::Network(network) => Some(CallerKey(format!("n:{network}"))),
        Identity::Unknown => None,
    }
}

/// Seau à jetons d'un appelant.
struct Bucket {
    tokens: f64,
    last: Instant,
}

/// One [`Budget`] per [`Cost`]. Built once in `AppState`, taken from everywhere.
#[derive(Clone)]
pub struct Budgets {
    search: Budget,
    rerank: Budget,
    read: Budget,
    export: Budget,
    auth: Budget,
    porte: Budget,
    client: Budget,
    lecture: Budget,
    arrivee: Budget,
    action: Budget,
    signalement: Budget,
    annuaire: Budget,
    contribution: Budget,
}

impl Budgets {
    pub fn new(search_per_min: u32, rerank_per_min: u32) -> Self {
        let budget = |cost: Cost, per_min| Budget::new(cost.surface(), per_min);
        Self {
            search: budget(Cost::Search, search_per_min),
            rerank: budget(Cost::Rerank, rerank_per_min),
            read: budget(Cost::Read, READ_PER_MIN),
            export: budget(Cost::Export, EXPORT_PER_MIN),
            auth: budget(Cost::Auth, AUTH_PER_MIN),
            porte: budget(Cost::Porte, PORTE_PER_MIN),
            client: budget(Cost::ClientIncident, CLIENT_INCIDENTS_PER_MIN),
            lecture: budget(Cost::ReadReport, READ_REPORTS_PER_MIN),
            arrivee: budget(Cost::LandingReport, READ_REPORTS_PER_MIN),
            action: budget(Cost::ActionReport, ACTION_REPORTS_PER_MIN),
            signalement: budget(Cost::Signalement, SIGNALEMENTS_PER_MIN),
            annuaire: budget(Cost::Annuaire, ANNUAIRE_PER_MIN),
            contribution: budget(Cost::Contribution, CONTRIBUTIONS_PER_MIN),
        }
    }

    /// Takes the weight of `cost` from its bucket for `key`. `None`, the in-process SSR
    /// render, is not counted.
    pub async fn take(&self, cost: Cost, key: Option<&CallerKey>) -> Verdict {
        let budget = match cost {
            Cost::Search | Cost::Reference => &self.search,
            Cost::Rerank => &self.rerank,
            Cost::Read => &self.read,
            Cost::Export => &self.export,
            Cost::Auth => &self.auth,
            Cost::Porte => &self.porte,
            Cost::ClientIncident => &self.client,
            Cost::ReadReport => &self.lecture,
            Cost::LandingReport => &self.arrivee,
            Cost::ActionReport => &self.action,
            Cost::Signalement => &self.signalement,
            Cost::Annuaire => &self.annuaire,
            Cost::Contribution => &self.contribution,
        };
        budget.take(key, cost.weight()).await
    }
}

/// The bucket store of one cost class: one bucket per caller, continuous refill.
#[derive(Clone)]
struct Budget {
    surface: &'static str,
    buckets: TypedCache<Arc<Mutex<Bucket>>>,
    capacity: f64,
    refill_per_sec: f64,
}

impl Budget {
    fn new(surface: &'static str, per_min: u32) -> Self {
        let refill_per_sec = f64::from(per_min) / 60.0;
        let capacity = f64::from(per_min) * BURST_FACTOR;
        Self {
            surface,
            buckets: TypedCache::builder(MAX_KEYS)
                .time_to_idle(Duration::from_secs_f64(capacity / refill_per_sec))
                .build(),
            capacity,
            refill_per_sec,
        }
    }

    async fn take(&self, key: Option<&CallerKey>, weight: f64) -> Verdict {
        self.take_at(key, weight, Instant::now()).await
    }

    /// [`Budget::take`] à instant imposé — les tests pilotent le temps.
    async fn take_at(&self, key: Option<&CallerKey>, weight: f64, now: Instant) -> Verdict {
        let Some(key) = key else {
            return Verdict::Allowed;
        };
        let capacity = self.capacity;
        let bucket = self
            .buckets
            .get_with(key.0.clone(), async move {
                Arc::new(Mutex::new(Bucket {
                    tokens: capacity,
                    last: now,
                }))
            })
            .await;
        let mut bucket = bucket.lock().await;
        // `saturating_duration_since` : deux tâches concurrentes peuvent avoir lu
        // `Instant::now()` dans l'ordre inverse de leur prise du mutex.
        let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
        bucket.last = now;
        bucket.tokens = (bucket.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        if bucket.tokens >= weight {
            bucket.tokens -= weight;
            return Verdict::Allowed;
        }
        tracing::info!(
            target: LOG_TARGET,
            surface = self.surface,
            caller = key.kind(),
            verdict = Verdict::Exhausted.as_str(),
            "budget exhausted"
        );
        Verdict::Exhausted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> CallerKey {
        CallerKey(s.to_string())
    }

    fn headers(name: &'static str, value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(name, value.parse().unwrap());
        h
    }

    #[tokio::test]
    async fn burst_then_refill() {
        let budget = Budget::new("test", 60);
        let k = key("u:alice");
        let t0 = Instant::now();
        for _ in 0..120 {
            assert_eq!(budget.take_at(Some(&k), 1.0, t0).await, Verdict::Allowed);
        }
        assert_eq!(budget.take_at(Some(&k), 1.0, t0).await, Verdict::Exhausted);
        let t1 = t0 + Duration::from_secs(10);
        for _ in 0..10 {
            assert_eq!(budget.take_at(Some(&k), 1.0, t1).await, Verdict::Allowed);
        }
        assert_eq!(budget.take_at(Some(&k), 1.0, t1).await, Verdict::Exhausted);
    }

    #[tokio::test]
    async fn a_reference_takes_half_a_search() {
        let budget = Budget::new("test", 60);
        let k = key("u:alice");
        let t0 = Instant::now();
        let half = Cost::Reference.weight();
        for _ in 0..240 {
            assert_eq!(budget.take_at(Some(&k), half, t0).await, Verdict::Allowed);
        }
        assert_eq!(budget.take_at(Some(&k), half, t0).await, Verdict::Exhausted);
    }

    #[tokio::test]
    async fn refill_is_capped_at_capacity() {
        let budget = Budget::new("test", 60);
        let k = key("u:alice");
        let t0 = Instant::now();
        assert_eq!(budget.take_at(Some(&k), 1.0, t0).await, Verdict::Allowed);
        let t1 = t0 + Duration::from_secs(3600);
        for _ in 0..120 {
            assert_eq!(budget.take_at(Some(&k), 1.0, t1).await, Verdict::Allowed);
        }
        assert_eq!(budget.take_at(Some(&k), 1.0, t1).await, Verdict::Exhausted);
    }

    #[tokio::test]
    async fn buckets_are_independent() {
        let budget = Budget::new("test", 1);
        let (a, b) = (key("u:alice"), key("n:203.0.113.7"));
        let t0 = Instant::now();
        for _ in 0..2 {
            assert_eq!(budget.take_at(Some(&a), 1.0, t0).await, Verdict::Allowed);
        }
        assert_eq!(budget.take_at(Some(&a), 1.0, t0).await, Verdict::Exhausted);
        assert_eq!(budget.take_at(Some(&b), 1.0, t0).await, Verdict::Allowed);
    }

    #[tokio::test]
    async fn the_ssr_render_is_not_counted() {
        let budget = Budget::new("test", 1);
        let t0 = Instant::now();
        for _ in 0..1000 {
            assert_eq!(budget.take_at(None, 1.0, t0).await, Verdict::Allowed);
        }
    }

    #[test]
    fn identity_wins_over_network() {
        let h = headers("cf-connecting-ip", "203.0.113.7");
        assert_eq!(caller_key(Some("sub-42"), &h), Some(key("u:sub-42")));
    }

    #[test]
    fn anonymous_falls_back_to_network() {
        let h = headers("cf-connecting-ip", "203.0.113.7");
        assert_eq!(caller_key(None, &h), Some(key("n:203.0.113.7")));
    }

    #[test]
    fn ipv6_is_grouped_by_64() {
        let a = caller_key(None, &headers("cf-connecting-ip", "2001:db8:1:2:3:4:5:6"));
        let b = caller_key(None, &headers("cf-connecting-ip", "2001:db8:1:2:ffff::1"));
        assert_eq!(a, b);
        assert_eq!(a, Some(key("n:2001:db8:1:2::/64")));
        let c = caller_key(None, &headers("cf-connecting-ip", "2001:db8:1:3::1"));
        assert_ne!(a, c);
    }

    #[test]
    fn no_client_ip_means_unidentified() {
        assert_eq!(caller_key(None, &HeaderMap::new()), None);
        assert_eq!(
            caller_key(None, &headers("cf-connecting-ip", "pas-une-ip")),
            None
        );
    }
}
