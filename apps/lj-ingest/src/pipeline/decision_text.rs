//! Rewrites the decision texts an unchanged DILA member now reads differently, or the CJUE texts stored with references the strip now decodes or a table of contents, and the fields extracted from them. A CJUE footnote call an old strip split over three lines is joined back in place, at the same length. A DILA member that no longer reads as a decision retires its provenance. The French texts stored in capitals are recased in place, and the opendata texts whose « par » a pseudonymiser lettered get it back.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{anyhow, Result};
use flate2::write::GzEncoder;
use flate2::Compression;
use lj_store::repository::{
    DecisionRepository, DecisionTextDrift, DecisionTextRead, ExtractedFields, RepairText,
    StoredText,
};
use rayon::prelude::*;
use regex::Regex;
use serde_json::Value;
use tokio::sync::mpsc;

use super::dila::{classify_dila, ClassifiedDila, Fond};
use super::ExtractCtx;
use crate::config::Settings;

/// Texts compared per round trip.
const BATCH: usize = 1_000;
/// Members parsed per rayon round.
const CHUNK: usize = 4_096;
/// Decisions written per transaction: the bulk field update binds about twenty
/// parameters a decision, under Postgres's cap on the parameters of a statement.
const REWRITE_BATCH: usize = 256;

/// A fond whose decision bodies are read as HTML: three DILA ones, reread from
/// their archives, and the CJUE, whose stored texts are decoded again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum TextFond {
    Jade,
    Constit,
    Cnil,
    Cjue,
}

impl TextFond {
    fn dila(self) -> Option<Fond> {
        match self {
            Self::Jade => Some(Fond::Jade),
            Self::Constit => Some(Fond::Constit),
            Self::Cnil => Some(Fond::Cnil),
            Self::Cjue => None,
        }
    }
}

/// Stored CJUE texts read per round trip.
const CJUE_PAGE: i64 = 500;

/// Rereads `fond`, or every one, and rewrites the read texts that read
/// differently; `dry_run` counts them and writes nothing.
pub async fn backfill_decision_text(fond: Option<TextFond>, dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("session setup: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let ctx = if dry_run {
        None
    } else {
        Some(super::extract_ctx(&conn).await?)
    };
    let fonds = match fond {
        Some(f) => vec![f],
        None => vec![
            TextFond::Jade,
            TextFond::Constit,
            TextFond::Cnil,
            TextFond::Cjue,
        ],
    };
    let mut written = Vec::new();
    for fond in fonds {
        written.extend(backfill_fond(&repo, &settings, fond, ctx).await?);
    }
    if ctx.is_some() {
        let annuaire = repo
            .refresh_annuaire_scoped(&written)
            .await
            .map_err(|e| anyhow!("refresh_annuaire_scoped: {e}"))?;
        tracing::info!(annuaire, "backfill-decision-text: directory recounted");
    }
    Ok(())
}

/// Decisions whose texts are read per round trip of a repair.
const REPAIR_WINDOW: i64 = 20_000;

/// A rewrite of the stored texts the ingest now writes differently.
#[derive(Debug, Clone, Copy)]
pub enum TextRepair {
    /// The French texts stored in capitals, recased.
    Recase,
    /// The opendata texts whose « par » a pseudonymiser took for a name, with it written back.
    Par,
}

impl TextRepair {
    fn name(self) -> &'static str {
        match self {
            Self::Recase => "recase-texts",
            Self::Par => "restore-par",
        }
    }

    async fn window(
        self,
        repo: &DecisionRepository<'_>,
        after: i64,
        upto: i64,
    ) -> Result<Vec<RepairText>> {
        match self {
            Self::Recase => repo.texts_in_capitals(after, upto).await,
            Self::Par => repo.texts_lettering_par(after, upto).await,
        }
        .map_err(|e| anyhow!("{} window: {e}", self.name()))
    }

    fn repaired(self, text: &str) -> Option<String> {
        match self {
            Self::Recase => lj_core::truecase::recased_text(text),
            Self::Par => lj_core::lettered_par::restored_par(text),
        }
    }
}

/// The stored versions of the texts a repair rewrites, a gzip member per window
/// written before it, under the state's `backup/` and copied to the bucket.
struct Trace {
    name: String,
    path: PathBuf,
    file: std::fs::File,
}

impl Trace {
    fn open(settings: &Settings, repair: TextRepair) -> Result<Self> {
        let name = format!(
            "{}-{}.jsonl.gz",
            repair.name(),
            chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
        );
        let dir = settings.paths().root().join("backup");
        std::fs::create_dir_all(&dir).map_err(|e| anyhow!("{}: {e}", dir.display()))?;
        let path = dir.join(&name);
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)
            .map_err(|e| anyhow!("{}: {e}", path.display()))?;
        Ok(Self { name, path, file })
    }

    fn write(&mut self, repaired: &[(RepairText, String)]) -> Result<()> {
        let mut gz = GzEncoder::new(&mut self.file, Compression::default());
        for (t, _) in repaired {
            let line = serde_json::json!({
                "text_id": t.text_id,
                "decision_id": t.decision_id,
                "full_text": t.full_text,
            });
            serde_json::to_writer(&mut gz, &line)?;
            gz.write_all(b"\n")?;
        }
        gz.finish()?;
        self.file.sync_data()?;
        Ok(())
    }

    async fn upload(self, settings: &Settings) -> Result<()> {
        let bytes =
            std::fs::read(&self.path).map_err(|e| anyhow!("{}: {e}", self.path.display()))?;
        settings
            .bucket()?
            .put(&format!("backup/{}", self.name), bytes)
            .await
            .map_err(|e| anyhow!("bucket put {}: {e}", self.name))?;
        tracing::info!(trace = %self.path.display(), "repair-texts: trace kept");
        Ok(())
    }
}

/// Rewrites the stored texts `repair` changes, as the ingest now writes them,
/// with the fields of the decisions read in them, each traced before it is
/// written; `dry_run` counts them and writes nothing.
pub async fn repair_texts(repair: TextRepair, dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("session setup: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let ctx = if dry_run {
        None
    } else {
        Some(super::extract_ctx(&conn).await?)
    };
    let upto = repo
        .max_decision_id()
        .await
        .map_err(|e| anyhow!("max_decision_id: {e}"))?;
    let mut trace = ctx
        .is_some()
        .then(|| Trace::open(&settings, repair))
        .transpose()?;
    let mut tally = Tally::default();
    let (mut candidates, mut fields_kept, mut bytes_added) = (0usize, 0u64, 0i64);
    let mut after = 0;
    while after < upto {
        let window = repair.window(&repo, after, after + REPAIR_WINDOW).await?;
        after += REPAIR_WINDOW;
        candidates += window.len();
        let repaired: Vec<(RepairText, String)> = tokio::task::spawn_blocking(move || {
            window
                .into_par_iter()
                .filter_map(|t| repair.repaired(&t.full_text).map(|r| (t, r)))
                .collect()
        })
        .await
        .map_err(|e| anyhow!("{} join: {e}", repair.name()))?;
        bytes_added += repaired
            .iter()
            .map(|(t, r)| r.len() as i64 - t.full_text.len() as i64)
            .sum::<i64>();
        if let Some(trace) = trace.as_mut().filter(|_| !repaired.is_empty()) {
            trace.write(&repaired)?;
        }
        let (read, held): (Vec<_>, Vec<_>) = repaired
            .into_iter()
            .map(|(t, full_text)| {
                tally.drifted.insert(t.text_id);
                let drift = DecisionTextDrift {
                    text_id: t.text_id,
                    decision_id: t.decision_id,
                    stored: t.full_text,
                    full_text,
                };
                (t.extracted, drift)
            })
            .partition(|(extracted, _)| *extracted);
        let read = read.into_iter().map(|(_, d)| d).collect();
        rewrite_drifts(&repo, read, ctx, &mut tally).await?;
        let held: Vec<DecisionTextDrift> = held.into_iter().map(|(_, d)| d).collect();
        fields_kept += if ctx.is_some() {
            repo.overwrite_texts(&held)
                .await
                .map_err(|e| anyhow!("overwrite_texts: {e}"))?
        } else {
            held.len() as u64
        };
        tracing::info!(
            repair = repair.name(),
            after,
            upto,
            repaired = tally.drifted.len(),
            bytes_added,
            "repair-texts: window read"
        );
    }
    if let Some(trace) = trace {
        trace.upload(&settings).await?;
    }
    if ctx.is_some() {
        let annuaire = repo
            .refresh_annuaire_scoped(&tally.written)
            .await
            .map_err(|e| anyhow!("refresh_annuaire_scoped: {e}"))?;
        tracing::info!(
            repair = repair.name(),
            annuaire,
            "repair-texts: directory recounted"
        );
    }
    tracing::warn!(
        repair = repair.name(),
        dry_run,
        candidates,
        repaired = tally.drifted.len(),
        written = tally.written.len(),
        unextracted = tally.unextracted,
        fields_kept,
        bytes_added,
        "repair-texts"
    );
    Ok(())
}

/// Decisions whose texts the annex heading is looked for in per round trip.
const ANNEX_WINDOW: i64 = 20_000;

/// Writes where the moyens annexés of each stored text start, the boundary
/// every text write now sets, and the tags of the passages it splits.
/// Replayable.
pub async fn annex_starts() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("session setup: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let upto = repo
        .max_decision_id()
        .await
        .map_err(|e| anyhow!("max_decision_id: {e}"))?;
    let (mut read, mut annexed, mut written, mut retagged) = (0usize, 0usize, 0u64, 0u64);
    let mut after = 0;
    while after < upto {
        let window = repo
            .texts_with_annexes(after, after + ANNEX_WINDOW)
            .await
            .map_err(|e| anyhow!("texts_with_annexes: {e}"))?;
        after += ANNEX_WINDOW;
        read += window.len();
        let mut starts: Vec<(i64, Option<i32>)> = Vec::with_capacity(window.len());
        let mut boundaries: HashMap<i64, Option<usize>> = HashMap::new();
        for (text_id, decision_id, cut, text) in window {
            let start = lj_core::annexes::start(&text);
            starts.push((text_id, start.map(|at| at as i32)));
            if cut {
                boundaries.insert(decision_id, start);
            }
        }
        annexed += starts.iter().filter(|s| s.1.is_some()).count();
        written += repo
            .set_annex_starts(&starts)
            .await
            .map_err(|e| anyhow!("set_annex_starts: {e}"))?;
        let ids: Vec<i64> = boundaries.keys().copied().collect();
        let zones: Vec<(i64, Vec<&'static str>)> = repo
            .passages_of(&ids)
            .await
            .map_err(|e| anyhow!("passages_of: {e}"))?
            .into_iter()
            .map(|(id, decision_id, start, end, zones)| {
                let summary = zones.iter().any(|z| z == lj_vocab::passage_zones::SUMMARY);
                let boundary = boundaries[&decision_id];
                (
                    id,
                    lj_chunk::zones(start as usize, end as usize, summary, boundary),
                )
            })
            .collect();
        retagged += repo
            .set_passage_zones(&zones)
            .await
            .map_err(|e| anyhow!("set_passage_zones: {e}"))?;
        tracing::info!(
            after,
            upto,
            read,
            annexed,
            written,
            retagged,
            "annex-starts: window read"
        );
    }
    tracing::warn!(read, annexed, written, retagged, "annex-starts");
    Ok(())
}

/// Decisions whose passages are tagged per round trip, one in twenty-five
/// carrying a headnote.
const HEADNOTE_WINDOW: i64 = 100_000;

/// Tags the passages of the decisions whose text carries the court's summary,
/// a tag the base keeps from then on. Replayable.
pub async fn headnote_tags() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("session setup: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let upto = repo
        .max_decision_id()
        .await
        .map_err(|e| anyhow!("max_decision_id: {e}"))?;
    let (mut tagged, mut after) = (0u64, 0);
    while after < upto {
        tagged += repo
            .tag_headnote_passages(after, after + HEADNOTE_WINDOW)
            .await
            .map_err(|e| anyhow!("tag_headnote_passages: {e}"))?;
        after += HEADNOTE_WINDOW;
        tracing::info!(after, upto, tagged, "headnote-tags: window read");
    }
    tracing::warn!(tagged, "headnote-tags");
    Ok(())
}

#[derive(Default)]
struct Tally {
    archives: usize,
    members: usize,
    unreadable: usize,
    texts: usize,
    /// Each drifted text once, though members repeat across archives.
    drifted: HashSet<i64>,
    unextracted: usize,
    written: Vec<i64>,
    /// The texts other than their decision's read text, which carry no field.
    translations: u64,
    /// The provenances whose member, in its last version, gives no decision.
    retired: usize,
    /// The footnote calls joined back into their line.
    calls: usize,
    /// The texts whose only change is a joined call, written in place, every offset unchanged.
    joined: u64,
}

/// What a member gives as the ingest reads it.
enum Member {
    /// Its provenance, whether it reads as a decision, and its full texts; an
    /// analysis alone is a decision and keeps its reading.
    Read {
        source_uid: String,
        decision: bool,
        texts: Vec<DecisionTextRead>,
    },
    Empty,
    Unreadable,
}

async fn backfill_fond(
    repo: &DecisionRepository<'_>,
    settings: &Settings,
    fond: TextFond,
    ctx: Option<&'static ExtractCtx>,
) -> Result<Vec<i64>> {
    let mut tally = Tally::default();
    match fond.dila() {
        Some(dila) => reread_archives(repo, settings, dila, ctx, &mut tally).await?,
        None => decode_cjue(repo, ctx, &mut tally).await?,
    }
    tracing::warn!(
        fond = ?fond,
        dry_run = ctx.is_none(),
        archives = tally.archives,
        members = tally.members,
        unreadable = tally.unreadable,
        texts = tally.texts,
        drifted = tally.drifted.len(),
        unextracted = tally.unextracted,
        written = tally.written.len(),
        translations = tally.translations,
        retired = tally.retired,
        calls = tally.calls,
        joined = tally.joined,
        "backfill-decision-text"
    );
    Ok(tally.written)
}

/// Decodes again the CJUE texts holding a reference, and drops the table of contents a catalogue
/// rendition opens on: the stock was stripped by a decoder that knew few names, and EUR-Lex is
/// not asked twice.
async fn decode_cjue(
    repo: &DecisionRepository<'_>,
    ctx: Option<&'static ExtractCtx>,
    tally: &mut Tally,
) -> Result<()> {
    let mut after = String::new();
    loop {
        let page: Vec<StoredText> = repo
            .texts_to_clean("cjue", "cjuf", &after, CJUE_PAGE)
            .await
            .map_err(|e| anyhow!("texts_to_clean: {e}"))?;
        let Some(last) = page.last() else {
            return Ok(());
        };
        after = last.source_uid.clone();
        tally.members += page.len();
        let mut read = Vec::new();
        let mut translations = Vec::new();
        let mut in_place = Vec::new();
        for text in page {
            let decoded = lj_sources::cjue::drop_contents(
                &lj_sources::html_strip::decode_stripped(&text.full_text),
            )
            .to_string();
            let (joined, calls) = join_footnote_calls(&decoded);
            tally.calls += calls;
            let drift = DecisionTextDrift {
                text_id: text.text_id,
                decision_id: text.decision_id,
                stored: text.full_text,
                full_text: joined,
            };
            if decoded == drift.stored {
                if calls > 0 {
                    in_place.push(drift);
                }
            } else if text.read {
                read.push(DecisionTextRead {
                    source_uid: text.source_uid,
                    content_checksum: text.content_checksum,
                    full_text: drift.full_text,
                });
            } else {
                translations.push(drift);
            }
        }
        rewrite_texts(repo, read, ctx, tally).await?;
        tally.translations += overwrite(repo, &translations, ctx).await?;
        tally.joined += overwrite(repo, &in_place, ctx).await?;
    }
}

/// Writes `drifts` over their stored texts, fields and passages untouched, and counts them;
/// a dry run only counts.
async fn overwrite(
    repo: &DecisionRepository<'_>,
    drifts: &[DecisionTextDrift],
    ctx: Option<&'static ExtractCtx>,
) -> Result<u64> {
    if ctx.is_none() {
        return Ok(drifts.len() as u64);
    }
    repo.overwrite_texts(drifts)
        .await
        .map_err(|e| anyhow!("overwrite_texts: {e}"))
}

/// A footnote call an old strip split over three lines, `(`, `1`, `)`, joined back into its
/// line as `( 1 )`, two spaces for two line breaks; returns the text and the calls joined.
fn join_footnote_calls(text: &str) -> (String, usize) {
    static CALL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\(\n(\*?[0-9]{1,3}|\*|[ivx]{1,4})\n\)").unwrap());
    let calls = CALL.find_iter(text).count();
    if calls == 0 {
        return (text.to_string(), 0);
    }
    (CALL.replace_all(text, "( $1 )").into_owned(), calls)
}

async fn reread_archives(
    repo: &DecisionRepository<'_>,
    settings: &Settings,
    fond: Fond,
    ctx: Option<&'static ExtractCtx>,
    tally: &mut Tally,
) -> Result<()> {
    let paths = lj_sources::dila::cached_dila_tarballs(&settings.cache_dir(), fond.source())?;
    tally.archives = paths.len();
    let (tx, mut rx) = mpsc::channel::<Vec<Member>>(4);
    let reader = tokio::task::spawn_blocking(move || read_archives(fond, &paths, &tx));
    let mut reads: Vec<DecisionTextRead> = Vec::new();
    // The archives come in order, so a member's last version decides.
    let mut decision_of: HashMap<String, bool> = HashMap::new();
    while let Some(members) = rx.recv().await {
        for member in members {
            tally.members += 1;
            match member {
                Member::Read {
                    source_uid,
                    decision,
                    texts,
                } => {
                    decision_of.insert(source_uid, decision);
                    reads.extend(texts);
                }
                Member::Empty => {}
                Member::Unreadable => tally.unreadable += 1,
            }
        }
        while reads.len() >= BATCH {
            let batch: Vec<DecisionTextRead> = reads.drain(..BATCH).collect();
            rewrite_texts(repo, batch, ctx, tally).await?;
        }
    }
    reader
        .await
        .map_err(|e| anyhow!("backfill-decision-text reader: {e}"))??;
    rewrite_texts(repo, reads, ctx, tally).await?;

    let none: Vec<String> = decision_of
        .into_iter()
        .filter_map(|(uid, decision)| (!decision).then_some(uid))
        .collect();
    for batch in none.chunks(BATCH) {
        let held = repo
            .active_source_uids(batch)
            .await
            .map_err(|e| anyhow!("active_source_uids: {e}"))?;
        tally.retired += held.len();
        if ctx.is_none() {
            continue;
        }
        for source_uid in held {
            repo.delete(&source_uid)
                .await
                .map_err(|e| anyhow!("delete {source_uid}: {e}"))?;
        }
    }
    Ok(())
}

/// Streams the archives, parsing each chunk of members in parallel.
fn read_archives(fond: Fond, paths: &[PathBuf], tx: &mpsc::Sender<Vec<Member>>) -> Result<()> {
    for (i, path) in paths.iter().enumerate() {
        let send = |chunk: Vec<(String, Vec<u8>)>| -> Result<()> {
            let members = chunk
                .into_par_iter()
                .map(|(name, raw)| read_member(fond, &name, &raw))
                .collect();
            tx.blocking_send(members)
                .map_err(|_| anyhow!("backfill-decision-text: consumer stopped"))
        };
        let mut chunk: Vec<(String, Vec<u8>)> = Vec::with_capacity(CHUNK);
        lj_sources::tar_reader::for_each_member_filtered(
            path,
            |name| name.to_lowercase().ends_with(".xml"),
            |name, raw| {
                chunk.push((name, raw));
                if chunk.len() == CHUNK {
                    send(std::mem::take(&mut chunk))?;
                }
                Ok(())
            },
        )?;
        send(chunk)?;
        tracing::info!(archive = %path.display(), n = i + 1, of = paths.len(), "backfill-decision-text: archive read");
    }
    Ok(())
}

/// A member as the ingest reads it, its provenance named after its file, which
/// is its DILA identifier.
fn read_member(fond: Fond, name: &str, raw: &[u8]) -> Member {
    if raw.is_empty() {
        return Member::Empty;
    }
    let Some(id) = Path::new(name).file_stem().and_then(|s| s.to_str()) else {
        return Member::Empty;
    };
    let repaired = lj_sources::dila::repair_dila(raw, fond.source());
    match classify_dila(repaired, raw, fond) {
        Ok(classified) => Member::Read {
            source_uid: format!("{}/{id}", fond.source_prefix()),
            decision: !classified.is_empty(),
            texts: classified
                .into_iter()
                .filter_map(|c| match c {
                    ClassifiedDila::Full(c) => Some(DecisionTextRead {
                        source_uid: c.decision.source_uid,
                        content_checksum: c.content_checksum,
                        full_text: lj_core::truecase::recased_text(
                            &c.decision.texte_integral_clean,
                        )
                        .unwrap_or(c.decision.texte_integral_clean),
                    }),
                    ClassifiedDila::Analysis(_) => None,
                })
                .collect(),
        },
        Err(e) => {
            tracing::error!(member = %name, error = %e, "backfill-decision-text: parse failed");
            Member::Unreadable
        }
    }
}

type ReextractInputs = HashMap<i64, (Vec<(String, Value)>, Option<String>)>;

/// Writes the drifted texts of `reads` with their fields re-extracted, and
/// drops their passages for `embed-missing`; `None` context makes a dry run.
async fn rewrite_texts(
    repo: &DecisionRepository<'_>,
    reads: Vec<DecisionTextRead>,
    ctx: Option<&'static ExtractCtx>,
    tally: &mut Tally,
) -> Result<()> {
    tally.texts += reads.len();
    if reads.is_empty() {
        return Ok(());
    }
    let drifts: Vec<DecisionTextDrift> = repo
        .drifted_read_texts(&reads)
        .await
        .map_err(|e| anyhow!("drifted_read_texts: {e}"))?
        .into_iter()
        .filter(|d| tally.drifted.insert(d.text_id))
        .collect();
    rewrite_drifts(repo, drifts, ctx, tally).await
}

/// Writes the read texts `drifts` with their fields re-extracted and their passages
/// dropped, a transaction per [`REWRITE_BATCH`]; `None` context makes a dry run.
async fn rewrite_drifts(
    repo: &DecisionRepository<'_>,
    mut drifts: Vec<DecisionTextDrift>,
    ctx: Option<&'static ExtractCtx>,
    tally: &mut Tally,
) -> Result<()> {
    let Some(ctx) = ctx else {
        return Ok(());
    };
    while !drifts.is_empty() {
        let rest = drifts.split_off(drifts.len().min(REWRITE_BATCH));
        rewrite_batch(repo, drifts, ctx, tally).await?;
        drifts = rest;
    }
    Ok(())
}

async fn rewrite_batch(
    repo: &DecisionRepository<'_>,
    drifts: Vec<DecisionTextDrift>,
    ctx: &'static ExtractCtx,
    tally: &mut Tally,
) -> Result<()> {
    let ids: Vec<i64> = drifts.iter().map(|d| d.decision_id).collect();
    let inputs: ReextractInputs = repo
        .fetch_reextract_inputs_batch(&ids)
        .await
        .map_err(|e| anyhow!("fetch_reextract_inputs_batch: {e}"))?
        .into_iter()
        .map(|(id, _, provenances, case_name)| (id, (provenances, case_name)))
        .collect();
    let (drifts, extracted) = tokio::task::spawn_blocking(move || {
        let extracted: Vec<Result<ExtractedFields>> = drifts
            .par_iter()
            .map(|d| extract(d, &inputs, ctx))
            .collect();
        (drifts, extracted)
    })
    .await
    .map_err(|e| anyhow!("decision text join: {e}"))?;
    let mut kept = Vec::with_capacity(drifts.len());
    let mut items = Vec::with_capacity(drifts.len());
    for (d, fields) in drifts.into_iter().zip(extracted) {
        match fields {
            Ok(fields) => {
                items.push((d.decision_id, fields));
                kept.push(d);
            }
            Err(e) => {
                tracing::error!(error = %e, "backfill-decision-text: extraction failed");
                tally.unextracted += 1;
            }
        }
    }
    repo.conn.batch_execute("BEGIN").await?;
    let done = async {
        let written = repo
            .rewrite_read_texts(&kept)
            .await
            .map_err(|e| anyhow!("rewrite_read_texts: {e}"))?;
        let set: HashSet<i64> = written.iter().copied().collect();
        items.retain(|(id, _)| set.contains(id));
        repo.update_extracted_fields_bulk(&items, None)
            .await
            .map_err(|e| anyhow::Error::new(e).context("update_extracted_fields_bulk"))?;
        super::attract::attract_pending_for(repo, &written).await?;
        Ok::<_, anyhow::Error>(written)
    }
    .await;
    match done {
        Ok(written) => {
            repo.conn.batch_execute("COMMIT").await?;
            tally.written.extend(written);
            Ok(())
        }
        Err(e) => {
            let _ = repo.conn.batch_execute("ROLLBACK").await;
            Err(e)
        }
    }
}

fn extract(
    d: &DecisionTextDrift,
    inputs: &ReextractInputs,
    ctx: &ExtractCtx,
) -> Result<ExtractedFields> {
    let (provenances, case_name) = inputs
        .get(&d.decision_id)
        .ok_or_else(|| anyhow!("decision {}: no provenance to extract", d.decision_id))?;
    super::reextract::reextract_one(
        d.decision_id,
        &d.full_text,
        provenances,
        case_name.as_deref(),
        ctx,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_split_footnote_call_joins_its_line_at_the_same_length() {
        let text = "présentées le 26 mars 2009 (\n1\n)\nAffaire C-32/08\n(\n*1\n) Langue de procédure\n(\n\n2\n)";
        let (joined, calls) = join_footnote_calls(text);
        assert_eq!(
            joined,
            "présentées le 26 mars 2009 ( 1 )\nAffaire C-32/08\n( *1 ) Langue de procédure\n(\n\n2\n)"
        );
        assert_eq!((calls, joined.len()), (2, text.len()));
    }
}
