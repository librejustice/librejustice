//! The Conseil du contentieux des étrangers: its arrêts listed by language, then month by month,
//! each month kept under `<state_dir>/ingest/cce/months/`, each PDF kept in the bucket, then loaded.

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use lj_bucket::{key, Bucket};
use lj_core::chamber::Seat;
use lj_parse::{
    cce_names, cce_source_uid, parse_cce, CceArret, CceLanguage, CceParsed, CceRefused, CCE_CODE,
    CCE_LABEL, CCE_LANGUAGES, CCE_PLACE, CCE_TYPE,
};
use lj_pg::db::Connection;
use lj_sources::cce::{CceClient, Row};
use lj_store::repository::{DecisionRepository, ExtractedFields, JurisdictionRow};

use crate::config::Settings;

use super::batch::drain_batch;
use super::europe::europe_pool;
use super::{content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode, WriteMode};

/// The daily run sends no request past this, about 120 at the site's pace.
const BUDGET: Duration = Duration::from_secs(1_200);
const BATCH: usize = 50;

/// A month's arrêts as its listing gave them, and the count the site stated then.
#[derive(Serialize, Deserialize)]
struct Month {
    count: usize,
    rows: Vec<Row>,
}

fn months_dir(dir: &Path, language: CceLanguage) -> PathBuf {
    dir.join("months").join(language.code())
}

/// Every month kept in `language`, by `YYYY-MM`.
fn kept_months(dir: &Path, language: CceLanguage) -> Result<BTreeMap<String, Month>> {
    let mut months = BTreeMap::new();
    let entries = match std::fs::read_dir(months_dir(dir, language)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(months),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|x| x == "json") {
            let month = path
                .file_stem()
                .and_then(|s| s.to_str())
                .expect("a month file named by its month")
                .to_string();
            months.insert(month, serde_json::from_slice(&std::fs::read(&path)?)?);
        }
    }
    Ok(months)
}

fn write_month(dir: &Path, language: CceLanguage, month: &str, kept: &Month) -> Result<()> {
    let folder = months_dir(dir, language);
    std::fs::create_dir_all(&folder)?;
    let path = folder.join(format!("{month}.json"));
    let part = path.with_extension("part");
    std::fs::write(&part, serde_json::to_vec(kept)?)?;
    std::fs::rename(&part, &path)?;
    Ok(())
}

/// The arrêts a run left out for good, `number\treason`: a PDF the site no longer has, a scan.
fn refused_path(dir: &Path) -> PathBuf {
    dir.join("refused.tsv")
}

fn read_refused(dir: &Path) -> Result<HashSet<u32>> {
    match std::fs::read_to_string(refused_path(dir)) {
        Ok(lines) => Ok(lines
            .lines()
            .filter_map(|l| l.split('\t').next()?.parse().ok())
            .collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashSet::new()),
        Err(e) => Err(e.into()),
    }
}

fn arret(row: &Row, language: CceLanguage) -> CceArret<'_> {
    CceArret {
        number: row.number,
        language,
        date: &row.date,
        url: &row.url,
        procedure: row.procedure.as_deref(),
        country: row.country.as_deref(),
        composition: row.composition.as_deref(),
        judge: row.judge.as_deref(),
    }
}

fn candidate(parsed: CceParsed, pdf: &[u8]) -> Candidate {
    let seat = Seat {
        jurisdiction_type: CCE_TYPE.to_string(),
        jurisdiction_code: Some(CCE_CODE.to_string()),
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
            code: CCE_CODE.to_string(),
            source_code: CCE_CODE.to_string(),
            jurisdiction_type: CCE_TYPE.to_string(),
            city: None,
            label: CCE_LABEL.to_string(),
            place: Some(CCE_PLACE.to_string()),
        }),
        ..Default::default()
    };
    Candidate {
        decision_id: None,
        public_id: generate_public_id(),
        content_checksum: content_checksum(pdf),
        prebuilt_source_fields: Some(parsed.source_fields),
        prebuilt_extracted: Some(extracted),
        decision: parsed.decision,
        raw_payload: Vec::new(),
        payload_format: "pdf".to_string(),
        write_mode: WriteMode::Full,
        passages: true,
        distinct: false,
    }
}

async fn pdf_text(pdf: Vec<u8>) -> Result<String> {
    Ok(tokio::task::spawn_blocking(move || lj_sources::pdf::pdftotext_extract(&pdf)).await??)
}

#[derive(Debug, Clone, Copy)]
enum Stop {
    Budget,
    Limit,
}

struct Run<'a> {
    client: CceClient,
    conn: &'a Connection,
    bucket: Bucket,
    dir: PathBuf,
    refused: HashSet<u32>,
    budget: Option<Duration>,
    limit: Option<usize>,
    started: Instant,
    stop: Option<Stop>,
    listed: usize,
    fetched: usize,
    from_bucket: usize,
    gone: usize,
    scans: usize,
    batch: Vec<Candidate>,
    counts: IngestCounts,
}

impl Run<'_> {
    /// Whether a request may leave, the reason it may not recorded.
    fn may_send(&mut self) -> bool {
        if self.budget.is_some_and(|b| self.started.elapsed() >= b) {
            self.stop = Some(Stop::Budget);
        }
        self.stop.is_none()
    }

    fn refuse(&mut self, number: u32, reason: &str) -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(refused_path(&self.dir))?;
        writeln!(file, "{number}\t{reason}")?;
        self.refused.insert(number);
        Ok(())
    }

    /// French, then Dutch, then German, each walked whole before the next.
    async fn walk(&mut self) -> Result<()> {
        for language in CCE_LANGUAGES {
            self.walk_language(language).await?;
            if self.stop.is_some() {
                break;
            }
        }
        Ok(())
    }

    /// Every year newest first, its month counts asked again when its own count moved, then each
    /// month newest first, listed again when its count moved, and its arrêts the base lacks read.
    async fn walk_language(&mut self, language: CceLanguage) -> Result<()> {
        if !self.may_send() {
            return Ok(());
        }
        let mut years = self.client.years(language).await?;
        years.sort_unstable_by(|a, b| b.0.cmp(&a.0));
        let mut kept = kept_months(&self.dir, language)?;
        for (year, count) in years {
            let of_year = |m: &String| m.starts_with(&format!("{year}-"));
            let held: usize = kept
                .iter()
                .filter(|(m, _)| of_year(m))
                .map(|(_, k)| k.count)
                .sum();
            let mut months: Vec<(String, usize)> = if held == count {
                kept.iter()
                    .filter(|(m, _)| of_year(m))
                    .map(|(m, k)| (m.clone(), k.count))
                    .collect()
            } else {
                if !self.may_send() {
                    return Ok(());
                }
                self.client.months(language, &year).await?
            };
            months.sort_unstable_by(|a, b| b.0.cmp(&a.0));
            for (month, count) in months {
                let rows = match kept.remove(&month) {
                    Some(k) if k.count == count => k.rows,
                    _ => match self.list(language, &month).await? {
                        Some(rows) => rows,
                        None => return Ok(()),
                    },
                };
                self.month(language, &rows).await?;
                if self.stop.is_some() {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// Lists every page of `month` and keeps it, `None` when the budget ends the listing first.
    async fn list(&mut self, language: CceLanguage, month: &str) -> Result<Option<Vec<Row>>> {
        let mut seen = HashSet::new();
        let mut rows = Vec::new();
        let mut count = None;
        let mut page = 0;
        loop {
            if !self.may_send() {
                return Ok(None);
            }
            let listing = self.client.listing(language, month, page).await?;
            let pages = listing.pages();
            count.get_or_insert(listing.total);
            rows.extend(listing.rows.into_iter().filter(|r| seen.insert(r.number)));
            page += 1;
            if page >= pages {
                break;
            }
        }
        let count = count.expect("a listing read at least once");
        if rows.len() < count {
            tracing::warn!(
                lang = language.code(),
                month,
                count,
                listed = rows.len(),
                "cce: a month lists fewer arrêts than it counts"
            );
        }
        self.listed += 1;
        let kept = Month { count, rows };
        write_month(&self.dir, language, month, &kept)?;
        tracing::info!(lang = language.code(), month, count, "cce: month listed");
        Ok(Some(kept.rows))
    }

    async fn month(&mut self, language: CceLanguage, rows: &[Row]) -> Result<()> {
        let uids: Vec<String> = rows.iter().map(|r| cce_source_uid(r.number)).collect();
        let known = DecisionRepository::new(self.conn)
            .known_source_uids(&uids)
            .await?;
        for (row, uid) in rows.iter().zip(&uids) {
            if known.contains(uid) || self.refused.contains(&row.number) {
                continue;
            }
            self.arret(language, row).await?;
            if self.stop.is_some() {
                break;
            }
        }
        Ok(())
    }

    /// Reads an arrêt's PDF from the bucket, else from the site, keeping it in the bucket.
    async fn arret(&mut self, language: CceLanguage, row: &Row) -> Result<()> {
        let key = key::cce(row.number);
        let pdf = match self.bucket.get(&key).await? {
            Some(bytes) => {
                self.from_bucket += 1;
                bytes.to_vec()
            }
            None => {
                if self.limit.is_some_and(|l| self.fetched >= l) {
                    self.stop = Some(Stop::Limit);
                }
                if !self.may_send() {
                    return Ok(());
                }
                let Some(pdf) = self.client.pdf(&row.url).await? else {
                    tracing::warn!(number = row.number, url = %row.url, "cce: a listed PDF answers 404");
                    self.gone += 1;
                    return self.refuse(row.number, "gone");
                };
                self.bucket.put(&key, pdf.clone()).await?;
                self.fetched += 1;
                pdf
            }
        };
        let text = pdf_text(pdf.clone()).await?;
        let parsed = match parse_cce(&arret(row, language), &text) {
            Ok(parsed) => parsed,
            Err(CceRefused::NoText) => {
                self.scans += 1;
                return self.refuse(row.number, "scan");
            }
        };
        self.batch.push(candidate(parsed, &pdf));
        if self.batch.len() >= BATCH {
            self.flush().await?;
        }
        Ok(())
    }

    async fn flush(&mut self) -> Result<()> {
        let batch = std::mem::take(&mut self.batch);
        if !batch.is_empty() {
            drain_batch(self.conn, batch, IngestMode::MissingHash, &mut self.counts).await?;
            tracing::info!(
                fetched = self.fetched,
                from_bucket = self.from_bucket,
                created = self.counts.created,
                "cce sync"
            );
        }
        Ok(())
    }
}

/// Reads the months whose count moved, then every arrêt the base lacks, newest first, no request
/// past [`BUDGET`] unless `full`. `dry_run` reads each language's newest `limit` arrêts alone.
/// `only` writes these arrêts over from the bucket, and asks the site nothing.
pub async fn sync_cce(full: bool, limit: Option<usize>, dry_run: bool, only: &[u32]) -> Result<()> {
    let settings = Settings::from_env()?;
    if !only.is_empty() {
        return reread(&settings, only).await;
    }
    let client = CceClient::new(&settings.egress_proxy)?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    if !DecisionRepository::new(&conn).cce_lock().await? {
        tracing::info!("cce sync: another run holds the lock, which keeps one client on the site");
        return Ok(());
    }
    if dry_run {
        return dry(&client, limit.unwrap_or(3)).await;
    }
    let named = DecisionRepository::new(&conn)
        .write_jurisdiction_names(&cce_names())
        .await?;
    tracing::info!(named, "cce sync: court names written");
    let dir = settings.state_dir.join("ingest/cce");
    std::fs::create_dir_all(&dir)?;
    let mut run = Run {
        client,
        conn: &conn,
        bucket: settings.bucket()?,
        refused: read_refused(&dir)?,
        dir,
        budget: (!full).then_some(BUDGET),
        limit,
        started: Instant::now(),
        stop: None,
        listed: 0,
        fetched: 0,
        from_bucket: 0,
        gone: 0,
        scans: 0,
        batch: Vec::new(),
        counts: IngestCounts::default(),
    };
    let walked = run.walk().await;
    run.flush().await?;
    tracing::warn!(
        requests = run.client.sent(),
        months_listed = run.listed,
        fetched = run.fetched,
        from_bucket = run.from_bucket,
        gone = run.gone,
        scans = run.scans,
        created = run.counts.created,
        updated = run.counts.updated,
        skipped = run.counts.skipped,
        errors = run.counts.errors,
        stop = ?run.stop,
        "cce sync done"
    );
    walked.map_err(|e| anyhow!("cce sync stopped, the next run resumes: {e}"))
}

/// Writes `numbers` over, each parsed again from its PDF in the bucket and its row in the kept
/// months, its checksum unchanged.
async fn reread(settings: &Settings, numbers: &[u32]) -> Result<()> {
    let dir = settings.state_dir.join("ingest/cce");
    let bucket = settings.bucket()?;
    let wanted: HashSet<u32> = numbers.iter().copied().collect();
    let mut found = HashSet::new();
    let mut batch = Vec::new();
    for language in CCE_LANGUAGES {
        for kept in kept_months(&dir, language)?.into_values() {
            for row in kept.rows.iter().filter(|r| wanted.contains(&r.number)) {
                let pdf = bucket
                    .get(&key::cce(row.number))
                    .await?
                    .ok_or_else(|| anyhow!("cce {}: no PDF in the bucket", row.number))?
                    .to_vec();
                let text = pdf_text(pdf.clone()).await?;
                let parsed = parse_cce(&arret(row, language), &text)
                    .map_err(|e| anyhow!("cce {}: {e:?}", row.number))?;
                batch.push(candidate(parsed, &pdf));
                found.insert(row.number);
            }
        }
    }
    let missing: Vec<u32> = wanted.difference(&found).copied().collect();
    if !missing.is_empty() {
        bail!("cce reread: no kept month lists {missing:?}");
    }
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let mut counts = IngestCounts::default();
    drain_batch(&conn, batch, IngestMode::All, &mut counts).await?;
    tracing::warn!(
        read = found.len(),
        created = counts.created,
        updated = counts.updated,
        errors = counts.errors,
        "cce reread done"
    );
    Ok(())
}

async fn dry(client: &CceClient, limit: usize) -> Result<()> {
    for language in CCE_LANGUAGES {
        let years = client.years(language).await?;
        let year = years
            .iter()
            .map(|(y, _)| y)
            .max()
            .expect("years() refuses an empty answer");
        let months = client.months(language, year).await?;
        let (month, count) = months
            .iter()
            .max_by(|a, b| a.0.cmp(&b.0))
            .expect("months() refuses an empty answer");
        let listing = client.listing(language, month, 0).await?;
        tracing::info!(
            lang = language.code(),
            month,
            count,
            total = listing.total,
            "cce dry run: newest month"
        );
        for row in listing.rows.iter().take(limit) {
            let Some(pdf) = client.pdf(&row.url).await? else {
                println!("{}\tgone", row.number);
                continue;
            };
            let text = pdf_text(pdf).await?;
            match parse_cce(&arret(row, language), &text) {
                Ok(p) => println!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{} chars",
                    row.number,
                    language.code(),
                    row.date,
                    p.decision.chamber.as_deref().unwrap_or_default(),
                    row.procedure.as_deref().unwrap_or_default(),
                    row.country.as_deref().unwrap_or_default(),
                    p.decision.texte_integral_clean.chars().count(),
                ),
                Err(e) => println!("{}\trefused\t{e:?}", row.number),
            }
        }
    }
    Ok(())
}
