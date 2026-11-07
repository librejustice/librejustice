//! The prefectures' recueils des actes administratifs → `raa_document`, listed
//! through Attrap, each PDF read from its prefecture, page by page.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use bytes::Bytes;
use chrono::{Days, NaiveDate, Utc};
use lj_bucket::{key, Bucket};

use lj_sources::raa::{self, Fetched, RaaClient};
use lj_store::repository::{DecisionRepository, RaaListedRow, RaaQueued, RaaRead};

use crate::config::Settings;
use crate::ocr::{self, Ocr};

/// A page with fewer letters has no text layer to read, and goes to the OCR.
const TEXT_LAYER_LETTERS: usize = 50;
const LIST_WINDOW_DAYS: u64 = 7;
const QUEUE_BATCH: i64 = 200;
const MAX_PACE: Duration = Duration::from_secs(120);
const REFUSAL_PAUSE: Duration = Duration::from_secs(45 * 60);
const MAX_REFUSALS: u32 = 4;
const MAX_TRANSPORT_ERRORS: u32 = 10;

#[derive(Debug, Default)]
struct Counts {
    fetched: usize,
    read: usize,
    awaiting_ocr: usize,
    ocr_pages: usize,
    gone: usize,
    unreadable: usize,
    refused: usize,
    failed: usize,
}

pub async fn sync_raa(
    since: Option<NaiveDate>,
    pace: Duration,
    minutes: Option<u64>,
    list_only: bool,
) -> Result<()> {
    let settings = Settings::from_env()?;
    if settings.egress_proxy.is_empty() {
        bail!("LIBREJUSTICE_EGRESS_PROXY is empty: the recueils are read through the egress proxy");
    }
    let client = RaaClient::new(
        &settings.egress_proxy,
        (!settings.page_gateway.is_empty()).then(|| settings.page_gateway.clone()),
    )?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    if !repo.raa_lock().await? {
        tracing::info!("raa: another run holds the lock");
        return Ok(());
    }

    let today = Utc::now().date_naive();
    let since = since.unwrap_or(today - Days::new(14));
    list(&repo, &client, since, today).await?;
    if !list_only {
        let bucket = settings.bucket()?;
        let ocr = Ocr::from_pool(&repo).await?;
        if ocr.is_none() {
            tracing::warn!("raa: the mistral-vibe pool is empty, scanned pages wait");
        }
        let deadline = minutes.map(|m| Instant::now() + Duration::from_secs(m * 60));
        let c = fetch(&repo, &client, &bucket, ocr.as_ref(), pace, deadline).await?;
        tracing::info!(
            fetched = c.fetched,
            read = c.read,
            awaiting_ocr = c.awaiting_ocr,
            ocr_pages = c.ocr_pages,
            gone = c.gone,
            unreadable = c.unreadable,
            refused = c.refused,
            failed = c.failed,
            "raa: fetched"
        );
    }
    for (status, n) in repo.raa_counts().await? {
        tracing::info!(status, n, "raa: recueils");
    }
    Ok(())
}

async fn list(
    repo: &DecisionRepository<'_>,
    client: &RaaClient,
    since: NaiveDate,
    to: NaiveDate,
) -> Result<()> {
    let (mut listed, mut new) = (0usize, 0u64);
    let mut unplaced = BTreeSet::new();
    let mut from = since;
    while from <= to {
        let until = (from + Days::new(LIST_WINDOW_DAYS - 1)).min(to);
        let items = client.list(from, until).await?;
        let rows: Vec<RaaListedRow> = items
            .iter()
            .map(|l| {
                let dept = raa::seat_department(&l.administration);
                if dept.is_none() {
                    unplaced.insert(l.administration.clone());
                }
                RaaListedRow {
                    id: url_id(&l.url),
                    url: &l.url,
                    publisher: &l.administration,
                    dept,
                    published: l.date,
                    title: &l.name,
                }
            })
            .collect();
        new += repo.raa_list(&rows).await?;
        listed += items.len();
        from = until + Days::new(1);
    }
    if !unplaced.is_empty() {
        tracing::warn!(publishers = ?unplaced, "raa: publishers with no département");
    }
    tracing::info!(%since, %to, listed, new, "raa: listed");
    Ok(())
}

/// Reads the queue at one request per `pace` across every prefecture, which
/// share one front.
async fn fetch(
    repo: &DecisionRepository<'_>,
    client: &RaaClient,
    bucket: &Bucket,
    ocr: Option<&Ocr>,
    mut pace: Duration,
    deadline: Option<Instant>,
) -> Result<Counts> {
    let mut c = Counts::default();
    let mut ocr = ocr;
    let (mut refusals, mut transport) = (0u32, 0u32);
    let mut last: Option<Instant> = None;
    'run: loop {
        let queue = repo.raa_queue(ocr.is_some(), QUEUE_BATCH).await?;
        if queue.is_empty() {
            break;
        }
        for doc in queue {
            if deadline.is_some_and(|d| Instant::now() >= d) {
                break 'run;
            }
            if let Some(last) = last {
                tokio::time::sleep_until((last + pace).into()).await;
            }
            last = Some(Instant::now());
            let pause = match client.fetch(&doc.url).await {
                Ok(Fetched::Pdf(bytes)) => {
                    (refusals, transport) = (0, 0);
                    c.fetched += 1;
                    if !read(repo, bucket, &mut ocr, &doc, bytes, deadline, &mut c).await? {
                        tracing::warn!("raa: OCR failed, scanned pages wait for the next run");
                    }
                    false
                }
                Ok(Fetched::Gone) => {
                    repo.raa_close(doc.id, "gone").await?;
                    c.gone += 1;
                    false
                }
                Ok(Fetched::Refused(status)) => {
                    c.refused += 1;
                    refusals += 1;
                    tracing::warn!(status, refusals, url = %doc.url, "raa: request refused");
                    true
                }
                Err(e) => {
                    c.failed += 1;
                    transport += 1;
                    repo.raa_failed(doc.id).await?;
                    tracing::warn!(url = %doc.url, error = %e, "raa: fetch failed");
                    transport >= MAX_TRANSPORT_ERRORS
                }
            };
            if pause {
                if refusals >= MAX_REFUSALS {
                    tracing::warn!(refusals, "raa: still refused, the run stops");
                    break 'run;
                }
                transport = 0;
                pace = (pace * 2).min(MAX_PACE);
                tracing::warn!(
                    pause_s = REFUSAL_PAUSE.as_secs(),
                    pace_s = pace.as_secs(),
                    "raa: pausing"
                );
                tokio::time::sleep(REFUSAL_PAUSE).await;
            }
        }
    }
    Ok(c)
}

/// False when the OCR failed: `ocr` is then unset, and the scanned pages wait
/// for the next run.
async fn read(
    repo: &DecisionRepository<'_>,
    bucket: &Bucket,
    ocr: &mut Option<&Ocr>,
    doc: &RaaQueued,
    bytes: Vec<u8>,
    deadline: Option<Instant>,
    c: &mut Counts,
) -> Result<bool> {
    let checksum = xxhash_rust::xxh3::xxh3_64(&bytes) as i64;
    let bytes = Bytes::from(bytes);
    let file = key::raa(doc.id, checksum);
    if !bucket.contains(&file).await? {
        bucket.put(&file, bytes.clone()).await?;
    }
    let pdf = bytes.clone();
    let layer = tokio::task::spawn_blocking(move || lj_sources::pdf::pdftotext_extract(&pdf))
        .await
        .map_err(|e| anyhow!("pdftotext task: {e}"))?;
    let layer = match layer {
        Ok(layer) => layer,
        Err(e) => {
            tracing::warn!(url = %doc.url, error = %e, "raa: unreadable pdf");
            repo.raa_close(doc.id, "unreadable").await?;
            c.unreadable += 1;
            return Ok(true);
        }
    };
    let mut pages = layer_pages(&layer);
    let mut ocr_pages = Vec::new();
    if doc.pdf_checksum == Some(checksum) {
        for &i in &doc.ocr_pages {
            let kept = doc
                .pages
                .as_ref()
                .and_then(|p| p.get(i as usize).cloned().flatten());
            if let (Some(text), Some(slot)) = (kept, pages.get_mut(i as usize)) {
                *slot = Some(text);
                ocr_pages.push(i);
            }
        }
    }
    let missing: Vec<u32> = (0..pages.len() as u32)
        .filter(|&i| pages[i as usize].is_none())
        .collect();
    let mut ocr_ok = true;
    if let Some(reader) = *ocr {
        for batch in missing.chunks(ocr::BATCH) {
            if deadline.is_some_and(|d| Instant::now() >= d) {
                break;
            }
            let numbers: Vec<u32> = batch.iter().map(|i| i + 1).collect();
            let pdf = bytes.clone();
            let subset =
                tokio::task::spawn_blocking(move || lj_sources::pdf::pages_extract(&pdf, &numbers))
                    .await
                    .map_err(|e| anyhow!("pages task: {e}"))?
                    .map_err(|e| anyhow!("pages_extract {}: {e}", doc.url))?;
            match reader.pages(&subset, batch.len(), &doc.url).await {
                Ok(markdown) if markdown.len() == batch.len() => {
                    for (&i, md) in batch.iter().zip(markdown) {
                        pages[i as usize] = Some(md);
                        ocr_pages.push(i as i32);
                    }
                    c.ocr_pages += batch.len();
                }
                Ok(markdown) => bail!(
                    "raa {}: the OCR gave {} pages for {}",
                    doc.url,
                    markdown.len(),
                    batch.len()
                ),
                Err(e) => {
                    tracing::warn!(url = %doc.url, error = %e, "raa: OCR failed");
                    ocr_ok = false;
                    break;
                }
            }
        }
    }
    if !ocr_ok {
        *ocr = None;
    }
    ocr_pages.sort_unstable();
    let body = pages.iter().all(Option::is_some).then(|| {
        let joined: Vec<&str> = pages.iter().flatten().map(String::as_str).collect();
        lj_parse::reflow_page_text(&joined.join("\u{c}"))
    });
    if body.is_some() {
        c.read += 1;
    } else {
        c.awaiting_ocr += 1;
    }
    repo.raa_write(&RaaRead {
        id: doc.id,
        pdf_checksum: checksum,
        pages: &pages,
        ocr_pages: &ocr_pages,
        body: body.as_deref(),
    })
    .await?;
    Ok(ocr_ok)
}

/// One entry per page of the `pdftotext` output, `None` for a page with no text
/// layer. Every page, the last included, ends with a form feed.
fn layer_pages(layer: &str) -> Vec<Option<String>> {
    let mut parts: Vec<&str> = layer.split('\u{c}').collect();
    if layer.ends_with('\u{c}') {
        parts.pop();
    }
    parts
        .into_iter()
        .map(|p| {
            (p.chars().filter(|c| c.is_alphabetic()).count() >= TEXT_LAYER_LETTERS)
                .then(|| p.to_string())
        })
        .collect()
}

fn url_id(url: &str) -> i64 {
    xxhash_rust::xxh3::xxh3_64(url.as_bytes()) as i64
}
