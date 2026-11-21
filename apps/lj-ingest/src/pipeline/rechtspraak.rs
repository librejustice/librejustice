//! Rechtspraak: the rulings the Dutch courts publish on `data.rechtspraak.nl`, each document read
//! once and loaded by the standard upsert path, none kept. A daily run reads the documents
//! modified since the last one; the stock walks the decision months, newest first.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use chrono::{Datelike, Months, NaiveDate, Utc};
use tokio::signal::unix::{signal, Signal, SignalKind};

use lj_core::chamber::Seat;
use lj_parse::{
    parse_rechtspraak, rechtspraak_checksummed, rechtspraak_names, rechtspraak_taken, DUTCH_PLACE,
};
use lj_pg::db::Connection;
use lj_sources::error::SourceError;
use lj_sources::rechtspraak::RechtspraakClient;
use lj_store::repository::{DecisionRepository, ExtractedFields, JurisdictionRow};

use crate::config::Settings;

use super::batch::drain_batch;
use super::europe::europe_pool;
use super::{content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode, WriteMode};

const BATCH: usize = 100;
/// The daily run starts no batch past this.
const BUDGET: Duration = Duration::from_secs(600);
/// A daily run with no cursor reads the documents modified in the day before.
const FIRST_WINDOW: chrono::TimeDelta = chrono::TimeDelta::days(1);
/// The stock walks month by month down to this one, then reads every older decision as one slice.
const MONTHLY_FROM: NaiveDate = NaiveDate::from_ymd_opt(2000, 1, 1).expect("a day");
const OLDEST: NaiveDate = NaiveDate::from_ymd_opt(1800, 1, 1).expect("a day");

fn dir(settings: &Settings) -> PathBuf {
    settings.state_dir.join("ingest/rechtspraak")
}

/// The modification time, UTC, of the last document a daily run read.
fn cursor_path(dir: &Path) -> PathBuf {
    dir.join("cursor")
}

/// The documents read and not loaded, each with the modification time it was read at.
fn refused_path(dir: &Path) -> PathBuf {
    dir.join("refused.tsv")
}

fn read_cursor(dir: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(cursor_path(dir)) {
        Ok(cursor) => Ok(Some(cursor.trim().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn first_cursor() -> String {
    (Utc::now() - FIRST_WINDOW)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

fn write_cursor(dir: &Path, cursor: &str) -> Result<()> {
    let part = cursor_path(dir).with_extension("part");
    std::fs::write(&part, cursor)?;
    std::fs::rename(&part, cursor_path(dir))?;
    Ok(())
}

/// Every entry a search lists, page after page.
async fn list(
    client: &RechtspraakClient,
    query: &[(&str, String)],
) -> Result<Vec<(String, String)>> {
    let mut entries = Vec::new();
    loop {
        let page = client.search(query, entries.len()).await?;
        let read = page.entries.len();
        entries.extend(page.entries);
        if read == 0 || entries.len() >= page.total {
            if entries.len() < page.total {
                tracing::warn!(
                    read = entries.len(),
                    total = page.total,
                    ?query,
                    "rechtspraak: a search lists fewer entries than it counts"
                );
            }
            return Ok(entries);
        }
    }
}

/// The rulings with a document, on top of `query`.
fn rulings(query: &[(&'static str, String)]) -> Vec<(&'static str, String)> {
    [
        ("return", "DOC".to_string()),
        ("type", "Uitspraak".to_string()),
    ]
    .into_iter()
    .chain(query.iter().cloned())
    .collect()
}

/// The decision-date slices of the stock, newest first: each month from `start`'s down to
/// [`MONTHLY_FROM`], then everything older.
fn slices(start: NaiveDate) -> Vec<(NaiveDate, NaiveDate)> {
    let mut month = start.with_day(1).expect("a month has a first day");
    let mut slices = Vec::new();
    while month >= MONTHLY_FROM {
        let next = month + Months::new(1);
        slices.push((month, next.pred_opt().expect("a day before a month")));
        month = month - Months::new(1);
    }
    slices.push((OLDEST, MONTHLY_FROM.pred_opt().expect("a day before 2000")));
    slices
}

struct Loader {
    counts: IngestCounts,
    fetched: usize,
    gone: usize,
    refused: usize,
    known_refused: HashMap<String, String>,
    ledger: std::fs::File,
}

impl Loader {
    fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = refused_path(dir);
        let known_refused = match std::fs::read_to_string(&path) {
            Ok(listed) => listed
                .lines()
                .filter_map(|l| l.split_once('\t'))
                .map(|(ecli, updated)| (ecli.to_string(), updated.to_string()))
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            counts: IngestCounts::default(),
            fetched: 0,
            gone: 0,
            refused: 0,
            known_refused,
            ledger: std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?,
        })
    }

    fn candidate(xml: &str) -> Result<Candidate> {
        let parsed = parse_rechtspraak(xml)?;
        let court = parsed.court;
        let seat = Seat {
            jurisdiction_type: court.jurisdiction_type.to_string(),
            jurisdiction_code: Some(court.code.to_string()),
            ..Default::default()
        };
        let extracted = ExtractedFields {
            date_lecture: parsed
                .decision
                .date_lecture
                .as_deref()
                .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()),
            docket_numbers: parsed.decision.numero_dossiers.clone().unwrap_or_default(),
            seat_paths: seat.paths(),
            seat: Some(seat),
            jurisdiction: Some(JurisdictionRow {
                code: court.code.to_string(),
                source_code: court.code.to_string(),
                jurisdiction_type: court.jurisdiction_type.to_string(),
                city: None,
                label: court.label.to_string(),
                place: Some(DUTCH_PLACE.to_string()),
            }),
            ..Default::default()
        };
        let checksummed =
            rechtspraak_checksummed(&parsed.source_fields, &parsed.decision.texte_integral_clean);
        Ok(Candidate {
            decision_id: None,
            public_id: generate_public_id(),
            content_checksum: content_checksum(&checksummed),
            prebuilt_source_fields: Some(parsed.source_fields),
            prebuilt_extracted: Some(extracted),
            decision: parsed.decision,
            raw_payload: Vec::new(),
            payload_format: "xml".to_string(),
            write_mode: WriteMode::Full,
            passages: true,
            distinct: false,
        })
    }

    fn refuse(&mut self, ecli: &str, updated: &str) -> Result<()> {
        writeln!(self.ledger, "{ecli}\t{updated}")?;
        self.known_refused
            .insert(ecli.to_string(), updated.to_string());
        Ok(())
    }

    /// Loads the documents a batch read, `None` for an ECLI the service no longer serves.
    async fn load(
        &mut self,
        conn: &Connection,
        docs: &[(&str, &str, Option<String>)],
    ) -> Result<()> {
        let mut candidates = Vec::new();
        for (ecli, updated, xml) in docs {
            self.fetched += 1;
            let Some(xml) = xml else {
                self.gone += 1;
                self.refuse(ecli, updated)?;
                continue;
            };
            match Self::candidate(xml) {
                Ok(c) => candidates.push(c),
                Err(e) => {
                    tracing::debug!(ecli = %ecli, error = %e, "rechtspraak document refused");
                    self.refused += 1;
                    self.refuse(ecli, updated)?;
                }
            }
        }
        if !candidates.is_empty() {
            drain_batch(conn, candidates, IngestMode::MissingHash, &mut self.counts).await?;
        }
        Ok(())
    }
}

enum Stop {
    Budget,
    Term,
    Failed(SourceError),
}

/// Reads the documents of `entries` and loads them a batch at a time, up to a stop: the count of
/// entries done comes back with it.
async fn read(
    client: &RechtspraakClient,
    conn: &Connection,
    loader: &mut Loader,
    entries: &[(String, String)],
    term: &mut Signal,
    deadline: Option<Instant>,
) -> Result<(usize, Option<Stop>)> {
    let mut done = 0;
    for chunk in entries.chunks(BATCH) {
        if deadline.is_some_and(|d| Instant::now() > d) {
            return Ok((done, Some(Stop::Budget)));
        }
        let mut docs = Vec::with_capacity(chunk.len());
        let mut stop = None;
        for (ecli, updated) in chunk {
            let answer = tokio::select! {
                answer = client.document(ecli) => answer,
                _ = term.recv() => {
                    stop = Some(Stop::Term);
                    break;
                }
            };
            match answer {
                Ok(xml) => docs.push((ecli.as_str(), updated.as_str(), xml)),
                Err(e) => {
                    stop = Some(Stop::Failed(e));
                    break;
                }
            }
        }
        loader.load(conn, &docs).await?;
        done += docs.len();
        tracing::info!(
            fetched = loader.fetched,
            created = loader.counts.created,
            updated = loader.counts.updated,
            refused = loader.refused,
            "rechtspraak sync"
        );
        if stop.is_some() {
            return Ok((done, stop));
        }
    }
    Ok((done, None))
}

/// The first day of `month` (`YYYY-MM`), else today.
fn start_month(month: Option<&str>) -> Result<NaiveDate> {
    Ok(match month {
        Some(m) => NaiveDate::parse_from_str(&format!("{m}-01"), "%Y-%m-%d")
            .map_err(|e| anyhow!("--month {m}: {e}"))?,
        None => Utc::now().date_naive(),
    })
}

/// Reads the rulings modified since the last run and loads them. `full` walks the stock from
/// `month` down instead, skipping what the base holds, with no time budget; `limit` stops after
/// that many documents; `dry_run` parses them and prints a line each, writing nothing.
pub async fn sync_rechtspraak(
    full: bool,
    month: Option<String>,
    limit: Option<usize>,
    dry_run: bool,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = dir(&settings);
    let client = RechtspraakClient::new(&settings.egress_proxy)?;
    let start = start_month(month.as_deref())?;
    if dry_run {
        return dry(&client, &dir, full, start, limit.unwrap_or(20)).await;
    }
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    if !DecisionRepository::new(&conn).rechtspraak_lock().await? {
        tracing::info!("rechtspraak sync: another run holds the lock");
        return Ok(());
    }
    let mut loader = Loader::open(&dir)?;
    let mut term = signal(SignalKind::terminate())?;
    let stop = if full {
        stock(&client, &conn, &mut loader, &mut term, start, limit).await?
    } else {
        daily(&client, &conn, &mut loader, &mut term, &dir, limit).await?
    };
    let named = DecisionRepository::new(&conn)
        .write_jurisdiction_names(&rechtspraak_names())
        .await?;
    tracing::warn!(
        full,
        fetched = loader.fetched,
        gone = loader.gone,
        refused = loader.refused,
        created = loader.counts.created,
        updated = loader.counts.updated,
        skipped = loader.counts.skipped,
        errors = loader.counts.errors,
        named,
        "rechtspraak sync done"
    );
    match stop {
        None | Some(Stop::Budget) => Ok(()),
        Some(Stop::Term) => bail!("rechtspraak sync stopped by SIGTERM, the next run resumes"),
        Some(Stop::Failed(e)) => bail!("rechtspraak sync stopped, the next run resumes: {e}"),
    }
}

/// The rulings modified since the cursor, oldest change first, the cursor moved past what was
/// loaded.
async fn daily(
    client: &RechtspraakClient,
    conn: &Connection,
    loader: &mut Loader,
    term: &mut Signal,
    dir: &Path,
    limit: Option<usize>,
) -> Result<Option<Stop>> {
    let started = Instant::now();
    let cursor = match read_cursor(dir)? {
        Some(cursor) => cursor,
        None => {
            tracing::info!(
                "rechtspraak sync: no cursor yet, the documents of the day before are read"
            );
            first_cursor()
        }
    };
    // `modified` is read as Amsterdam time, ahead of UTC: the UTC cursor asks an hour or two more.
    let mut listed = list(client, &rulings(&[("modified", cursor[..19].to_string())])).await?;
    listed.retain(|(_, updated)| *updated >= cursor);
    listed.sort_by(|a, b| a.1.cmp(&b.1));
    let newest = listed.last().map(|(_, updated)| updated.clone());
    let (wanted, others): (Vec<_>, Vec<_>) = listed
        .into_iter()
        .partition(|(ecli, _)| rechtspraak_taken(ecli));
    let wanted = &wanted[..limit.unwrap_or(wanted.len()).min(wanted.len())];
    tracing::info!(%cursor, wanted = wanted.len(), others = others.len(), "rechtspraak sync: documents modified since the cursor");
    let (done, stop) = read(client, conn, loader, wanted, term, Some(started + BUDGET)).await?;
    if limit.is_some() {
        return Ok(stop);
    }
    let reached = match (&stop, newest) {
        (None, Some(newest)) => Some(newest),
        _ => done.checked_sub(1).map(|i| wanted[i].1.clone()),
    };
    if let Some(reached) = reached {
        write_cursor(dir, &reached)?;
    }
    if stop.is_some() {
        tracing::warn!(
            left = wanted.len() - done,
            "rechtspraak sync: the documents left wait for the next run"
        );
    }
    Ok(stop)
}

/// The rulings of each decision month from `start` down, those the base holds or a run refused at
/// the same modification left out.
async fn stock(
    client: &RechtspraakClient,
    conn: &Connection,
    loader: &mut Loader,
    term: &mut Signal,
    start: NaiveDate,
    limit: Option<usize>,
) -> Result<Option<Stop>> {
    let mut left = limit.unwrap_or(usize::MAX);
    for (first, last) in slices(start) {
        if left == 0 {
            break;
        }
        let listed = list(
            client,
            &rulings(&[("date", first.to_string()), ("date", last.to_string())]),
        )
        .await?;
        let (taken, others): (Vec<_>, Vec<_>) = listed
            .into_iter()
            .partition(|(ecli, _)| rechtspraak_taken(ecli));
        let uids: Vec<String> = taken
            .iter()
            .map(|(ecli, _)| format!("rechtspraak/{ecli}"))
            .collect();
        let held = DecisionRepository::new(conn)
            .provenance_decisions(&uids)
            .await?;
        let mut wanted: Vec<(String, String)> = taken
            .into_iter()
            .filter(|(ecli, updated)| {
                !held.contains_key(&format!("rechtspraak/{ecli}"))
                    && loader.known_refused.get(ecli) != Some(updated)
            })
            .collect();
        wanted.truncate(left);
        left -= wanted.len();
        tracing::info!(%first, %last, held = held.len(), others = others.len(), wanted = wanted.len(), "rechtspraak stock: a month listed");
        let (_, stop) = read(client, conn, loader, &wanted, term, None).await?;
        if stop.is_some() {
            tracing::warn!(%first, "rechtspraak stock stopped, `--month` resumes at this month");
            return Ok(stop);
        }
    }
    Ok(None)
}

/// Lists the documents of the stock's first month, or those modified since the cursor, reads
/// `limit` of them and parses them, a line on stdout for each: no database, nothing written.
async fn dry(
    client: &RechtspraakClient,
    dir: &Path,
    full: bool,
    start: NaiveDate,
    limit: usize,
) -> Result<()> {
    let query = if full {
        let (first, last) = slices(start)[0];
        rulings(&[("date", first.to_string()), ("date", last.to_string())])
    } else {
        let cursor = read_cursor(dir)?.unwrap_or_else(first_cursor);
        rulings(&[("modified", cursor[..19].to_string())])
    };
    let listed = list(client, &query).await?;
    tracing::info!(
        full,
        listed = listed.len(),
        "rechtspraak dry run: documents listed"
    );
    for (ecli, _) in listed
        .iter()
        .filter(|(ecli, _)| rechtspraak_taken(ecli))
        .take(limit)
    {
        let Some(xml) = client.document(ecli).await? else {
            println!("{ecli}\tgone");
            continue;
        };
        match parse_rechtspraak(&xml) {
            Ok(p) => println!(
                "{ecli}\t{}\t{}\t{}\t{} chars\t{}\tabstract {} chars",
                p.court.code,
                p.decision.date_lecture.as_deref().unwrap_or_default(),
                p.decision
                    .numero_dossiers
                    .clone()
                    .unwrap_or_default()
                    .join(" | "),
                p.decision.texte_integral_clean.chars().count(),
                p.source_fields["lang"].as_str().unwrap_or_default(),
                p.source_fields["abstract"]
                    .as_str()
                    .map_or(0, |a| a.chars().count()),
            ),
            Err(e) => println!("{ecli}\trefused\t{e}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stock_walks_months_down_then_reads_the_rest_at_once() {
        let slices = slices(NaiveDate::from_ymd_opt(2026, 11, 19).unwrap());
        let day = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        assert_eq!(slices[0], (day(2026, 11, 1), day(2026, 11, 30)));
        assert_eq!(slices[1], (day(2026, 10, 1), day(2026, 10, 31)));
        assert_eq!(slices[9], (day(2026, 2, 1), day(2026, 2, 28)));
        assert_eq!(
            slices[slices.len() - 2],
            (day(2000, 1, 1), day(2000, 1, 31))
        );
        assert_eq!(slices[slices.len() - 1], (OLDEST, day(1999, 12, 31)));
        assert_eq!(slices.len(), 26 * 12 + 11 + 1);
    }
}
