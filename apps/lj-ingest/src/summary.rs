//! Offline backfill of `decisions.summary`.
//!
//! Keyset iteration over the decisions whose summary is missing or older than the
//! target generation, then two shared files of jobs which the doors pull from, one
//! per shape: a decision whose court published its own commentary is pasted, the
//! others are rewritten.
//!
//! Nothing here sets a rate. A pool takes a job only when its own bucket can pay for
//! it, so the calls in flight are a consequence of the quotas rather than a setting,
//! and a refusal hands the job straight back to the queue instead of being ridden
//! out on the decision's back.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::signal::unix::{signal, SignalKind};

use anyhow::{anyhow, bail, Result};
use lj_llm::codex;
use lj_llm::gemini::{GeminiClient, GeminiError};
use lj_llm::luna::{LunaClient, LunaError, LunaSeat};
use lj_store::repository::{
    CollageRow, Credential, DecisionRepository, MissingSummaryRow, PickedText, StoredSummary,
    SummaryContext,
};
use lj_summary::{
    analysis_resumes, assemble_collage, assemble_rewrite, body_text_from_full_text, clean_summary,
    collage_quote, decision_title, reclean_parts, repair_collage, repaste_collage, repaste_quote,
    Header, Provenance, SummaryEngine, Written, COLLAGE_PROSE_MIN, CONTEXT_FIELDS, CONTEXT_SOURCES,
    JURISDICTION_TIERS,
};

use crate::config::Settings;

/// Passes a decision gets across the pools before a later run has it: one that
/// three pools could not write is a defect to look at, not a loop to run.
const MAX_ATTEMPTS: u32 = 3;

/// The engines one door serves, one per shape of work, `None` where it serves none.
///
/// Both engines of a door share model and reasoning level, which is what lets one
/// client and one key ledger carry the two: a second client on the same keys would
/// hold a second rate ledger and spend the quota twice.
#[derive(Clone, Copy)]
struct Engines {
    rewrite: Option<SummaryEngine>,
    collage: Option<SummaryEngine>,
}

impl Engines {
    fn for_shape(self, shape: Shape) -> Option<SummaryEngine> {
        match shape {
            Shape::Rewrite => self.rewrite,
            Shape::Collage => self.collage,
        }
    }

    /// The engine the client is built from, either one saying the same model and
    /// the same level.
    fn any(self) -> SummaryEngine {
        self.rewrite
            .or(self.collage)
            .expect("une porte sert au moins une forme")
    }
}

/// Thinking tokens are charged here too.
const GEMINI_MAX_OUTPUT_TOKENS: u32 = 16_000;

/// A model a run may spend, each writing with the engines of the current
/// generation, until its quota refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Model {
    // No collage on gemini, and not for want of quality: a request is what this
    // door is rationed on, per day and per key, and a collage spends a whole one
    // on two sentences the token metered door writes for almost nothing.
    #[value(name = "gemini-3.1-flash-lite")]
    Gemini31FlashLite,
    #[value(name = "gemini-3.5-flash-lite")]
    Gemini35FlashLite,
    #[value(name = "gpt-6-luna")]
    Gpt6Luna,
}

/// What the scheduled chain spends. The free gemini quota is held per model, so a
/// second model is capacity the first one does not have.
pub const CHAIN_MODELS: &[Model] = &[
    Model::Gemini31FlashLite,
    Model::Gemini35FlashLite,
    Model::Gpt6Luna,
];

impl Model {
    fn engines(self) -> Engines {
        let rewrite = |engine| Engines {
            rewrite: Some(engine),
            collage: None,
        };
        match self {
            Self::Gemini31FlashLite => rewrite(SummaryEngine::Gemini31BlockV12),
            Self::Gemini35FlashLite => rewrite(SummaryEngine::Gemini35BlockV12),
            Self::Gpt6Luna => Engines {
                rewrite: Some(SummaryEngine::Luna6BlockV12),
                collage: Some(COLLAGE),
            },
        }
    }
}

/// The engine that pastes a court's commentary. A collage stays at the generation that
/// wrote it, and a text in a language it does not write is rewritten instead.
const COLLAGE: SummaryEngine = SummaryEngine::Luna6CollageHeadedV12;

/// The same door, serving the catch-up alone, which has nothing to rewrite.
const LUNA_COLLAGE_ONLY: Engines = Engines {
    rewrite: None,
    collage: Some(COLLAGE),
};

/// Sleep floor of a driver whose pool cannot pay: the quota says how long to wait,
/// and this only keeps a wait of zero from spinning.
const IDLE_FLOOR_S: f64 = 0.05;

/// Gemini's 503 is an overloaded model, not a quota, and arrives in bursts of
/// several minutes: a job is retried, its back-off growing.
const MAX_UNAVAILABLE: u32 = 8;
const UNAVAILABLE_BACKOFF_MAX_S: f64 = 120.0;

/// Drivers held per call the demand asks for. The two sides are not symmetric: an
/// excess sleeps, a shortfall leaves quota unused, so the factor sits above one. Two
/// covers a doubling of the round trip without touching the admitted rate, which the
/// ledger holds whatever the driver count.
const DRIVER_MARGIN: f64 = 2.0;

/// The shape a decision is written in: its court's own words pasted, a rewrite
/// when the court published none, and nothing when no door of the run can paste.
/// A rewrite reads worse than the court's words, and the row would then be
/// written twice.
fn work_for(
    header: Header,
    commentary: Option<String>,
    can_collage: bool,
    lang: &str,
) -> Option<Work> {
    match commentary {
        Some(commentary) if COLLAGE.writes(lang) => {
            can_collage.then_some(Work::Collage { header, commentary })
        }
        _ => Some(Work::Rewrite { header }),
    }
}

/// The headers of a batch, by decision id, in one lookup.
pub(crate) async fn headers(
    repo: &DecisionRepository<'_>,
    rows: &[MissingSummaryRow],
) -> Result<std::collections::HashMap<i64, Header>> {
    let ids: Vec<i64> = rows.iter().map(|row| row.decision_id).collect();
    let contexts: std::collections::HashMap<i64, SummaryContext> = repo
        .summary_contexts(&ids, &CONTEXT_SOURCES, &CONTEXT_FIELDS)
        .await?
        .into_iter()
        .map(|context| (context.decision_id, context))
        .collect();
    Ok(rows
        .iter()
        .map(|row| {
            let context = contexts.get(&row.decision_id);
            let provenances: Vec<Provenance> = context
                .map(|c| c.provenances.as_slice())
                .unwrap_or_default()
                .iter()
                .map(|(source, fields)| Provenance { source, fields })
                .collect();
            let header = Header::new(
                &row.jurisdiction_type,
                row_title(row),
                context.and_then(|c| c.case_name.clone()),
                &provenances,
            );
            (row.decision_id, header)
        })
        .collect())
}

/// The court's commentary as a collage pastes it: an analysis cut to its résumés,
/// recased when the court published it in capitals, as the body of such a decision
/// is shown. `None` when an analysis leaves no résumé to paste.
fn court_commentary(raw: &str, analysis: bool) -> Option<String> {
    let commentary = if analysis {
        analysis_resumes(raw)
    } else {
        raw.to_string()
    };
    if commentary.chars().count() <= COLLAGE_PROSE_MIN {
        return None;
    }
    Some(lj_core::truecase::recased_summary(&commentary).unwrap_or(commentary))
}

/// A door, its quota and the engines it serves. The engine is a property of the
/// job and not of the door: that pairing is what the bench graded, and a prompt
/// served by another model is a summary nobody measured.
enum Pool {
    Gemini(Arc<GeminiClient>, Engines),
    Luna(Arc<LunaClient>, Engines),
}

/// What a pool did with a job.
enum Attempt {
    Written(Written),
    /// Nothing was sent: the pool cannot pay for this many seconds.
    Busy(f64),
    /// The pool is out until the quota resets, so it takes no further job.
    Handover,
    /// The model is overloaded: the job waits, the key is not at fault.
    Unavailable(String),
    Failed(String),
}

impl Pool {
    fn engines(&self) -> Engines {
        match self {
            Pool::Gemini(_, e) | Pool::Luna(_, e) => *e,
        }
    }

    fn engine_for(&self, shape: Shape) -> Option<SummaryEngine> {
        self.engines().for_shape(shape)
    }

    /// The file a door empties first. A collage costs the token metered door two
    /// sentences where a rewrite costs it the whole summary, and costs a request
    /// metered door the same one request either way. So the subscription seat goes
    /// to the collages and the free keys to the rewrites, each helping on the other
    /// file once its own is empty.
    fn prefers(&self) -> Shape {
        match self {
            Pool::Luna(..) => Shape::Collage,
            Pool::Gemini(..) => Shape::Rewrite,
        }
    }

    /// Order the doors are served in, lowest first. A door waits while a lower
    /// rank still has a driver able to take a job.
    ///
    /// The day's decisions go to the subscription seat, whose capacity expires in
    /// hours and is spent or lost. The free keys are rationed by the day and keep
    /// theirs for the catch-up, which has its own process.
    fn rank(&self) -> u8 {
        match self {
            Pool::Luna(..) => 0,
            Pool::Gemini(..) => 1,
        }
    }

    /// Keys the pool holds, one call at a time each.
    fn keys(&self) -> usize {
        match self {
            Pool::Gemini(client, _) => client.key_count(),
            Pool::Luna(client, _) => client.key_count(),
        }
    }

    /// Drivers the pool gets for a batch of `jobs`. One per key is the floor, which
    /// is what a pool that has served nothing yet runs on. Above it the measured
    /// demand rules: a quota paying for more calls in flight than the pool has keys
    /// is exactly what one driver per key cannot serve, and a large RPM or TPM
    /// allowance is that case. A driver the ledger refuses puts its job back and
    /// sleeps, so an excess costs a parked task and never a job held from another
    /// pool; a shortfall costs quota bought and left unused. Hence the margin.
    fn drivers(&self, jobs: usize) -> usize {
        let keys = self.keys();
        let (measured, mute) = self.demand();
        let heard = keys.saturating_sub(mute);
        let demand = if heard == 0 {
            0.0
        } else {
            measured * keys as f64 / heard as f64
        };
        let wanted = (demand * DRIVER_MARGIN).ceil() as usize;
        wanted.max(keys).min(jobs.max(1))
    }

    /// Calls in flight the quota justifies holding, against the drivers the pool
    /// was given. A gap either way is what an adaptive count would close.
    fn demand(&self) -> (f64, usize) {
        match self {
            Pool::Gemini(client, _) => client.demand(),
            Pool::Luna(client, _) => client.demand(),
        }
    }

    /// A refusal is a state of the pool, not a failure of the job: the ledger has
    /// already cut its rate, so the job goes back to the queue and pays nothing.
    async fn summarise(&self, job: &Job, engine: SummaryEngine) -> Attempt {
        let input = job.work.input(&job.body);
        let prompt = engine
            .system_prompt(job.reader.as_deref().unwrap_or(&job.lang))
            .expect("a job is in a language its engines write");
        match self {
            Pool::Gemini(client, _) => match client
                .generate_now(&prompt, &input, GEMINI_MAX_OUTPUT_TOKENS)
                .await
            {
                Ok(raw) => job.work.finish(&raw),
                Err(GeminiError::Busy(wait)) => Attempt::Busy(wait),
                Err(GeminiError::Status(429)) => Attempt::Busy(0.0),
                Err(GeminiError::Exhausted) => Attempt::Handover,
                Err(err @ GeminiError::Status(503)) => Attempt::Unavailable(err.to_string()),
                Err(err) => Attempt::Failed(err.to_string()),
            },
            Pool::Luna(client, _) => match client
                .generate_now(engine.model(), luna_effort(engine), &prompt, &input)
                .await
            {
                Ok(raw) => job.work.finish(&raw),
                Err(LunaError::Busy(wait)) => Attempt::Busy(wait),
                Err(LunaError::Exhausted) => Attempt::Handover,
                Err(err) => Attempt::Failed(err.to_string()),
            },
        }
    }
}

/// What the model is asked for on a decision, and what its answer becomes.
///
/// Two shapes, because a decision whose court published its own commentary does not
/// need rewriting: the model writes the two sentences that frame it and the code
/// pastes the commentary between them, word for word.
enum Work {
    Rewrite { header: Header },
    Collage { header: Header, commentary: String },
}

/// Which of the two shapes a job is, and which file it waits in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Shape {
    Collage,
    Rewrite,
}

impl Shape {
    fn other(self) -> Self {
        match self {
            Self::Collage => Self::Rewrite,
            Self::Rewrite => Self::Collage,
        }
    }
}

impl Work {
    fn shape(&self) -> Shape {
        match self {
            Work::Rewrite { .. } => Shape::Rewrite,
            Work::Collage { .. } => Shape::Collage,
        }
    }

    fn input(&self, body: &str) -> String {
        match self {
            Work::Rewrite { header } => header.rewrite_input(body),
            Work::Collage { header, commentary } => header.collage_input(body, commentary),
        }
    }

    fn finish(&self, raw: &str) -> Attempt {
        let written = match self {
            Work::Rewrite { .. } => assemble_rewrite(raw),
            Work::Collage { commentary, .. } => assemble_collage(raw, commentary),
        };
        match written {
            Some(written) => Attempt::Written(written),
            None => Attempt::Failed("answer without its opening and body lines".to_string()),
        }
    }

    /// Where the court's words sit: the body of a collage, nowhere in a rewrite.
    fn quote(&self, written: &Written) -> Option<std::ops::Range<usize>> {
        match self {
            Work::Rewrite { .. } => None,
            Work::Collage { .. } => Some(written.body.clone()),
        }
    }
}

/// A decision to summarise, and the passes it has already cost.
struct Job {
    decision_id: i64,
    lang: String,
    /// The language of a reader summary, `None` for the text's own.
    reader: Option<String>,
    body: String,
    work: Work,
    attempts: u32,
    unavailable: u32,
}

/// One file per shape, so a door can be served what it is cheapest on without
/// scanning past what it cannot serve at all.
#[derive(Default)]
struct Files {
    collages: std::collections::VecDeque<Job>,
    rewrites: std::collections::VecDeque<Job>,
}

type Queue = Arc<std::sync::Mutex<Files>>;

impl Files {
    fn push_back(&mut self, job: Job) {
        match job.work.shape() {
            Shape::Collage => self.collages.push_back(job),
            Shape::Rewrite => self.rewrites.push_back(job),
        }
    }

    fn push_front(&mut self, job: Job) {
        match job.work.shape() {
            Shape::Collage => self.collages.push_front(job),
            Shape::Rewrite => self.rewrites.push_front(job),
        }
    }

    /// The next job this door has an engine for, its preferred shape first. A door
    /// with no engine for a shape never sees that file.
    fn take(&mut self, pool: &Pool) -> Option<Job> {
        let first = pool.prefers();
        [first, first.other()]
            .into_iter()
            .filter(|shape| pool.engine_for(*shape).is_some())
            .find_map(|shape| match shape {
                Shape::Collage => self.collages.pop_front(),
                Shape::Rewrite => self.rewrites.pop_front(),
            })
    }

    fn len(&self) -> usize {
        self.collages.len() + self.rewrites.len()
    }
}

/// A summary written, on its way to the store.
struct Done {
    decision_id: i64,
    lang: String,
    reader: Option<String>,
    quote: Option<std::ops::Range<usize>>,
    written: Written,
    engine: SummaryEngine,
}

/// Drivers of rank 0 still able to take a job, which a higher rank waits on.
type Ahead = Arc<std::sync::atomic::AtomicUsize>;

/// How often a waiting driver rereads [`Ahead`].
const AHEAD_POLL_S: f64 = 0.5;

/// Set by SIGTERM or SIGINT during a run: the drivers take no new job, and the
/// calls in flight, already paid for, are written before the run ends.
static STOPPING: AtomicBool = AtomicBool::new(false);

/// True while a run drains on a signal. The chain runs its steps in one process, so
/// outside a run, or on a second signal, the signal ends the process as it would
/// without a handler.
static RUNNING: AtomicBool = AtomicBool::new(false);
static HANDLER: std::sync::Once = std::sync::Once::new();

struct Run;

impl Run {
    fn start() -> Self {
        HANDLER.call_once(|| {
            tokio::spawn(async {
                let mut term = signal(SignalKind::terminate()).expect("handler SIGTERM");
                loop {
                    tokio::select! {
                        _ = term.recv() => {}
                        _ = tokio::signal::ctrl_c() => {}
                    }
                    if !RUNNING.load(Ordering::Relaxed) || STOPPING.swap(true, Ordering::Relaxed) {
                        std::process::exit(143);
                    }
                    tracing::warn!("summary_stopping");
                }
            });
        });
        RUNNING.store(true, Ordering::Relaxed);
        Run
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::Relaxed);
    }
}

/// Which doors a run lets pull at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Doors {
    /// The lowest rank serves alone while it can, the others wait behind it. The
    /// day's few decisions then go to the window that expires in hours, and the
    /// free day is left whole for the drain.
    ByRank,
    /// All of them together. A drain queues more work than any door's day holds,
    /// so holding one back writes no summary anywhere and lets its day expire.
    All,
}

fn past(stop_at: Option<std::time::Instant>) -> bool {
    stop_at.is_some_and(|at| std::time::Instant::now() >= at)
}

/// Pulls from the shared queue as fast as its own quota allows, and no faster, once
/// no better ranked door is left to take the job.
async fn drive(
    pool: Arc<Pool>,
    queue: Queue,
    tx: tokio::sync::mpsc::Sender<Done>,
    ahead: Ahead,
    doors: Doors,
    stop_at: Option<std::time::Instant>,
) {
    use std::sync::atomic::Ordering::Relaxed;
    if doors == Doors::ByRank && pool.rank() > 0 {
        while ahead.load(Relaxed) > 0 {
            tokio::time::sleep(std::time::Duration::from_secs_f64(AHEAD_POLL_S)).await;
        }
    }
    pull(&pool, &queue, &tx, stop_at).await;
    if pool.rank() == 0 {
        ahead.fetch_sub(1, Relaxed);
    }
}

/// The job is taken for the length of one call: a pool that cannot pay hands it
/// straight back, where a worker holding it would park it behind a bucket.
async fn pull(
    pool: &Arc<Pool>,
    queue: &Queue,
    tx: &tokio::sync::mpsc::Sender<Done>,
    stop_at: Option<std::time::Instant>,
) {
    let take = || {
        if STOPPING.load(Ordering::Relaxed) || past(stop_at) {
            return None;
        }
        queue.lock().expect("file non empoisonnée").take(pool)
    };
    while let Some(mut job) = take() {
        let engine = pool
            .engine_for(job.work.shape())
            .expect("la file ne rend que les formes que la porte sert");
        match pool.summarise(&job, engine).await {
            Attempt::Written(written) => {
                let done = Done {
                    decision_id: job.decision_id,
                    lang: job.lang.clone(),
                    reader: job.reader.clone(),
                    quote: job.work.quote(&written),
                    written,
                    engine,
                };
                if tx.send(done).await.is_err() {
                    return;
                }
            }
            Attempt::Busy(wait) => {
                queue.lock().expect("file non empoisonnée").push_front(job);
                tokio::time::sleep(std::time::Duration::from_secs_f64(wait.max(IDLE_FLOOR_S)))
                    .await;
            }
            Attempt::Handover => {
                queue.lock().expect("file non empoisonnée").push_front(job);
                return;
            }
            Attempt::Unavailable(err) => {
                job.unavailable += 1;
                if job.unavailable >= MAX_UNAVAILABLE {
                    tracing::error!(
                        decision_id = job.decision_id,
                        engine = engine.code(),
                        error = %err,
                        "summary_failed"
                    );
                    continue;
                }
                let wait = (2f64.powi(job.unavailable as i32) * 2.5).min(UNAVAILABLE_BACKOFF_MAX_S);
                queue.lock().expect("file non empoisonnée").push_back(job);
                tokio::time::sleep(std::time::Duration::from_secs_f64(wait)).await;
            }
            Attempt::Failed(err) => {
                job.attempts += 1;
                if job.attempts >= MAX_ATTEMPTS {
                    tracing::error!(
                        decision_id = job.decision_id,
                        engine = engine.code(),
                        error = %err,
                        "summary_failed"
                    );
                } else {
                    queue.lock().expect("file non empoisonnée").push_back(job);
                }
            }
        }
    }
}

/// Format ETA « 12h03m04s ».
pub fn format_eta(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    format!("{h}h{m:02}m{sec:02}s")
}

/// Tirage `rand01` ∈ [0, 1) non cryptographique (jitter / sampling). Dérivé de
/// l'horloge — suffisant pour désynchroniser des workers / varier l'ordre.
fn rand01() -> f64 {
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0) as f64)
        / 1_000_000_000.0
}

/// Which stale decisions a run takes, and in what order.
#[derive(Debug, Clone)]
pub enum Order {
    /// Every one of them, by id: the sweep that finishes the corpus.
    Sweep,
    /// Cited at least `min` times inside the corpus, most cited first.
    Cited {
        min: i32,
        only: Vec<String>,
        named: bool,
    },
    /// Every one of them by [`JURISDICTION_TIERS`], most recent first inside a tier:
    /// the sweep that spends a week where a reader is once the cited ones are done.
    Jurisdiction {
        reverse: bool,
        only: Vec<String>,
        named: bool,
    },
    /// What engine `code` wrote, in one jurisdiction type or in all of them: the
    /// repass of a text that changed without its generation moving, which no
    /// version predicate can select. Repassing the Conseil d'État repairs faulty
    /// decisions and restores the outcome clause, cassation gaining none.
    Engine {
        code: i16,
        jurisdiction: Option<String>,
    },
}

/// Fills the summary of every text a decision's passages are cut from, each in its
/// own language, up to `summary_prompt_version >= target_version`.
///
/// Reads and writes on two connections of the pool. No rate is set here: each quota
/// bucket admits what it can pay for, the drivers pull from one shared queue, and
/// the writes are serialised on a single connection.
#[allow(clippy::too_many_arguments)]
pub async fn backfill_summaries(
    target_version: i16,
    batch_size: i64,
    limit: Option<i64>,
    shuffle: bool,
    refresh_stale: bool,
    order: Order,
    doors: Doors,
    models: &[Model],
    accounts: &[String],
    caps: &[(String, f64)],
    langs: &[String],
    reader: Option<&str>,
    stop_after: Option<std::time::Duration>,
) -> Result<()> {
    let stop_at = stop_after.map(|after| std::time::Instant::now() + after);
    let langs: Vec<&str> = langs.iter().map(String::as_str).collect();
    let _run = Run::start();
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let fetch_conn = pool
        .get()
        .await
        .map_err(|e| anyhow!("pool.get fetch: {e}"))?;
    let write_conn = pool
        .get()
        .await
        .map_err(|e| anyhow!("pool.get write: {e}"))?;
    // Backfill de maintenance : on lève la borne API (`build_pool` pose
    // `statement_timeout=30s`). Le scan de frontière des décisions sans résumé est
    // un batch long, pas une requête interactive.
    fetch_conn
        .batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout fetch: {e}"))?;
    write_conn
        .batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout write: {e}"))?;
    let fetch_repo = DecisionRepository::new(&fetch_conn);
    let write_repo = DecisionRepository::new(&write_conn);

    let picked = |c: &Credential| {
        accounts.is_empty()
            || accounts
                .iter()
                .any(|a| *a == c.tenant || *a == c.fingerprint)
    };
    let api_keys = |creds: Vec<Credential>| -> Vec<String> {
        creds
            .iter()
            .filter(|c| picked(c))
            .filter_map(|c| c.api_key().map(str::to_string))
            .collect()
    };
    let gemini_keys = api_keys(fetch_repo.credentials("gemini").await?);
    let mut gemini = Vec::new();
    let mut luna = None;
    for model in models {
        let engines = model.engines();
        let engine = engines.any();
        match model {
            Model::Gemini31FlashLite | Model::Gemini35FlashLite => {
                if gemini_keys.is_empty() {
                    continue;
                }
                let client = GeminiClient::new(
                    gemini_keys.clone(),
                    engine.model().to_string(),
                    engine.thinking().wire().map(str::to_string),
                )?;
                client.seed_day(&fetch_repo.gemini_day_spent(engine.model()).await?);
                gemini.push((Arc::new(client), engines));
            }
            Model::Gpt6Luna => {
                luna = luna_client(&fetch_repo, &picked, caps)
                    .await?
                    .map(|client| (client, engines));
            }
        }
    }
    let gemini = Arc::new(gemini);
    let luna_seats = luna.as_ref().map_or(0, |(c, _)| c.key_count());
    let pools: Vec<Arc<Pool>> = gemini
        .iter()
        .map(|(client, engines)| Arc::new(Pool::Gemini(Arc::clone(client), *engines)))
        .chain(
            luna.iter()
                .map(|(client, engines)| Arc::new(Pool::Luna(Arc::clone(client), *engines))),
        )
        .collect();
    let can_collage = pools.iter().any(|p| p.engine_for(Shape::Collage).is_some());
    if !can_collage
        && matches!(&order, Order::Engine { code, .. }
            if SummaryEngine::from_code(*code).is_some_and(|e| SummaryEngine::COLLAGES.contains(&e)))
    {
        bail!("repassage du collage demandé sans porte qui colle : il faut un siège luna");
    }
    let drivers: usize = pools.iter().map(|p| p.keys()).sum();
    tracing::info!(
        gemini_keys = gemini_keys.len(),
        luna_seats,
        pools = pools.len(),
        drivers,
        "summary_backfill_pools"
    );

    // Sampling de review : démarre la pagination keyset à un id aléatoire (avec
    // wraparound côté repo).
    let start_id = if shuffle {
        let max = fetch_repo.max_decision_id().await?;
        if max > 0 {
            (rand01() * (max as f64)) as i64
        } else {
            0
        }
    } else {
        0
    };

    let start = std::time::Instant::now();
    let mut processed: usize = 0;
    let mut persisted: usize = 0;
    let mut par_moteur: std::collections::BTreeMap<&'static str, usize> = Default::default();
    let mut left_to_collage: usize = 0;

    let own = match &order {
        Order::Sweep => {
            fetch_repo
                .iter_decisions_missing_summary(
                    target_version,
                    batch_size,
                    limit,
                    start_id,
                    shuffle,
                    refresh_stale,
                    &langs,
                )
                .await?
        }
        Order::Cited { min, only, named } => {
            let only: Vec<&str> = only.iter().map(String::as_str).collect();
            fetch_repo
                .iter_summaries_by_citations(
                    target_version,
                    batch_size,
                    limit,
                    refresh_stale,
                    *min,
                    &only,
                    *named,
                    &langs,
                )
                .await?
        }
        Order::Jurisdiction {
            reverse,
            only,
            named,
        } => {
            let only: Vec<&str> = only.iter().map(String::as_str).collect();
            let (types, tiers): (Vec<&str>, Vec<i32>) = JURISDICTION_TIERS.iter().copied().unzip();
            fetch_repo
                .iter_summaries_by_jurisdiction(
                    target_version,
                    batch_size,
                    limit,
                    refresh_stale,
                    &types,
                    &tiers,
                    *reverse,
                    &only,
                    *named,
                    &langs,
                )
                .await?
        }
        Order::Engine { code, jurisdiction } => {
            fetch_repo
                .iter_summaries_by_engine(*code, jurisdiction.as_deref(), batch_size, limit)
                .await?
        }
    };
    let readers = match reader {
        Some(reader) => {
            let (types, tiers): (Vec<&str>, Vec<i32>) = JURISDICTION_TIERS.iter().copied().unzip();
            fetch_repo
                .iter_reader_summaries(
                    reader,
                    target_version,
                    batch_size,
                    limit,
                    refresh_stale,
                    &types,
                    &tiers,
                )
                .await?
        }
        None => Vec::new(),
    };
    let texts: usize = own.iter().map(Vec::len).sum();
    let reader_texts: usize = readers.iter().map(Vec::len).sum();
    let queued = texts + reader_texts;
    tracing::info!(order = ?order, texts, readers = reader_texts, reader, "summary_backfill_queue");
    let batches = own
        .into_iter()
        .map(|batch| (None, batch))
        .chain(readers.into_iter().map(|batch| (reader, batch)));

    for (reader, batch) in batches {
        let ids: Vec<i64> = batch.iter().map(|row| row.decision_id).collect();
        let keys: Vec<(i64, &str)> = batch
            .iter()
            .map(|row| (row.decision_id, row.lang.as_str()))
            .collect();
        let mut full_texts: std::collections::HashMap<(i64, String), PickedText> = fetch_repo
            .fetch_texts(&keys)
            .await?
            .into_iter()
            .map(|text| ((text.decision_id, text.lang.clone()), text))
            .collect();
        // What each read text carries decides its shape, one indexed lookup on the
        // batch's ids: a court that published its own commentary is not rewritten,
        // its words are pasted, which beats a rewrite.
        let mut commentaries: std::collections::HashMap<i64, String> = match reader {
            Some(_) => Default::default(),
            None => fetch_repo
                .fetch_collage_rows(&ids, COLLAGE_PROSE_MIN as i32)
                .await?
                .into_iter()
                .filter_map(|row| {
                    court_commentary(&row.commentary, row.analysis)
                        .map(|commentary| (row.decision_id, commentary))
                })
                .collect(),
        };

        let headers = headers(&fetch_repo, &batch).await?;

        let queue: Queue = Arc::new(std::sync::Mutex::new(Files::default()));
        for row in &batch {
            let picked = full_texts
                .remove(&(row.decision_id, row.lang.clone()))
                .and_then(|text| {
                    let body = body_text_from_full_text(&text.full_text)?;
                    Some((text.lang, body))
                });
            let Some((lang, body)) = picked else {
                tracing::warn!(
                    public_id = %row.public_id,
                    decision_id = row.decision_id,
                    lang = %row.lang,
                    "summary_no_body"
                );
                continue;
            };
            let header = headers
                .get(&row.decision_id)
                .cloned()
                .expect("one header per decision of the batch");
            let commentary = row
                .read
                .then(|| commentaries.remove(&row.decision_id))
                .flatten();
            let Some(work) = work_for(header, commentary, can_collage, &lang) else {
                left_to_collage += 1;
                continue;
            };
            queue.lock().expect("file non empoisonnée").push_back(Job {
                decision_id: row.decision_id,
                lang,
                reader: reader.map(str::to_owned),
                body,
                work,
                attempts: 0,
                unavailable: 0,
            });
        }

        let jobs = queue.lock().expect("file non empoisonnée").len();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Done>(64);
        let mut running = Vec::new();
        let ahead: Ahead = Arc::new(std::sync::atomic::AtomicUsize::new(
            pools
                .iter()
                .filter(|p| p.rank() == 0)
                .map(|p| p.drivers(jobs))
                .sum(),
        ));
        for pool in &pools {
            for _ in 0..pool.drivers(jobs) {
                running.push(tokio::spawn(drive(
                    Arc::clone(pool),
                    Arc::clone(&queue),
                    tx.clone(),
                    Arc::clone(&ahead),
                    doors,
                    stop_at,
                )));
            }
        }
        // The writer's only sender must be the drivers, or the loop below never ends.
        drop(tx);

        processed += batch.len();
        while let Some(done) = rx.recv().await {
            match &done.reader {
                Some(reader) => {
                    write_repo
                        .set_reader_summary(
                            done.decision_id,
                            &done.lang,
                            reader,
                            &done.written.summary,
                            Some(done.written.body),
                            done.engine.prompt_version(),
                            done.engine.code(),
                        )
                        .await
                }
                None => {
                    write_repo
                        .set_summary(
                            done.decision_id,
                            &done.lang,
                            &done.written.summary,
                            Some(done.written.body),
                            done.quote,
                            done.engine.prompt_version(),
                            done.engine.code(),
                        )
                        .await
                }
            }
            .map_err(|e| anyhow!("set_summary id={}: {e}", done.decision_id))?;
            persisted += 1;
            *par_moteur.entry(done.engine.model()).or_default() += 1;
        }
        for driver in running {
            driver.await.map_err(|e| anyhow!("join: {e}"))?;
        }

        // Every pool handed over: what is left in the queue belongs to a later run,
        // and pulling the next batch would only pile up the same refusals.
        let left = queue.lock().expect("file non empoisonnée").len();
        processed -= left;
        flush_gemini_day(&write_repo, &gemini).await?;
        flush_luna_rotations(
            &write_repo,
            luna.as_ref().map(|(client, _)| client.as_ref()),
        )
        .await?;
        let elapsed = start.elapsed().as_secs_f64();
        let rate = processed as f64 / elapsed.max(1e-6);
        tracing::info!(
            processed,
            persisted,
            rate,
            par_moteur = %format_par_moteur(&par_moteur),
            elapsed = %format_eta(elapsed),
            "summary_progress"
        );
        log_gemini_quota(&gemini);
        log_demand(&pools);
        log_luna_seats(luna.as_ref().map(|(client, _)| client));
        if STOPPING.load(Ordering::Relaxed) {
            tracing::warn!(left, "summary_stopped");
            break;
        }
        if past(stop_at) {
            tracing::info!(left = queued - processed, "summary_deadline");
            break;
        }
        if left > 0 {
            tracing::warn!(left, "summary_pools_handed_over");
            break;
        }
    }

    flush_gemini_day(&write_repo, &gemini).await?;
    flush_luna_rotations(
        &write_repo,
        luna.as_ref().map(|(client, _)| client.as_ref()),
    )
    .await?;
    log_gemini_quota(&gemini);
    log_demand(&pools);
    tracing::info!(
        processed,
        persisted,
        par_moteur = %format_par_moteur(&par_moteur),
        left_to_collage,
        elapsed = %format_eta(start.elapsed().as_secs_f64()),
        "summary_done"
    );
    Ok(())
}

/// Seats in the order the door serves them: the shortest window first, since it is
/// the first to expire, then the least spent. A 5-hour window goes before a week
/// and a week before the 30 days of a free plan, each seat reading its own.
///
/// This is a priority and not a rotation: `Ledger::reserve` takes the first seat
/// that can pay, so the head absorbs everything its bucket admits. A seat the
/// endpoint will not answer for goes last rather than stopping the run.
async fn by_expiry(seats: Vec<LunaSeat>) -> Vec<LunaSeat> {
    let http = match reqwest::Client::builder()
        .user_agent("librejustice-ingest")
        .build()
    {
        Ok(http) => http,
        Err(_) => return seats,
    };
    let mut keyed: Vec<(i64, i64, LunaSeat)> = Vec::with_capacity(seats.len());
    for seat in seats {
        let key = match codex::usage(&http, &seat.access_token, &seat.account_id).await {
            Ok(usage) => usage
                .windows()
                .first()
                .map(|w| (w.limit_window_seconds, w.used_percent as i64))
                .unwrap_or((i64::MAX, i64::MAX)),
            Err(_) => (i64::MAX, i64::MAX),
        };
        keyed.push((key.0, key.1, seat));
    }
    keyed.sort_by_key(|(window, used, _)| (*window, *used));
    for (window, used, seat) in &keyed {
        tracing::info!(
            seat = %seat.fingerprint,
            fenetre_s = window,
            utilise_pct = used,
            "luna_ordre_des_sieges"
        );
    }
    keyed.into_iter().map(|(_, _, seat)| seat).collect()
}

/// The seats of the subscription door, or `None` when the base holds no live one.
///
/// A seat in `caps`, by tenant or fingerprint, fills each window up to its share
/// only, and serves even when parked by hand.
async fn luna_client(
    repo: &DecisionRepository<'_>,
    picked: &impl Fn(&Credential) -> bool,
    caps: &[(String, f64)],
) -> Result<Option<Arc<LunaClient>>> {
    let cap_of = |c: &Credential| {
        caps.iter()
            .find(|(seat, _)| *seat == c.tenant || *seat == c.fingerprint)
            .map(|(_, cap)| *cap)
    };
    let seat_of = |c: &Credential| {
        Some(LunaSeat {
            fingerprint: c.fingerprint.clone(),
            account_id: c.account.clone()?,
            access_token: c.access_token()?.to_string(),
            refresh_token: c.refresh_token()?.to_string(),
            cap_percent: cap_of(c),
        })
    };
    let now = chrono::Utc::now();
    let mut seats = Vec::new();
    let mut quota_parked = Vec::new();
    for c in repo
        .all_credentials("openai")
        .await?
        .iter()
        .filter(|c| picked(c))
    {
        let Some(seat) = seat_of(c) else { continue };
        match c.disabled_until {
            Some(_) if crate::credential::parked_until_enabled(c) && seat.cap_percent.is_some() => {
                seats.push(seat)
            }
            Some(until) if until > now => {
                if !crate::credential::parked_until_enabled(c) {
                    quota_parked.push(seat);
                }
            }
            _ => seats.push(seat),
        }
    }
    seats.extend(reopened(repo, quota_parked).await?);
    if seats.is_empty() {
        return Ok(None);
    }
    Ok(Some(Arc::new(LunaClient::new(by_expiry(seats).await)?)))
}

/// The seats a 429 parked whose windows all have room again.
async fn reopened(repo: &DecisionRepository<'_>, parked: Vec<LunaSeat>) -> Result<Vec<LunaSeat>> {
    if parked.is_empty() {
        return Ok(parked);
    }
    let http = reqwest::Client::builder()
        .user_agent("librejustice-ingest")
        .build()?;
    let mut back = Vec::new();
    for seat in parked {
        let Ok(usage) = codex::usage(&http, &seat.access_token, &seat.account_id).await else {
            continue;
        };
        let used: Vec<f64> = usage.windows().iter().map(|w| w.used_percent).collect();
        if !windows_free(&used) {
            continue;
        }
        repo.enable_credential(&seat.fingerprint).await?;
        tracing::warn!(seat = %seat.fingerprint, used = ?used, "luna_seat_unparked_early");
        back.push(seat);
    }
    Ok(back)
}

/// Every window the service reports has room left, and it reports at least one.
fn windows_free(used_percent: &[f64]) -> bool {
    !used_percent.is_empty() && used_percent.iter().all(|u| *u < 100.0)
}

/// The door refuses a call without an effort, which every luna engine declares.
fn luna_effort(engine: SummaryEngine) -> &'static str {
    engine
        .thinking()
        .wire()
        .expect("un moteur luna déclare son niveau de réflexion")
}

/// Identifiers per pass of the re-cleaning. The whole stock is read, and only what
/// the cleaning changes is written back.
const RECLEAN_BATCH: i64 = 50_000;

/// Moves the summaries generations 8 to 11 wrote in French on texts in another
/// language to those texts' French reader summary, one transaction per batch.
pub async fn move_reader_summaries(batch: i64) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute(
        "SET statement_timeout = '300s'; SET application_name = 'lj-move-reader-summaries'",
    )
    .await
    .map_err(|e| anyhow!("set session: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let (mut moved, mut requeued) = (0i64, 0i64);
    loop {
        let (texts, decisions) = repo
            .move_reader_summaries(
                lj_summary::FRENCH_SUMMARY_GENERATION,
                lj_chunk::EMBED_VERSION,
                batch,
            )
            .await?;
        moved += texts;
        requeued += decisions;
        tracing::info!(moved, requeued, "reader_summaries_moved");
        if texts < batch {
            return Ok(());
        }
    }
}

/// Replays [`clean_summary`] over the stored summaries and writes back the ones it
/// changes, without touching the engine that wrote them. A collage gets back where
/// its commentary sits too, which the reader sees set apart from the model's words.
///
/// The cleaning is deterministic and gains rules: one added upstream leaves the stock
/// behind until this runs.
pub async fn reclean_summaries() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let read = pool
        .get()
        .await
        .map_err(|e| anyhow!("pool.get read: {e}"))?;
    let write = pool
        .get()
        .await
        .map_err(|e| anyhow!("pool.get write: {e}"))?;
    read.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let read_repo = DecisionRepository::new(&read);
    let write_repo = DecisionRepository::new(&write);
    let Some((min_id, max_id)) = read_repo.decision_id_bounds().await? else {
        return Ok(());
    };
    let (mut seen, mut repaired, mut repasted, mut unplaced, mut bare) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut from = min_id;
    while from <= max_id {
        let to = from.saturating_add(RECLEAN_BATCH);
        let commentaries: std::collections::HashMap<i64, (String, bool)> = read_repo
            .collage_commentaries_in_range(
                from,
                to,
                &SummaryEngine::COLLAGES.map(SummaryEngine::code),
                COLLAGE_PROSE_MIN as i32,
            )
            .await?
            .into_iter()
            .map(|(id, raw, analysis)| (id, (raw, analysis)))
            .collect();
        for stored in read_repo.summaries_in_range(from, to).await? {
            let StoredSummary {
                decision_id: id,
                summary,
                body: stored_body,
                quote: stored_quote,
            } = stored;
            seen += 1;
            let raw = commentaries.get(&id);
            let commentary = match raw.map(|(raw, analysis)| court_commentary(raw, *analysis)) {
                Some(None) => {
                    bare += 1;
                    continue;
                }
                commentary => commentary.flatten(),
            };
            let Some(commentary) = commentary else {
                // A court's summary pasted whole stays as it was pasted.
                if stored_quote.is_some() {
                    continue;
                }
                let (cleaned, body) = match stored_body.clone() {
                    Some(at) => match reclean_parts(&summary, at) {
                        Some(written) => (written.summary, Some(written.body)),
                        None => {
                            unplaced += 1;
                            (clean_summary(&summary), None)
                        }
                    },
                    None => (clean_summary(&summary), None),
                };
                if cleaned != summary || body != stored_body {
                    write_repo
                        .rewrite_summary_text(id, &cleaned, body, None)
                        .await?;
                    repaired += 1;
                }
                continue;
            };
            let mut cleaned = clean_summary(&summary);
            // The stored quote says where the paste sits, which a commentary cut or
            // recased since no longer finds; without one, the paste is found by its text.
            let swapped = match (raw, stored_quote.clone()) {
                (_, Some(at)) => repaste_quote(&summary, at, &commentary),
                (Some((raw, _)), None) => {
                    if let Some(closed) = repair_collage(&cleaned, raw)
                        .or_else(|| repair_collage(&cleaned, &commentary))
                    {
                        cleaned = closed;
                    }
                    (*raw != commentary)
                        .then(|| repaste_collage(&cleaned, raw, &commentary))
                        .flatten()
                }
                _ => None,
            };
            if let Some(swapped) = swapped.filter(|swapped| *swapped != cleaned) {
                cleaned = swapped;
                repasted += 1;
            }
            let quote = collage_quote(&cleaned, &commentary);
            if quote.is_none() {
                unplaced += 1;
            }
            if cleaned != summary || quote != stored_quote || quote != stored_body {
                write_repo
                    .rewrite_summary_text(id, &cleaned, quote.clone(), quote)
                    .await?;
                repaired += 1;
            }
        }
        from = to;
    }
    tracing::warn!(
        lues = seen,
        reparees = repaired,
        repasted_collages = repasted,
        unplaced_collages = unplaced,
        collages_without_resume = bare,
        "resumes_renettoyes"
    );
    Ok(())
}

/// Overwrites the summary of the decisions whose juridiction published its own
/// commentary, by pasting that commentary rather than passing it through a model.
///
/// One door serves it, the token metered one, because the call is two sentences and
/// a request metered door would spend a whole request on it. The collage beats
/// rewriting the same decisions.
pub async fn upgrade_summaries_commentaries(batch_size: i64, limit: Option<i64>) -> Result<()> {
    let doors = Doors::All;
    let _run = Run::start();
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let fetch_conn = pool
        .get()
        .await
        .map_err(|e| anyhow!("pool.get fetch: {e}"))?;
    let write_conn = pool
        .get()
        .await
        .map_err(|e| anyhow!("pool.get write: {e}"))?;
    fetch_conn
        .batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout fetch: {e}"))?;
    write_conn
        .batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout write: {e}"))?;
    let fetch_repo = DecisionRepository::new(&fetch_conn);
    let write_repo = DecisionRepository::new(&write_conn);

    let luna = luna_client(&fetch_repo, &|_: &Credential| true, &[])
        .await?
        .ok_or_else(|| anyhow!("aucun siège luna dans llm_credential : porte inutilisable"))?;
    let pools: Vec<Arc<Pool>> = vec![Arc::new(Pool::Luna(Arc::clone(&luna), LUNA_COLLAGE_ONLY))];
    tracing::info!(
        seats = luna.key_count(),
        engine = LUNA_COLLAGE_ONLY.any().code(),
        "collage_backfill_pools"
    );

    let start = std::time::Instant::now();
    let mut todo = fetch_repo
        .collage_queue(
            &SummaryEngine::COLLAGES.map(SummaryEngine::code),
            COLLAGE_PROSE_MIN as i32,
        )
        .await?;
    if let Some(max) = limit {
        todo.truncate(max as usize);
    }
    tracing::info!(
        queued = todo.len(),
        elapsed = %format_eta(start.elapsed().as_secs_f64()),
        "collage_queue_built"
    );

    let mut processed: usize = 0;
    let mut persisted: usize = 0;
    for chunk in todo.chunks(batch_size as usize) {
        let page: Vec<CollageRow> = fetch_repo
            .fetch_collage_rows(chunk, COLLAGE_PROSE_MIN as i32)
            .await?;
        let mut headers = headers(&fetch_repo, &fetch_repo.summary_rows(chunk).await?).await?;

        let queue: Queue = Arc::new(std::sync::Mutex::new(Files::default()));
        for row in page.iter().filter(|row| COLLAGE.writes(&row.lang)) {
            let Some(commentary) = court_commentary(&row.commentary, row.analysis) else {
                tracing::warn!(decision_id = row.decision_id, "collage_no_resume");
                continue;
            };
            let Some(header) = headers.remove(&row.decision_id) else {
                tracing::warn!(decision_id = row.decision_id, "collage_no_public_id");
                continue;
            };
            match body_text_from_full_text(&row.full_text) {
                Some(body) => queue.lock().expect("file non empoisonnée").push_back(Job {
                    decision_id: row.decision_id,
                    lang: row.lang.clone(),
                    reader: None,
                    body,
                    work: Work::Collage { header, commentary },
                    attempts: 0,
                    unavailable: 0,
                }),
                None => tracing::warn!(decision_id = row.decision_id, "collage_no_body"),
            }
        }

        let jobs = queue.lock().expect("file non empoisonnée").len();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Done>(64);
        let mut running = Vec::new();
        let ahead: Ahead = Arc::new(std::sync::atomic::AtomicUsize::new(
            pools
                .iter()
                .filter(|p| p.rank() == 0)
                .map(|p| p.drivers(jobs))
                .sum(),
        ));
        for pool in &pools {
            for _ in 0..pool.drivers(jobs) {
                running.push(tokio::spawn(drive(
                    Arc::clone(pool),
                    Arc::clone(&queue),
                    tx.clone(),
                    Arc::clone(&ahead),
                    doors,
                    None,
                )));
            }
        }
        drop(tx);

        processed += page.len();
        while let Some(done) = rx.recv().await {
            write_repo
                .set_summary(
                    done.decision_id,
                    &done.lang,
                    &done.written.summary,
                    Some(done.written.body),
                    done.quote,
                    done.engine.prompt_version(),
                    done.engine.code(),
                )
                .await
                .map_err(|e| anyhow!("set_summary id={}: {e}", done.decision_id))?;
            persisted += 1;
        }
        for driver in running {
            driver.await.map_err(|e| anyhow!("join: {e}"))?;
        }

        let remaining = queue.lock().expect("file non empoisonnée").len();
        flush_luna_rotations(&write_repo, Some(&luna)).await?;
        let elapsed = start.elapsed().as_secs_f64();
        tracing::info!(
            processed,
            persisted,
            queued = todo.len(),
            rate = processed as f64 / elapsed.max(1e-6),
            elapsed = %format_eta(elapsed),
            "collage_progress"
        );
        log_demand(&pools);
        if STOPPING.load(Ordering::Relaxed) {
            tracing::warn!(remaining, "collage_stopped");
            break;
        }
        if remaining > 0 {
            tracing::warn!(remaining, "collage_pool_handed_over");
            break;
        }
    }

    flush_luna_rotations(&write_repo, Some(&luna)).await?;
    tracing::info!(
        processed,
        persisted,
        elapsed = %format_eta(start.elapsed().as_secs_f64()),
        "collage_done"
    );
    Ok(())
}

/// What the quota justifies holding against what the pool was given, which is the
/// reading an adaptive driver count would act on.
fn log_demand(pools: &[Arc<Pool>]) {
    for pool in pools {
        let (demande, muettes) = pool.demand();
        tracing::info!(
            model = pool.engines().any().model(),
            cles = pool.keys(),
            demande,
            muettes,
            "summary_pool_demand"
        );
    }
}

/// Emitted per batch: the daily chain caps this command with a watchdog, so a report kept for the clean exit is one nobody reads.
fn log_gemini_quota(gemini: &[(Arc<GeminiClient>, Engines)]) {
    for (client, engines) in gemini {
        let quota = client.refused_quota().unwrap_or_default();
        tracing::info!(
            model = engines.any().model(),
            refus = client.refusals(),
            refus_nus = client.bare_refusals(),
            quota = %quota,
            rate_par_min = client.rate_per_min(),
            budget_restant = client.day_left(),
            "summary_gemini_quota"
        );
    }
}

/// What a seat cost: the door answers only a percentage of a window, so a seat
/// emptied in one run otherwise leaves no trace of the value it held.
fn log_luna_seats(luna: Option<&Arc<LunaClient>>) {
    let Some(client) = luna else {
        return;
    };
    for seat in client.spent_per_seat() {
        tracing::info!(
            seat = %seat.fingerprint,
            calls = seat.calls,
            input = seat.usage.input,
            cached = seat.usage.cached,
            cache_write = seat.usage.cache_write,
            output = seat.usage.output,
            "luna_seat_spent"
        );
    }
}

/// `<model>=<n>` per pool, which pool served what.
fn format_par_moteur(counts: &std::collections::BTreeMap<&'static str, usize>) -> String {
    counts
        .iter()
        .map(|(model, n)| format!("{model}={n}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Per batch and never per generation: one round trip for a thousand summaries.
async fn flush_gemini_day(
    repo: &DecisionRepository<'_>,
    gemini: &[(Arc<GeminiClient>, Engines)],
) -> Result<()> {
    for (client, engines) in gemini {
        let pending = client.day_pending();
        if pending.is_empty() {
            continue;
        }
        let totals = repo
            .bump_gemini_day_spent(engines.any().model(), &pending)
            .await?;
        client.day_flushed(&pending, &totals);
    }
    Ok(())
}

/// Writes back the seats luna refreshed. The refresh token is single use, so one
/// kept in memory dies with the process.
async fn flush_luna_rotations(
    repo: &DecisionRepository<'_>,
    luna: Option<&LunaClient>,
) -> Result<()> {
    let Some(client) = luna else {
        return Ok(());
    };
    for rotation in client.take_rotations() {
        let secret = serde_json::json!({
            "access_token": rotation.access_token,
            "refresh_token": rotation.refresh_token,
        });
        repo.merge_credential_secret(&rotation.fingerprint, &secret)
            .await?;
        tracing::info!(fingerprint = %rotation.fingerprint, "luna_seat_rotated");
    }
    Ok(())
}

/// What a run would send for the decisions of a bench file, a JSON list of objects
/// carrying their `id`: the inputs of both shapes, and the parts they are built from.
pub async fn export_inputs(bench: &std::path::Path, out: &std::path::Path) -> Result<()> {
    let ids: Vec<i64> = serde_json::from_slice::<Vec<serde_json::Value>>(&std::fs::read(bench)?)?
        .iter()
        .map(|decision| {
            let id = &decision["id"];
            id.as_i64()
                .or_else(|| id.as_str()?.parse().ok())
                .ok_or_else(|| anyhow!("id illisible : {id}"))
        })
        .collect::<Result<_>>()?;
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let rows = repo.summary_rows(&ids).await?;
    let mut headers = headers(&repo, &rows).await?;
    let full_texts: std::collections::HashMap<i64, PickedText> = repo
        .fetch_full_texts(&ids)
        .await?
        .into_iter()
        .map(|text| (text.decision_id, text))
        .collect();
    let commentaries: std::collections::HashMap<i64, String> = repo
        .fetch_collage_rows(&ids, COLLAGE_PROSE_MIN as i32)
        .await?
        .into_iter()
        .filter_map(|row| {
            court_commentary(&row.commentary, row.analysis)
                .map(|commentary| (row.decision_id, commentary))
        })
        .collect();
    let mut inputs = serde_json::Map::new();
    for row in &rows {
        let Some((text, body)) = full_texts.get(&row.decision_id).and_then(|text| {
            let body = body_text_from_full_text(&text.full_text)?;
            Some((text, body))
        }) else {
            continue;
        };
        let header = headers
            .remove(&row.decision_id)
            .expect("one header per row");
        let commentary = commentaries.get(&row.decision_id).cloned();
        let collage = commentary.clone().map(|commentary| {
            Work::Collage {
                header: header.clone(),
                commentary,
            }
            .input(&body)
        });
        inputs.insert(
            row.decision_id.to_string(),
            serde_json::json!({
                "lang": text.lang,
                "title": header.title,
                "case_name": header.case_name,
                "classification": header.classification,
                "body": body,
                "commentary": commentary,
                "collage": collage,
                "rewrite": Work::Rewrite { header }.input(&body),
            }),
        );
    }
    tracing::info!(asked = ids.len(), written = inputs.len(), "summary_inputs");
    std::fs::write(out, serde_json::to_vec(&inputs)?)?;
    Ok(())
}

/// How [`finish_answers`] reads an answer.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum Answer {
    /// One block, as the generation 10 rewrite wrote it.
    Block,
    /// Opening, body and outcome on three lines.
    Parts,
    /// Opening and outcome around the commentary of [`export_inputs`].
    Collage,
}

/// What the store would hold for a bench's raw answers, `{id: {"summary": raw}}`,
/// written as `{id: {"summary", "body"}}`.
pub fn finish_answers(
    answer: Answer,
    inputs: &std::path::Path,
    answers: &std::path::Path,
    out: &std::path::Path,
) -> Result<()> {
    let inputs: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&std::fs::read(inputs)?)?;
    let answers: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&std::fs::read(answers)?)?;
    let mut finished = serde_json::Map::new();
    for (id, raw) in &answers {
        let raw = raw["summary"].as_str().unwrap_or_default();
        let written = match answer {
            Answer::Block => Some(Written {
                summary: clean_summary(raw),
                body: 0..0,
            })
            .filter(|written| !written.summary.is_empty()),
            Answer::Parts => assemble_rewrite(raw),
            Answer::Collage => inputs
                .get(id)
                .and_then(|input| input["commentary"].as_str())
                .and_then(|commentary| assemble_collage(raw, commentary)),
        };
        match written {
            Some(written) => {
                finished.insert(
                    id.clone(),
                    serde_json::json!({
                        "summary": written.summary,
                        "body": [written.body.start, written.body.end],
                    }),
                );
            }
            None => tracing::warn!(id, "summary_answer_unfinished"),
        }
    }
    tracing::info!(
        answers = answers.len(),
        finished = finished.len(),
        "summary_finish"
    );
    std::fs::write(out, serde_json::to_vec(&finished)?)?;
    Ok(())
}

/// Titre lisible d'une ligne de backfill, injecté dans le prompt.
/// `docket_numbers` → premier numéro éventuel.
fn row_title(row: &MissingSummaryRow) -> String {
    decision_title(
        &row.jurisdiction_type,
        row.jurisdiction_name.as_deref(),
        row.date_lecture.as_deref(),
        row.docket_numbers.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parked_seat_reopens_only_with_room_in_every_window() {
        assert!(windows_free(&[0.0, 2.0]));
        assert!(!windows_free(&[100.0, 0.0]));
        assert!(!windows_free(&[10.0, 100.0]));
        assert!(!windows_free(&[]));
    }

    #[test]
    fn eta_format() {
        assert_eq!(format_eta(0.0), "0h00m00s");
        assert_eq!(format_eta(3661.0), "1h01m01s");
        assert_eq!(format_eta(-5.0), "0h00m00s");
    }

    /// A gemini client is built for one model and one level, so a door carrying two
    /// engines would send one of them settings nobody graded. Luna names both per call.
    #[test]
    fn une_porte_gemini_ne_porte_qu_un_reglage_et_luna_nomme_le_sien() {
        for model in [Model::Gemini31FlashLite, Model::Gemini35FlashLite] {
            let engines = model.engines();
            if let (Some(rewrite), Some(collage)) = (engines.rewrite, engines.collage) {
                assert_eq!(rewrite.model(), collage.model(), "{rewrite:?}/{collage:?}");
                assert_eq!(rewrite.thinking(), collage.thinking());
            }
        }
        for engines in [Model::Gpt6Luna.engines(), LUNA_COLLAGE_ONLY] {
            for engine in [engines.rewrite, engines.collage].into_iter().flatten() {
                luna_effort(engine);
            }
        }
    }

    /// A door never sees a file it has no engine for, and takes its own shape first.
    #[test]
    fn la_file_sert_la_forme_que_la_porte_prefere_puis_l_autre() {
        let body = || "corps".to_string();
        let header = || Header {
            title: "titre".into(),
            case_name: None,
            classification: Vec::new(),
        };
        let collage = || Job {
            decision_id: 1,
            lang: "fr".into(),
            reader: None,
            body: body(),
            work: Work::Collage {
                header: header(),
                commentary: "sommaire".into(),
            },
            attempts: 0,
            unavailable: 0,
        };
        let rewrite = || Job {
            decision_id: 2,
            lang: "fr".into(),
            reader: None,
            body: body(),
            work: Work::Rewrite { header: header() },
            attempts: 0,
            unavailable: 0,
        };

        let mut files = Files::default();
        files.push_back(collage());
        files.push_back(rewrite());
        let gemini = Pool::Gemini(
            Arc::new(GeminiClient::new(vec!["k".into()], "m".into(), None).unwrap()),
            Model::Gemini31FlashLite.engines(),
        );
        assert_eq!(files.take(&gemini).map(|j| j.decision_id), Some(2));
        assert_eq!(files.take(&gemini).map(|j| j.decision_id), None);
        assert_eq!(files.len(), 1);
    }
}
