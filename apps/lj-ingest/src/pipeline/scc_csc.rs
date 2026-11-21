//! The judgments of the Supreme Court of Canada in French, from its site: each page kept under
//! `<state_dir>/ingest/scc-csc/html/`, then loaded by the standard upsert path.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use lj_core::chamber::Seat;
use lj_parse::{
    scc_decision, scc_source_uid, SccDecision, SccRefused, SCC_CODE, SCC_LABEL, SCC_PLACE, SCC_TYPE,
};
use lj_pg::db::Connection;
use lj_sources::scc_csc::{parse_item, Answer, Listing, SccClient};
use lj_store::repository::{DecisionRepository, ExtractedFields, JurisdictionRow};

use crate::config::Settings;

use super::batch::drain_batch;
use super::europe::europe_pool;
use super::{
    content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode, WriteMode,
    BATCH_SIZE,
};

/// The daily run sends no request past this, twenty at the site's pace.
const BUDGET: Duration = Duration::from_secs(600);

/// `--full` has walked every year from the newest down to `walked`.
#[derive(Default, Serialize, Deserialize)]
struct Cursor {
    walked: Option<i32>,
}

impl Cursor {
    fn read(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(raw) => Ok(serde_json::from_slice(&raw)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    fn write(&self, path: &Path) -> Result<()> {
        let part = path.with_extension("part");
        std::fs::write(&part, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&part, path)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
enum Stop {
    Refused,
    Budget,
    Limit,
}

struct Run<'a> {
    client: SccClient,
    conn: &'a Connection,
    pages: PathBuf,
    dry_run: bool,
    budget: Option<Duration>,
    limit: Option<usize>,
    started: Instant,
    stop: Option<Stop>,
    fetched: usize,
    english: usize,
    refused: usize,
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

    async fn listing(&mut self, year: Option<i32>, page: usize) -> Result<Option<Listing>> {
        if !self.may_send() {
            return Ok(None);
        }
        Ok(match self.client.listing(year, page).await? {
            Answer::Page(listing) => Some(listing),
            Answer::Refused => {
                self.stop = Some(Stop::Refused);
                None
            }
        })
    }

    fn page_path(&self, id: u64) -> PathBuf {
        self.pages.join(format!("{id}.html"))
    }

    /// Reads the judgments of a listing page the base lacks, a kept page from its file. Whether
    /// every one of them was in the base or kept before this run.
    async fn page(&mut self, listing: &Listing) -> Result<bool> {
        let uids: Vec<String> = listing.ids.iter().map(|&id| scc_source_uid(id)).collect();
        let known = DecisionRepository::new(self.conn)
            .known_source_uids(&uids)
            .await?;
        let mut visited = true;
        for (&id, uid) in listing.ids.iter().zip(&uids) {
            if known.contains(uid) {
                continue;
            }
            visited &= self.page_path(id).exists();
            self.judgment(id).await?;
            if self.stop.is_some() {
                break;
            }
        }
        Ok(visited)
    }

    async fn judgment(&mut self, id: u64) -> Result<()> {
        let path = self.page_path(id);
        let html = if path.exists() {
            std::fs::read_to_string(&path)?
        } else {
            if self.limit.is_some_and(|l| self.fetched >= l) {
                self.stop = Some(Stop::Limit);
            }
            if !self.may_send() {
                return Ok(());
            }
            let Answer::Page(html) = self.client.item(id).await? else {
                self.stop = Some(Stop::Refused);
                return Ok(());
            };
            let part = path.with_extension("part");
            std::fs::write(&part, &html)?;
            std::fs::rename(&part, &path)?;
            self.fetched += 1;
            html
        };
        let page = parse_item(id, &html);
        match scc_decision(&page) {
            Ok(parsed) => {
                if self.dry_run {
                    tracing::info!(
                        id,
                        title = %page.title,
                        date = ?parsed.decision.date_lecture,
                        dockets = ?parsed.decision.numero_dossiers,
                        neutral = ?parsed.source_fields["neutral_citation"].as_str(),
                        report = ?parsed.source_fields["report_citation"].as_str(),
                        chars = parsed.decision.texte_integral_clean.chars().count(),
                        "scc-csc judgment parsed"
                    );
                    return Ok(());
                }
                self.batch.push(candidate(parsed, &page.title, &html));
                if self.batch.len() >= BATCH_SIZE {
                    self.flush().await?;
                }
            }
            Err(SccRefused::English) => self.english += 1,
            Err(reason) => {
                tracing::warn!(id, ?reason, "scc-csc judgment refused");
                self.refused += 1;
            }
        }
        Ok(())
    }

    /// Reads the years newest first, from below the cursor with `full`. Whether `full` walked
    /// every year.
    async fn walk(&mut self, full: bool, cursor: &mut Cursor, cursor_path: &Path) -> Result<bool> {
        let Some(newest) = self.listing(None, 1).await? else {
            return Ok(false);
        };
        let start = match cursor.walked {
            Some(walked) if full => walked - 1,
            _ => newest.year,
        };
        tracing::info!(full, from = start, "scc-csc sync: start");
        let years: Vec<i32> = newest
            .years
            .iter()
            .copied()
            .filter(|&y| y <= start)
            .collect();
        let mut first = Some(newest);
        for year in years {
            let mut page = 1;
            loop {
                let listing = match first.take().filter(|l| l.year == year) {
                    Some(listing) => listing,
                    None => match self.listing(Some(year), page).await? {
                        Some(listing) => listing,
                        None => return Ok(false),
                    },
                };
                let visited = !listing.ids.is_empty() && self.page(&listing).await?;
                if self.stop.is_some() || (!full && visited) {
                    return Ok(false);
                }
                if page >= listing.pages() {
                    break;
                }
                page += 1;
            }
            if full && !self.dry_run {
                cursor.walked = Some(year);
                cursor.write(cursor_path)?;
            }
        }
        Ok(full)
    }

    async fn flush(&mut self) -> Result<()> {
        let batch = std::mem::take(&mut self.batch);
        if !batch.is_empty() {
            drain_batch(self.conn, batch, IngestMode::MissingHash, &mut self.counts).await?;
        }
        Ok(())
    }
}

fn candidate(parsed: SccDecision, title: &str, html: &str) -> Candidate {
    let seat = Seat {
        jurisdiction_type: SCC_TYPE.to_string(),
        jurisdiction_code: Some(SCC_CODE.to_string()),
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
            code: SCC_CODE.to_string(),
            source_code: SCC_CODE.to_string(),
            jurisdiction_type: SCC_TYPE.to_string(),
            city: None,
            label: SCC_LABEL.to_string(),
            place: Some(SCC_PLACE.to_string()),
        }),
        case_name: Some(title.to_string()).filter(|t| !t.is_empty()),
        ..Default::default()
    };
    Candidate {
        decision_id: None,
        public_id: generate_public_id(),
        content_checksum: content_checksum(html.as_bytes()),
        prebuilt_source_fields: Some(parsed.source_fields),
        prebuilt_extracted: Some(extracted),
        decision: parsed.decision,
        raw_payload: Vec::new(),
        payload_format: "html".to_string(),
        write_mode: WriteMode::Full,
        passages: true,
        distinct: false,
    }
}

/// Reads the newest judgments until a listing page holds none the base lacks, sending no request
/// past [`BUDGET`]. `full` walks every year instead, from the newest down, and resumes below the
/// last year it walked whole. `limit` stops after that many judgments fetched; `dry_run` fetches
/// and parses, keeps the pages and loads nothing.
pub async fn sync_scc_csc(full: bool, limit: Option<usize>, dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.state_dir.join("ingest/scc-csc");
    let pages = dir.join("html");
    std::fs::create_dir_all(&pages)?;
    let cursor_path = dir.join("cursor.json");
    let mut cursor = Cursor::read(&cursor_path)?;

    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    if !DecisionRepository::new(&conn).scc_csc_lock().await? {
        tracing::info!("scc-csc sync: another run holds the lock");
        return Ok(());
    }
    let mut run = Run {
        client: SccClient::new(&settings.egress_proxy)?,
        conn: &conn,
        pages,
        dry_run,
        budget: (!full).then_some(BUDGET),
        limit,
        started: Instant::now(),
        stop: None,
        fetched: 0,
        english: 0,
        refused: 0,
        batch: Vec::new(),
        counts: IngestCounts::default(),
    };

    let walked = run.walk(full, &mut cursor, &cursor_path).await;
    run.flush().await?;
    if walked? && !dry_run {
        std::fs::remove_file(&cursor_path)?;
        tracing::info!("scc-csc sync: every year walked, the next --full starts again");
    }
    tracing::info!(
        requests = run.client.sent(),
        fetched = run.fetched,
        english = run.english,
        refused = run.refused,
        created = run.counts.created,
        updated = run.counts.updated,
        skipped = run.counts.skipped,
        errors = run.counts.errors,
        stop = ?run.stop,
        "scc-csc sync done"
    );
    if let Some(Stop::Refused) = run.stop {
        bail!(
            "scc-csc: the site refused the run after {} requests, the next run resumes",
            run.client.sent()
        );
    }
    Ok(())
}
