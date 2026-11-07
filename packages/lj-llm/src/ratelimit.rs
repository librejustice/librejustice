//! Quota ledger of a key pool: it holds the counters and only admits a request it
//! can pay for. A Mistral response carries the key's quota state
//! (`x-ratelimit-*-minute`) but no `Retry-After`, and Gemini carries neither.
//!
//! The two counters obey different laws:
//!
//! - **requests**: hard-edged bucket, back to full at the top of the wall minute,
//!   and proper to `/v1/chat/completions`. `/v1/conversations` calls leave it
//!   where it stood, so that route has no request counter and does not gate.
//! - **tokens**: rolling window, shared by the two routes. The tokens of a
//!   `chat/completions` call show up in what `conversations` announces next, so the
//!   route is not a dimension of the bucket. Each spend is released 60 s after it
//!   was made, continuously and with no minute boundary. Hence `TokenWindow`, one
//!   slot per wall second, whose remaining capacity is the ceiling minus what was
//!   spent over the last 60 s.
//!
//! [`Ledger::new`] takes those ceilings from the response headers, which is Mistral.
//! [`Ledger::learned`] has none to read, which is Gemini. What it drives is the calls
//! in flight it aims to hold, AIMD on the outcomes alone, and the admission rate that
//! Little derives from it and from the round trip it measures. A ceiling below one
//! call in flight is reachable that way and by no concurrency cap. Nothing here is a
//! configured rate, and a refusal that names its own ceiling replaces the search.

use std::sync::Mutex;
use std::time::Duration;

/// Seed token ceiling: `mistral-small-2603` on `/v1/conversations`. It keeps a
/// fresh client from starting blind; the first response replaces it with what
/// the service announces.
const SEED_TOK_PER_MIN: u32 = 20_000;

/// Marge posée après la frontière de minute : la bascule du service traîne de
/// quelques centaines de millisecondes après le top. Avec [`JITTER_SPREAD`], les
/// réveils tombent dans `[500, 750[` ms.
const BOUNDARY_MARGIN: Duration = Duration::from_millis(500);

/// Étalement des réveils, et nombre de crans distincts.
const JITTER_SPREAD: Duration = Duration::from_millis(250);
const JITTER_STEPS: u64 = 16;

/// Wait floor. A token wait is the deadline of the spend that covers the deficit,
/// which goes to zero when that spend is about to be released; without a floor,
/// admission re-probes every bucket in a tight loop to gain nothing but timer
/// granularity.
const MIN_WAIT: Duration = Duration::from_millis(25);

/// Width of the token window: a spend stays in it until it is released.
const WINDOW_SECS: f64 = 60.0;

/// Slots in the window, one per wall second. One more than the window is wide, so
/// that two distinct seconds never land on the same slot while both are live and a
/// stale slot is recognised by its deadline alone.
const WINDOW_SLOTS: usize = 61;

/// Ratio d'amorce tokens/caractère. L'écart avec le ratio réellement facturé est
/// absorbé dès la 1ʳᵉ réponse.
const SEED_TOKENS_PER_CHAR: f64 = 1.0 / lj_vocab::tokens::CHARS_PER_TOKEN_SAFE;

/// Plafond et reste d'un compteur, tels que la réponse les annonce.
#[derive(Debug, Clone, Copy)]
pub struct Counter {
    pub limit: u32,
    pub left: u32,
}

/// Compteurs de quota lus sur une réponse Mistral.
///
/// Un 200 les porte tous. Un refus ne porte que la paire de la ressource épuisée —
/// requêtes ou tokens, jamais les deux, jamais le coût. Chaque paire se lit donc
/// pour elle-même.
#[derive(Debug, Clone, Copy, Default)]
pub struct Quota {
    pub req: Option<Counter>,
    pub tok: Option<Counter>,
    /// `x-ratelimit-tokens-query-cost` — coût facturé de CETTE requête. Exact :
    /// la somme des coûts colle au décrément du restant, à l'unité.
    pub cost: Option<u32>,
}

impl Quota {
    pub fn from_headers(headers: &reqwest::header::HeaderMap) -> Self {
        let num = |name: &str| -> Option<u32> { headers.get(name)?.to_str().ok()?.parse().ok() };
        let pair = |limit: &str, left: &str| {
            Some(Counter {
                limit: num(limit)?,
                left: num(left)?,
            })
        };
        Self {
            req: pair(
                "x-ratelimit-limit-req-minute",
                "x-ratelimit-remaining-req-minute",
            ),
            tok: pair(
                "x-ratelimit-limit-tokens-minute",
                "x-ratelimit-remaining-tokens-minute",
            ),
            cost: num("x-ratelimit-tokens-query-cost"),
        }
    }
}

/// Horloge murale en secondes depuis l'époque. La frontière des seaux est murale,
/// pas monotone : `Instant` ne saurait pas la situer.
pub fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// One wall second of token spending, and the instant it comes back.
#[derive(Debug, Clone, Copy, Default)]
struct Slot {
    /// Latest spend of the slot plus the window width. Once past, the slot reads
    /// as empty, with nothing to sweep.
    released_at: f64,
    tokens: f64,
}

/// Rolling token window of one key: what was spent over the last 60 s, laid out by
/// the wall second of the spend so that each release instant is known exactly.
#[derive(Debug)]
struct TokenWindow {
    limit: u32,
    slots: [Slot; WINDOW_SLOTS],
    /// Estimates of the calls in flight. They leave `inflight` for the window when
    /// the response announces what was actually billed.
    inflight: u32,
}

impl TokenWindow {
    fn new(limit: u32) -> Self {
        Self {
            limit,
            slots: [Slot::default(); WINDOW_SLOTS],
            inflight: 0,
        }
    }

    fn spent(&self, now: f64) -> f64 {
        self.slots
            .iter()
            .filter(|s| s.released_at > now)
            .map(|s| s.tokens)
            .sum()
    }

    fn record(&mut self, tokens: f64, now: f64) {
        let slot = &mut self.slots[now as u64 as usize % WINDOW_SLOTS];
        if slot.released_at <= now {
            *slot = Slot::default();
        }
        slot.tokens += tokens;
        slot.released_at = now + WINDOW_SECS;
    }

    /// Tokens payable at `now`, reservations in flight deducted. May go negative:
    /// the caller reads the deficit off it.
    fn available(&self, now: f64) -> f64 {
        f64::from(self.limit) - self.spent(now) - f64::from(self.inflight)
    }

    /// Wait after which `deficit` tokens will have come back. Slots go out oldest
    /// first, so the wait is the deadline of the one that covers the deficit.
    /// `None` when the whole window does not cover it: nothing is left to wait for
    /// but a call in flight settling, and no clock places that.
    fn wait_for(&self, deficit: f64, now: f64) -> Option<Duration> {
        let newest = now as u64;
        let mut given_back = 0.0;
        for sec in newest.saturating_sub(WINDOW_SLOTS as u64 - 1)..=newest {
            let slot = self.slots[sec as usize % WINDOW_SLOTS];
            if slot.released_at <= now {
                continue;
            }
            given_back += slot.tokens;
            if given_back >= deficit {
                return Some(Duration::from_secs_f64(slot.released_at - now));
            }
        }
        None
    }

    /// Realigns on an announced remaining that holds authority, which is to say a
    /// refusal. What the window does not account for was burnt by another consumer
    /// of the key, at an instant we do not know: it goes in now, and so comes back
    /// in 60 s. A served response does not come through here. Under load the
    /// service announces zero remaining on calls it serves, and believing it walls
    /// the key for a minute over nothing.
    fn reconcile(&mut self, left: u32, now: f64) {
        let unaccounted = f64::from(self.limit) - f64::from(left) - self.spent(now);
        if unaccounted > 0.0 {
            self.record(unaccounted, now);
        }
    }
}

/// Hard-edged request counter: back to full at the top of the wall minute. Proper
/// to a service that announces its remainder, and corrected by every response.
#[derive(Debug, Clone, Copy)]
struct RequestWindow {
    limit: u32,
    left: u32,
    /// Wall minute (unix / 60) `left` refers to.
    minute: u64,
}

impl RequestWindow {
    fn roll(&mut self, now: f64) {
        let m = (now / 60.0) as u64;
        if m > self.minute {
            self.left = self.limit;
            self.minute = m;
        }
    }
}

/// Growth per served call, in calls in flight. Additive and not scaled: a
/// proportional growth barely climbs off the floor within a run.
const GROW_STEP: f64 = 0.25;

/// Multiplicative cut on a refusal.
const BACKOFF: f64 = 0.75;

/// In-flight floor: a bucket that only ever refuses still probes.
const TARGET_MIN: f64 = 0.05;

/// Latency floor, so a bucket that has only seen instant failures still paces.
const LATENCY_MIN: f64 = 0.2;

/// A target that has been refused is not crossed again before this.
const PROBE_PERIOD: f64 = 120.0;

/// Token bucket whose burst and refill rate are learnt from the outcomes, for a
/// service that publishes no counter and states its ceilings only by refusing.
///
/// The service serves a burst, then a trickle, then nothing: a bucket, and not a
/// spacing it imposes. A refusal leaves the day's requests untouched, so it costs
/// the round trip and nothing else.
#[derive(Debug)]
struct LearnedRate {
    tokens: f64,
    /// Calls in flight the bucket aims to hold. AIMD moves this, and nothing else.
    target: f64,
    /// Round trip of a served call, moving average, seconds.
    latency: f64,
    /// Target that a refusal has already met, once one has.
    refused_at: Option<f64>,
    /// Instant before which the target does not cross `refused_at`.
    probe_at: f64,
    /// Floor a `retryDelay` has posted: the service's own figure holds.
    hold_until: f64,
    /// End of the episode one cut has already paid for. Held apart from `hold_until`, which
    /// the service may state as `0s`, and a wave of refusals would then cut once per refusal.
    cut_until: f64,
    /// An admission was refused since the last growth, so the target is what binds. Without it the target climbs over an idle pool and the next wave pays the whole climb in refusals.
    starved: bool,
    last: f64,
}

impl LearnedRate {
    fn new(rate_per_min: f64, latency: f64, now: f64) -> Self {
        let latency = latency.max(LATENCY_MIN);
        let target = (rate_per_min / 60.0 * latency).max(TARGET_MIN);
        Self {
            tokens: target.max(1.0),
            target,
            latency,
            refused_at: None,
            probe_at: 0.0,
            hold_until: 0.0,
            cut_until: 0.0,
            starved: false,
            last: now,
        }
    }

    /// Little, read backwards: holding `target` calls of `latency` seconds each is
    /// admitting `target / latency` of them a second. A service whose ceiling sits
    /// under one call in flight is held by this and by no concurrency cap.
    fn rate(&self) -> f64 {
        self.target / self.latency.max(LATENCY_MIN)
    }

    fn burst(&self) -> f64 {
        self.target.max(1.0)
    }

    fn refill(&mut self, now: f64) {
        if now > self.last {
            let burst = self.burst();
            self.tokens = (self.tokens + (now - self.last) * self.rate()).min(burst);
            self.last = now;
        }
    }

    fn try_take(&mut self, now: f64) -> Result<(), Duration> {
        self.refill(now);
        if now < self.hold_until {
            return Err(Duration::from_secs_f64(self.hold_until - now));
        }
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            return Ok(());
        }
        self.starved = true;
        Err(Duration::from_secs_f64((1.0 - self.tokens) / self.rate()))
    }

    /// Grows the rate, held under what has already been refused until a probe is
    /// due: hugging a known ceiling costs one refusal per [`PROBE_PERIOD`], where an
    /// unbounded climb pays one per cycle.
    ///
    /// The burst does not grow. Under a full queue the throughput is the refill rate,
    /// so a burst above one only spends tokens faster than they come back.
    fn served(&mut self, latency: Option<f64>, now: f64) {
        if let Some(sample) = latency.filter(|s| *s > 0.0) {
            self.latency = self.latency * 0.8 + sample * 0.2;
        }
        if !self.starved {
            return;
        }
        self.starved = false;
        let grown = self.target + GROW_STEP;
        self.target = match self.refused_at {
            Some(ceiling) if grown >= ceiling && now < self.probe_at => self.target,
            Some(ceiling) if grown >= ceiling => {
                self.probe_at = now + PROBE_PERIOD;
                grown
            }
            _ => grown,
        };
    }

    /// The rate a refusal happened at becomes the ceiling, and the wait is the
    /// service's own `retryDelay` when it states one. The bucket refills during the
    /// hold, so the wait is not paid twice.
    ///
    /// A refusal that states nothing waits out the wall minute, the period of every
    /// rate family we have met: waiting one refill instead has the bucket refused
    /// again on the way down, cut after cut.
    fn refused(&mut self, stated: Option<f64>, retry_delay: Option<Duration>, now: f64) {
        if let Some(delay) = retry_delay {
            self.hold_until = self.hold_until.max(now + delay.as_secs_f64());
        }
        // A quota is not a congestion: a refusal that names its ceiling gives the
        // target outright, where cutting would walk down to it paying one refusal a
        // step and then walk back up past it.
        if let Some(per_sec) = stated {
            self.target = (per_sec * self.latency).max(TARGET_MIN);
            self.refused_at = Some(self.target);
            self.probe_at = now + PROBE_PERIOD;
            self.tokens = self.tokens.min(self.burst());
            return;
        }
        let to_boundary = 60.0 - now % 60.0 + BOUNDARY_MARGIN.as_secs_f64();
        // One cut per episode: the calls of a wave already in flight all come back
        // refused, and cutting on each one ratchets the target down to the floor.
        if now < self.cut_until {
            return;
        }
        self.refused_at = Some(self.target);
        self.probe_at = now + PROBE_PERIOD;
        self.target = (self.target * BACKOFF).max(TARGET_MIN);
        self.tokens = 0.0;
        self.last = now;
        self.cut_until = now + to_boundary;
        if retry_delay.is_none() {
            self.hold_until = now + to_boundary;
        }
    }
}

/// What governs admission: what the service announces, or what its refusals have
/// taught.
#[derive(Debug)]
enum Pace {
    /// `x-ratelimit-*`, corrected by every response. `None` until one announces a
    /// request counter, and only `/v1/chat/completions` does.
    Announced(Option<RequestWindow>),
    /// The only regime available against a service that announces nothing.
    Learned(LearnedRate),
}

#[derive(Debug)]
struct Bucket {
    pace: Pace,
    inflight_req: u32,
    tokens: TokenWindow,
    /// Round trip of a served call, moving average, seconds. `None` until one has
    /// been served. A learnt bucket holds its own, which its AIMD moves.
    latency: Option<f64>,
    /// Tokens a served call billed, moving average. The token window is a ceiling in
    /// tokens and the demand a count of calls, so one converts the other.
    billed: Option<f64>,
    /// True as soon as a response has spoken of this key's tokens. Before that the
    /// counters are only the seed ceilings, and they assume the bucket free, which
    /// a fresh process cannot know: the bucket is held by the service, shared with
    /// whoever else hits the key, and observable only by spending it.
    informed: bool,
}

impl Bucket {
    fn seed() -> Self {
        Self {
            pace: Pace::Announced(None),
            inflight_req: 0,
            tokens: TokenWindow::new(SEED_TOK_PER_MIN),
            latency: None,
            billed: None,
            informed: false,
        }
    }

    /// Bucket of a service that announces nothing: `rate_per_min` and `burst` are
    /// seeds its refusals correct, never ceilings.
    fn learned(rate_per_min: f64, latency: f64, tok_per_min: u32, now: f64) -> Self {
        Self {
            pace: Pace::Learned(LearnedRate::new(rate_per_min, latency, now)),
            inflight_req: 0,
            tokens: TokenWindow::new(tok_per_min),
            latency: Some(latency),
            billed: None,
            informed: true,
        }
    }

    /// Moves an announced counter to the wall minute of `now` if it has turned. A
    /// learnt bucket has no boundary, it refills on its own.
    fn roll(&mut self, now: f64) {
        if let Pace::Announced(Some(req)) = &mut self.pace {
            req.roll(now);
        }
    }

    fn try_reserve(&mut self, cost: u32, now: f64) -> Result<(), Duration> {
        self.roll(now);
        // Until a response has spoken of this key, one call in flight: it is the
        // probe, and its `settle` is what makes the counters exact.
        if !self.informed && self.inflight_req > 0 {
            return Err(MIN_WAIT);
        }
        // A call dearer than the whole bucket would never pass: let the service be
        // the one to refuse it.
        let cost = cost.min(self.tokens.limit);

        // Tokens first: a request slot, once taken, is not given back, and it would
        // be lost if the token budget then refused.
        let available = self.tokens.available(now);
        if available < f64::from(cost) {
            return Err(self
                .tokens
                .wait_for(f64::from(cost) - available, now)
                .unwrap_or(MIN_WAIT));
        }

        // An announced `left` nets out what was served but not what is in flight.
        match &mut self.pace {
            Pace::Announced(None) => {}
            Pace::Announced(Some(req)) => {
                if req.left <= self.inflight_req {
                    return Err(Duration::from_secs_f64(60.0 - now % 60.0) + BOUNDARY_MARGIN);
                }
            }
            Pace::Learned(rate) => rate.try_take(now)?,
        }
        self.inflight_req += 1;
        self.tokens.inflight += cost;
        Ok(())
    }

    fn release(&mut self, reserved: u32) {
        self.inflight_req = self.inflight_req.saturating_sub(1);
        self.tokens.inflight = self.tokens.inflight.saturating_sub(reserved);
    }

    /// Takes the ceilings the response announces. Each pair applies on its own, a
    /// refusal carrying only one.
    fn observe(&mut self, q: Quota, now: f64) {
        self.roll(now);
        if let (Some(req), Pace::Announced(slot)) = (q.req, &mut self.pace) {
            // A zero limit is self-sealing: `roll` restores `left` from it, so
            // nothing is sent and no response can correct it. One slot keeps a probe.
            *slot = Some(RequestWindow {
                limit: req.limit.max(1),
                left: req.left,
                minute: (now / 60.0) as u64,
            });
        }
        if let Some(tok) = q.tok {
            self.tokens.limit = tok.limit;
            self.informed = true;
        }
    }

    /// Books a served call: `billed` is what it cost, and it comes back 60 s from
    /// now. A served call is also the only evidence a learnt bucket has that it can
    /// go faster.
    fn settle(&mut self, q: Quota, billed: u32, latency: Option<f64>, now: f64) {
        self.observe(q, now);
        self.tokens.record(f64::from(billed), now);
        if let Some(sample) = latency.filter(|s| *s > 0.0) {
            self.latency = Some(match self.latency {
                Some(held) => held * 0.8 + sample * 0.2,
                None => sample,
            });
        }
        if billed > 0 {
            let sample = f64::from(billed);
            self.billed = Some(match self.billed {
                Some(held) => held * 0.8 + sample * 0.2,
                None => sample,
            });
        }
        if let Pace::Learned(rate) = &mut self.pace {
            rate.served(latency, now);
        }
    }

    /// Calls in flight this bucket justifies holding, `None` before it has served
    /// one. A learnt bucket aims at a target and says it; an announced one is held
    /// by its counters, so the figure is Little read backwards as in `rate()`, on
    /// whichever of the request and token windows binds first.
    fn demand(&self) -> Option<f64> {
        if let Pace::Learned(rate) = &self.pace {
            return Some(rate.target);
        }
        Some(self.admitted_per_min()? / 60.0 * self.latency?.max(LATENCY_MIN))
    }

    /// Calls a minute the bucket admits as it stands, on whichever of the request
    /// and token windows binds first. A route that announces no request counter is
    /// mute until a call has been billed, the bill being what turns a ceiling in
    /// tokens into a count of calls.
    fn admitted_per_min(&self) -> Option<f64> {
        if let Pace::Learned(rate) = &self.pace {
            return Some(rate.rate() * 60.0);
        }
        let by_tok = self.billed.map(|b| f64::from(self.tokens.limit) / b);
        let by_req = match &self.pace {
            Pace::Announced(Some(req)) => Some(f64::from(req.limit)),
            _ => None,
        };
        match (by_req, by_tok) {
            (Some(r), Some(t)) => Some(r.min(t)),
            (found, None) | (None, found) => found,
        }
    }

    /// A refusal from a service that announces no counter: the rate it happened at
    /// is the only figure it gives, and `retry_delay` the only clock.
    fn refuse_learned(&mut self, stated: Option<f64>, retry_delay: Option<Duration>, now: f64) {
        if let Pace::Learned(rate) = &mut self.pace {
            rate.refused(stated, retry_delay, now);
        }
    }

    /// Books a refusal: nothing was billed, and the announced remaining holds.
    fn refuse(&mut self, q: Quota, now: f64) {
        self.observe(q, now);
        if let Some(tok) = q.tok {
            self.tokens.reconcile(tok.left, now);
        }
    }
}

/// Estimateur du coût d'une requête, en tokens facturés par caractère de payload,
/// mis à jour en moyenne glissante sur le coût réellement facturé.
///
/// L'estimation ne sert qu'à réserver ; [`Bucket::settle`] rend le compte juste.
/// Elle ne peut pas viser mieux : à payload constant, le préfixe déjà en cache
/// change ce qui est facturé. C'est un régime stable, pas une anomalie — la moyenne
/// le suit au lieu d'y résister.
#[derive(Debug)]
struct CostModel {
    tokens_per_char: Mutex<f64>,
}

impl CostModel {
    fn new() -> Self {
        Self {
            tokens_per_char: Mutex::new(SEED_TOKENS_PER_CHAR),
        }
    }

    fn estimate(&self, payload_chars: usize) -> u32 {
        let ratio = *self
            .tokens_per_char
            .lock()
            .expect("mutex coût non empoisonné");
        (payload_chars as f64 * ratio).ceil() as u32
    }

    fn observe(&self, payload_chars: usize, billed: u32) {
        if payload_chars == 0 {
            return;
        }
        let observed = f64::from(billed) / payload_chars as f64;
        let mut cur = self
            .tokens_per_char
            .lock()
            .expect("mutex coût non empoisonné");
        *cur = *cur * 0.8 + observed * 0.2;
    }
}

/// Quota ledger of a key pool.
///
/// One bucket per key and per model. Not per account: spending on one key leaves
/// the others untouched. Per model: a key spent on `mistral-small-2603` announces
/// a full window on `ministral-8b-latest` at the same instant.
///
/// The model is therefore a dimension of the bucket, and it is held by carrying one
/// ledger per model rather than a second index: a client calls a single model, so
/// its ledger has a single ceiling to learn.
#[derive(Debug)]
pub struct Ledger {
    buckets: Vec<Mutex<Bucket>>,
    cost: CostModel,
    /// Nombre d'attentes servies, pour étaler les réveils.
    waits: std::sync::atomic::AtomicU64,
}

/// Réservation en vol. Libère son budget en [`Drop`] si elle n'a pas été
/// réconciliée : une requête annulée par l'échéance du rerank ne fuit pas.
pub struct Reservation<'a> {
    ledger: &'a Ledger,
    key_idx: usize,
    cost: u32,
    payload_chars: usize,
    /// Wall instant the call left, so `settle` can hand the bucket its round trip.
    started_at: f64,
    settled: bool,
}

impl Reservation<'_> {
    pub fn key_idx(&self) -> usize {
        self.key_idx
    }

    /// Books the response's counters and gives the reservation back. `billed` is
    /// the body's own figure: `/v1/conversations` sends no cost header, and its
    /// `usage.total_tokens` is what the counter drops by, to the unit. Without it
    /// that route would hold estimates and never correct them.
    pub fn settle(mut self, quota: Quota, billed: Option<u32>, now: f64) {
        let exact = quota.cost.or(billed);
        if let Some(cost) = exact {
            self.ledger.cost.observe(self.payload_chars, cost);
        }
        let billed = exact.unwrap_or(self.cost);
        let mut b = self.ledger.buckets[self.key_idx]
            .lock()
            .expect("mutex seau non empoisonné");
        b.release(self.cost);
        b.settle(quota, billed, Some(now - self.started_at), now);
        self.settled = true;
    }

    /// Same on a refusal: nothing was billed, and the announced remaining holds.
    pub fn refused(mut self, quota: Quota, now: f64) {
        let mut b = self.ledger.buckets[self.key_idx]
            .lock()
            .expect("mutex seau non empoisonné");
        b.release(self.cost);
        b.refuse(quota, now);
        self.settled = true;
    }

    /// Books a refusal from a service that announces no counter: the bucket cuts its
    /// rate and holds for `retry_delay` when the body states one.
    pub fn refused_rate(
        mut self,
        stated_per_min: Option<f64>,
        retry_delay: Option<Duration>,
        now: f64,
    ) {
        let mut b = self.ledger.buckets[self.key_idx]
            .lock()
            .expect("mutex seau non empoisonné");
        b.release(self.cost);
        b.refuse_learned(stated_per_min.map(|r| r / 60.0), retry_delay, now);
        self.settled = true;
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        self.ledger.buckets[self.key_idx]
            .lock()
            .expect("mutex seau non empoisonné")
            .release(self.cost);
    }
}

impl Ledger {
    /// One bucket per key, ceilings learnt from the responses. Mistral, on both
    /// `/v1/conversations` and `/v1/chat/completions`.
    pub fn new(key_count: usize) -> Self {
        Self {
            buckets: (0..key_count).map(|_| Mutex::new(Bucket::seed())).collect(),
            cost: CostModel::new(),
            waits: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Rates learnt from the refusals, which is Gemini: it publishes no rate-limit
    /// header. Its quota is held per project, but no route and no header names the
    /// project of a key, so the bucket is keyed on the key like Mistral's.
    ///
    /// `rate_per_min` and `burst` only keep the first wave from being blind; from
    /// the first outcome on, they are the bucket's own business. The daily ceiling is
    /// not held here: an in-memory counter would hand the whole day's budget back on
    /// every restart, so it belongs where state survives one.
    pub fn learned(key_count: usize, rate_per_min: f64, latency: f64, tok_per_min: u32) -> Self {
        let now = now_unix();
        Self {
            buckets: (0..key_count)
                .map(|_| Mutex::new(Bucket::learned(rate_per_min, latency, tok_per_min, now)))
                .collect(),
            cost: CostModel::new(),
            waits: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Calls a minute the pool admits as it stands, summed over its buckets: what
    /// the learning has converged to, for the operator's log.
    pub fn rate_per_min(&self) -> f64 {
        self.buckets
            .iter()
            .filter_map(|b| {
                b.lock()
                    .expect("mutex seau non empoisonné")
                    .admitted_per_min()
            })
            .sum()
    }

    /// Calls in flight the pool justifies holding, summed over its buckets, and the
    /// count of buckets that have not served one yet. A driver that never waits is
    /// one the admitted rate does not pay for.
    pub fn demand(&self) -> (f64, usize) {
        let mut total = 0.0;
        let mut mute = 0usize;
        for b in &self.buckets {
            match b.lock().expect("mutex seau non empoisonné").demand() {
                Some(d) => total += d,
                None => mute += 1,
            }
        }
        (total, mute)
    }

    /// Coût estimé d'un payload de `payload_chars` caractères.
    pub fn estimate(&self, payload_chars: usize) -> u32 {
        self.cost.estimate(payload_chars)
    }

    /// Réserve sur la première clé de `order` qui peut payer. `order` porte la
    /// préférence de l'appelant (round-robin, clés refusées retirées).
    ///
    /// `Err(d)` : aucune ne peut, réessayer dans `d` — la plus courte des attentes,
    /// plus un jitter, jamais moins de [`MIN_WAIT`].
    pub fn reserve(
        &self,
        order: &[usize],
        payload_chars: usize,
        now: f64,
    ) -> Result<Reservation<'_>, Duration> {
        let cost = self.cost.estimate(payload_chars);
        let mut soonest = Duration::MAX;
        for &i in order {
            let mut bucket = self.buckets[i].lock().expect("mutex seau non empoisonné");
            match bucket.try_reserve(cost, now) {
                Ok(()) => {
                    return Ok(Reservation {
                        ledger: self,
                        key_idx: i,
                        cost,
                        payload_chars,
                        started_at: now,
                        settled: false,
                    });
                }
                Err(wait) => soonest = soonest.min(wait),
            }
        }
        let wait = if soonest == Duration::MAX {
            BOUNDARY_MARGIN
        } else {
            soonest
        };
        Err((wait + self.jitter()).max(MIN_WAIT))
    }

    /// Étalement ∈ [0, `JITTER_SPREAD`[, tiré d'un compteur et non de l'horloge : deux
    /// appels refusés à quelques microsecondes d'écart liraient la même sous-seconde.
    fn jitter(&self) -> Duration {
        let n = self
            .waits
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        JITTER_SPREAD.mul_f64((n % JITTER_STEPS) as f64 / JITTER_STEPS as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The announced counter of a bucket, when it carries one.
    fn announced(b: &Bucket) -> Option<RequestWindow> {
        match &b.pace {
            Pace::Announced(req) => *req,
            Pace::Learned(_) => None,
        }
    }

    /// A bucket claims drivers only once it has served a call: before that it knows
    /// neither its round trip nor what a call bills, and every figure would be a seed.
    #[test]
    fn a_bucket_claims_no_driver_before_it_has_served_one() {
        let t = 30_000_000_f64 * 60.0 + 10.0;

        let mut announced = Bucket::seed();
        assert_eq!(announced.demand(), None);
        // 50 requests a minute announced, a 4 s round trip: 50 / 60 * 4 in flight.
        announced.settle(quota(40, 45_000, 1_000), 1_000, Some(4.0), t);
        let claimed = announced.demand().expect("un appel servi");
        assert!((claimed - 50.0 / 60.0 * 4.0).abs() < 1e-9, "{claimed}");

        // Tokens bind first: 50 000 a minute at 5 000 a call is 10 calls, not the 50
        // the request counter announces.
        let mut token_bound = Bucket::seed();
        token_bound.settle(quota(40, 45_000, 5_000), 5_000, Some(4.0), t);
        let claimed = token_bound.demand().expect("un appel servi");
        assert!((claimed - 10.0 / 60.0 * 4.0).abs() < 1e-9, "{claimed}");

        let learned = Bucket::learned(60.0, 3.0, 1_000_000, t);
        assert_eq!(learned.demand(), Some(60.0 / 60.0 * 3.0));
    }

    /// Réponse complète : celle d'un 200.
    fn quota(req_left: u32, tok_left: u32, cost: u32) -> Quota {
        Quota {
            req: Some(Counter {
                limit: 50,
                left: req_left,
            }),
            tok: Some(Counter {
                limit: 50_000,
                left: tok_left,
            }),
            cost: Some(cost),
        }
    }

    /// Réponse d'un 429 de tokens : la seule paire des tokens, à zéro.
    fn refus_tokens() -> Quota {
        Quota {
            tok: Some(Counter {
                limit: 50_000,
                left: 0,
            }),
            ..Quota::default()
        }
    }

    /// Un seau neuf n'admet qu'un appel, quelle que soit la place qu'annoncent
    /// ses plafonds d'amorce : ils supposent le seau libre, et un process qui
    /// partage la clé avec un résident ne peut pas le savoir avant d'avoir tiré.
    /// La première réponse le renseigne, et il s'ouvre.
    #[test]
    fn un_seau_neuf_n_admet_qu_une_sonde() {
        let m = 30_000_000u64;
        let t = m as f64 * 60.0 + 10.0;
        let mut b = Bucket::seed();

        assert!(b.try_reserve(1_000, t).is_ok());
        assert_eq!(b.try_reserve(1_000, t), Err(MIN_WAIT));

        // Le `settle` de la sonde renseigne le seau : les suivants passent.
        b.settle(quota(40, 45_000, 1_000), 1_000, None, t);
        assert!(b.informed);
        assert!(b.try_reserve(1_000, t).is_ok());
    }

    /// A spend comes back 60 s after it was made, and the top of the wall minute
    /// gives nothing back. A charge burnt at second 10 is
    /// therefore still held at second 9 of the next minute.
    #[test]
    fn token_release_follows_the_sliding_window() {
        let m = 30_000_000u64;
        let t0 = m as f64 * 60.0 + 10.0;
        let mut b = Bucket::seed();
        b.settle(quota(40, 45_000, 5_000), 5_000, None, t0);

        assert_eq!(b.tokens.available(t0).round() as u32, 45_000);
        assert_eq!(b.tokens.available(t0 + 51.0).round() as u32, 45_000);
        assert_eq!(b.tokens.available(t0 + 60.5).round() as u32, 50_000);
    }

    /// Under load the service announces `remaining: 0` on calls it serves. Only
    /// the billed cost counts: believing that zero would wall the key for a minute
    /// after a call that went through.
    #[test]
    fn a_zero_remaining_on_a_served_call_does_not_wall_the_key() {
        let m = 30_000_000u64;
        let t = m as f64 * 60.0 + 10.0;
        let mut b = Bucket::seed();
        b.settle(quota(40, 0, 4_000), 4_000, None, t);

        assert_eq!(b.tokens.available(t).round() as u32, 46_000);
        assert!(b.try_reserve(4_000, t).is_ok());
    }

    /// Sustainable rate: at 20 000 tokens/min and 3 500 tokens per call, a key serves
    /// one call every 10.5 s. Five of them fill the window; the sixth waits for the
    /// deadline of the first, and not for the top of the minute.
    #[test]
    fn the_sustainable_rate_is_the_ceiling() {
        let mut b = Bucket::seed();
        b.tokens.limit = 20_000;
        b.informed = true;

        let mut t = 0.0;
        for _ in 0..5 {
            b.tokens.record(3_500.0, t);
            t += 10.5;
        }
        let wait = b.try_reserve(3_500, t).unwrap_err();
        assert_eq!((t + wait.as_secs_f64()).round() as u32, 60);
        assert!(b.try_reserve(3_500, 60.0).is_ok());
    }

    #[test]
    fn le_seau_de_requetes_repart_plein_au_top_de_minute() {
        let m = 30_000_000u64;
        let mut b = Bucket::seed();
        b.settle(quota(0, 49_000, 17), 17, None, m as f64 * 60.0 + 20.0);

        let wait = b.try_reserve(10, m as f64 * 60.0 + 20.0).unwrap_err();
        assert_eq!(wait.as_secs(), 40);

        b.roll((m + 1) as f64 * 60.0 + 1.0);
        assert_eq!(announced(&b).expect("le compteur est annoncé").left, 50);
    }

    /// The token wait is the deadline of the spend that covers the deficit, never a
    /// jump to the minute boundary: the window knows nothing of wall minutes. Here
    /// the boundary is 10 s away and the answer is 55 s.
    #[test]
    fn the_token_wait_does_not_jump_to_the_minute_boundary() {
        let m = 30_000_000u64;
        let t0 = m as f64 * 60.0 + 50.0;
        let mut b = Bucket::seed();
        b.informed = true;
        b.tokens.limit = 50_000;
        b.tokens.record(30_000.0, t0);
        b.tokens.record(20_000.0, t0 + 5.0);

        let t = t0 + 5.0;
        assert_eq!(b.tokens.available(t).round() as i64, 0);
        let wait = b.try_reserve(1_000, t).unwrap_err();
        assert_eq!(wait.as_secs_f64().round() as u32, 55);
    }

    #[test]
    fn une_reservation_abandonnee_rend_son_budget() {
        let ledger = Ledger::new(1);
        let now = now_unix();
        let before = ledger.buckets[0].lock().unwrap().tokens.available(now);
        {
            let r = ledger.reserve(&[0], 10_000, now).unwrap();
            assert_eq!(r.key_idx(), 0);
            let during = ledger.buckets[0].lock().unwrap().tokens.available(now);
            assert!(during < before, "la réservation doit grever le seau");
        }
        let after = ledger.buckets[0].lock().unwrap().tokens.available(now);
        assert_eq!(after, before);
    }

    /// `/v1/conversations` announces no request counter, and a bucket that has
    /// only ever seen that route must not gate on a phantom one.
    #[test]
    fn an_unannounced_request_counter_does_not_gate() {
        let m = 30_000_000u64;
        let t = m as f64 * 60.0;
        let mut b = Bucket::seed();
        let tok_only = Quota {
            tok: Some(Counter {
                limit: 1_000_000,
                left: 996_000,
            }),
            cost: Some(4_000),
            ..Quota::default()
        };
        for _ in 0..200 {
            b.try_reserve(4_000, t).expect("le plafond de tokens tient");
            b.release(4_000);
            b.settle(tok_only, 4_000, None, t);
        }
        assert!(announced(&b).is_none());
        assert!(b.try_reserve(4_000, t).is_ok());
    }

    /// The burst a learnt bucket holds is its in-flight target, which Little reads off
    /// the ceiling and the round trip: 60 calls a minute over 3 s is 3 in flight, so 3
    /// slots go through back to back and the 4th waits one refill, not a minute.
    #[test]
    fn a_learnt_bucket_spends_its_burst_then_paces() {
        let now = 20_000_000.0 * 60.0 + 12.0;
        let mut b = Bucket::learned(60.0, 3.0, 1_000_000, now);
        for _ in 0..3 {
            b.try_reserve(10, now).expect("la rafale passe d'un coup");
            b.release(10);
        }
        let wait = b.try_reserve(10, now).unwrap_err();
        assert_eq!(wait.as_secs_f64().round() as u32, 1);
        assert!(b.try_reserve(10, now + 1.1).is_ok());
    }

    /// A refusal that names its ceiling is a quota and not a congestion: the bucket
    /// takes the figure outright, where cutting walks down to it one refusal a step.
    #[test]
    fn a_named_refusal_takes_the_stated_ceiling() {
        let now = 20_000_000.0 * 60.0;
        let ledger = Ledger::learned(1, 600.0, 2.0, 1_000_000);
        assert_eq!(ledger.rate_per_min().round(), 600.0);
        ledger
            .reserve(&[0], 10, now)
            .expect("le seau neuf a un jeton")
            .refused_rate(Some(15.0), Some(Duration::from_secs(3)), now);
        assert_eq!(ledger.rate_per_min().round(), 15.0);
    }

    /// A refusal cuts the rate, holds the bucket for the `retryDelay` the body
    /// states, and the rate it happened at is not crossed again before the probe is
    /// due: hugging the ceiling is what keeps the refusals to one per period.
    #[test]
    fn a_refusal_cuts_the_rate_and_the_ceiling_holds() {
        let now = 20_000_000.0 * 60.0;
        let ledger = Ledger::learned(1, 60.0, 1.0, 1_000_000);
        ledger
            .reserve(&[0], 10, now)
            .expect("le seau neuf a un jeton")
            .refused_rate(None, Some(Duration::from_secs(35)), now);

        let wait = match ledger.reserve(&[0], 10, now) {
            Err(d) => d,
            Ok(_) => panic!("le seau doit tenir son repos"),
        };
        assert!(wait.as_secs() >= 34, "{wait:?}");
        assert!(ledger.reserve(&[0], 10, now + 36.0).is_ok());

        // 60 admissions a minute were refused, so the ceiling is 60, and the served
        // calls grow the rate back up to just under it.
        let cut = ledger.rate_per_min();
        assert!((30.0..60.0).contains(&cut), "{cut}");
        let feed = |t: f64| {
            if let Ok(r) = ledger.reserve(&[0], 10, t) {
                r.settle(Quota::default(), Some(10), t);
            }
        };
        for i in 0..80 {
            feed(now + 36.0 + f64::from(i));
        }
        let held = ledger.rate_per_min();
        assert!((cut..60.0).contains(&held), "le plafond tient : {held}");

        // Past the probe period it is crossed by one step, and by one only: that is
        // the whole price of finding out the tier has moved.
        feed(now + PROBE_PERIOD + 10.0);
        let probed = ledger.rate_per_min();
        assert!(probed > held, "la sonde franchit : {probed}");
        feed(now + PROBE_PERIOD + 20.0);
        assert_eq!(ledger.rate_per_min(), probed);
    }

    /// A wave admitted before the ceiling was known comes back refused call by call.
    /// Cutting on each one takes the rate down by 0.75 per refusal, so eight in-flight
    /// calls leave a tenth of the rate for a single congestion episode.
    #[test]
    fn the_refusals_of_one_wave_cut_the_rate_once() {
        let now = 20_000_000.0 * 60.0;
        let ledger = Ledger::learned(1, 60.0, 8.0, 1_000_000);
        let wave: Vec<_> = (0..8)
            .map(|_| ledger.reserve(&[0], 10, now).expect("la rafale passe"))
            .collect();
        for (i, reservation) in wave.into_iter().enumerate() {
            reservation.refused_rate(None, None, now + f64::from(i as u32) * 0.01);
        }
        assert!(
            (44.0..46.0).contains(&ledger.rate_per_min()),
            "{}",
            ledger.rate_per_min()
        );
    }

    /// A Gemini refusal may state `retryDelay: "0s"`, which leaves `hold_until` at
    /// `now`: a wave of ten such refusals still cuts once.
    #[test]
    fn a_wave_refused_with_a_zero_delay_cuts_once() {
        let now = 20_000_000.0 * 60.0 + 12.0;
        let ledger = Ledger::learned(1, 600.0, 1.0, 1_000_000);
        let wave: Vec<_> = (0..10)
            .map(|_| ledger.reserve(&[0], 10, now).expect("la rafale passe"))
            .collect();
        for (i, reservation) in wave.into_iter().enumerate() {
            reservation.refused_rate(None, Some(Duration::ZERO), now + i as f64 * 0.01);
        }
        let rate = ledger.rate_per_min();
        assert!((445.0..455.0).contains(&rate), "une seule coupe : {rate}");
    }

    /// The cut is what one episode pays once; the delay is the service's own figure,
    /// and a later refusal stating a longer one is obeyed without cutting again.
    #[test]
    fn a_skipped_cut_still_obeys_a_longer_delay() {
        let now = 20_000_000.0 * 60.0 + 12.0;
        let ledger = Ledger::learned(1, 120.0, 1.0, 1_000_000);
        let first = ledger.reserve(&[0], 10, now).expect("la rafale passe");
        let second = ledger.reserve(&[0], 10, now).expect("la rafale passe");
        first.refused_rate(None, Some(Duration::ZERO), now);
        second.refused_rate(None, Some(Duration::from_secs(35)), now + 0.01);

        let rate = ledger.rate_per_min();
        assert!((85.0..95.0).contains(&rate), "une seule coupe : {rate}");
        let wait = match ledger.reserve(&[0], 10, now + 1.0) {
            Err(wait) => wait,
            Ok(_) => panic!("le repos annonce par le second refus doit tenir"),
        };
        assert!(wait.as_secs() >= 33, "{wait:?}");
    }

    /// A served call grows the target only when the target is what binds. Otherwise it
    /// climbs over an idle pool and the next wave pays the whole climb in refusals.
    #[test]
    fn a_served_call_grows_a_learnt_rate_only_when_it_binds() {
        let now = 20_000_000.0 * 60.0;

        let idle = Ledger::learned(1, 15.0, 1.0, 1_000_000);
        let before = idle.rate_per_min();
        idle.reserve(&[0], 10, now)
            .expect("le seau neuf a un jeton")
            .settle(Quota::default(), Some(10), now);
        assert_eq!(
            idle.rate_per_min(),
            before,
            "rien ne bridait, rien ne grandit"
        );

        let pressed = Ledger::learned(1, 15.0, 1.0, 1_000_000);
        let held = pressed
            .reserve(&[0], 10, now)
            .expect("le seau neuf a un jeton");
        assert!(
            pressed.reserve(&[0], 10, now).is_err(),
            "la 2e admission doit buter sur la cible"
        );
        held.settle(Quota::default(), Some(10), now);
        assert!(pressed.rate_per_min() > before);
    }

    /// `/v1/conversations` sends no cost header, so the body's `usage` is the only
    /// exact figure on the summary route. Without it the route books estimates and
    /// its estimator never learns.
    #[test]
    fn a_route_without_a_cost_header_settles_on_the_body_usage() {
        let now = now_unix();
        let ledger = Ledger::new(1);
        let tok_only = Quota {
            tok: Some(Counter {
                limit: 20_000,
                left: 18_500,
            }),
            cost: None,
            ..Quota::default()
        };
        let payload_chars = 9_000;
        let seeded = ledger.cost.estimate(payload_chars);
        ledger
            .reserve(&[0], payload_chars, now)
            .expect("le seau est neuf")
            .settle(tok_only, Some(1_500), now);

        let available = ledger.buckets[0].lock().unwrap().tokens.available(now);
        assert_eq!(available, 18_500.0);
        let learnt = ledger.cost.estimate(payload_chars);
        assert!(
            (i64::from(learnt) - 1_500).abs() < (i64::from(seeded) - 1_500).abs(),
            "l'estimation doit se rapprocher du coût facturé"
        );
    }

    /// La réservation contourne une clé drainée au lieu de l'attendre.
    #[test]
    fn la_reservation_contourne_la_cle_drainee() {
        let ledger = Ledger::new(2);
        let now = now_unix();
        ledger.buckets[0]
            .lock()
            .unwrap()
            .refuse(refus_tokens(), now);
        let r = ledger.reserve(&[0, 1], 1_000, now).unwrap();
        assert_eq!(r.key_idx(), 1);
    }

    /// Le coût facturé ne se déduit pas du payload : l'estimation doit converger
    /// vers ce que le service annonce, si loin de l'amorce soit-il.
    #[test]
    fn le_cout_suit_ce_qui_est_facture() {
        let c = CostModel::new();
        let plein = c.estimate(6_000);
        assert!(plein > 1_500, "amorce non cachée : {plein}");
        for _ in 0..30 {
            c.observe(6_000, 20);
        }
        assert!(
            (20..26).contains(&c.estimate(6_000)),
            "{}",
            c.estimate(6_000)
        );
    }

    /// Un 429 de requêtes n'annonce que `limit`/`remaining-req-minute`. Cette seule
    /// paire doit murer la clé jusqu'au top de minute.
    #[test]
    fn un_refus_de_requetes_mure_la_cle() {
        let m = 30_000_000u64;
        let mut b = Bucket::seed();
        let refus = Quota {
            req: Some(Counter { limit: 50, left: 0 }),
            ..Quota::default()
        };
        b.refuse(refus, m as f64 * 60.0 + 20.0);
        let wait = b.try_reserve(10, m as f64 * 60.0 + 20.0).unwrap_err();
        assert_eq!(wait.as_secs(), 40);
    }

    /// A service serving a whole tier at `limit: 0` walls the key for as long as the
    /// process lives, the stored zero forbidding the very call that would lift it.
    #[test]
    fn a_zero_limit_still_admits_a_probe() {
        let m = 30_000_000u64;
        let mut b = Bucket::seed();
        let refus = Quota {
            req: Some(Counter { limit: 0, left: 0 }),
            ..Quota::default()
        };
        b.refuse(refus, m as f64 * 60.0 + 20.0);
        assert!(b.try_reserve(10, m as f64 * 60.0 + 20.0).is_err());

        let t = (m + 1) as f64 * 60.0 + 1.0;
        b.roll(t);
        assert_eq!(announced(&b).expect("le compteur est annoncé").left, 1);
        assert!(b.try_reserve(10, t).is_ok());
    }

    /// A token refusal announces zero remaining, and that zero holds: what the
    /// window does not account for was burnt by another consumer of the key, at an
    /// instant nothing reveals. It goes in now, so it comes back a full window
    /// later.
    #[test]
    fn un_refus_de_tokens_mure_la_cle() {
        let m = 30_000_000u64;
        let t = m as f64 * 60.0 + 20.0;
        let mut b = Bucket::seed();
        b.refuse(refus_tokens(), t);

        assert!(b.tokens.available(t) <= 0.0);
        assert_eq!(b.try_reserve(10, t).unwrap_err().as_secs(), 60);
        assert!(b.try_reserve(10, t + 60.5).is_ok());
    }

    /// The oldest slot comes back in half a millisecond and covers the deficit:
    /// admission waits the floor anyway rather than re-probing every bucket for
    /// nothing.
    #[test]
    fn l_attente_ne_descend_pas_sous_le_plancher() {
        let ledger = Ledger::new(1);
        let now = 20_000_000.0 * 60.0;
        {
            let mut b = ledger.buckets[0].lock().unwrap();
            b.informed = true;
            b.tokens.record(19_990.0, now - WINDOW_SECS + 0.0005);
            assert!(b.try_reserve(100, now).unwrap_err() < MIN_WAIT);
        }
        let wait = match ledger.reserve(&[0], 320, now) {
            Err(d) => d,
            Ok(_) => panic!("le seau ne peut pas payer, la réservation doit échouer"),
        };
        assert!(wait >= MIN_WAIT);
    }

    /// Deux attentes consécutives ne se réveillent pas au même instant.
    #[test]
    fn les_attentes_sont_etalees() {
        let ledger = Ledger::new(1);
        let now = now_unix();
        ledger.buckets[0]
            .lock()
            .unwrap()
            .refuse(refus_tokens(), now);
        let wait = |l: &Ledger| match l.reserve(&[0], 100, now) {
            Err(d) => d,
            Ok(_) => panic!("le seau est à sec, la réservation doit échouer"),
        };
        let a = wait(&ledger);
        let b = wait(&ledger);
        assert_ne!(a, b);
        assert!(b.abs_diff(a) < JITTER_SPREAD);
    }
}
