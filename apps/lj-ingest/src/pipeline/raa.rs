//! The prefectures' recueils des actes administratifs → `raa_document`, listed
//! through Attrap, each PDF read from its prefecture, page by page.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use bytes::Bytes;
use chrono::{Days, NaiveDate, Utc};
use lj_bucket::{key, Bucket};
use lj_llm::mistral::MistralError;

use lj_sources::raa::{self, Fetched, RaaClient};
use lj_store::repository::{DecisionRepository, RaaListedRow, RaaQueued, RaaRead, RaaScanned};

use crate::config::Settings;
use crate::ocr::{self, Ocr};

/// A page with fewer letters has no text layer to read, and goes to the OCR.
const TEXT_LAYER_LETTERS: usize = 50;
const LIST_WINDOW_DAYS: u64 = 7;
const QUEUE_BATCH: i64 = 200;
const MAX_PACE: Duration = Duration::from_secs(120);
const REFUSAL_PAUSE: Duration = Duration::from_secs(45 * 60);
const CALM_BEFORE_PACE_RESET: Duration = Duration::from_secs(60 * 60);
const MAX_REFUSALS: u32 = 4;
const MAX_TRANSPORT_ERRORS: u32 = 10;
const OCR_PAUSE: Duration = Duration::from_secs(10 * 60);
const OCR_IDLE: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
struct FetchCounts {
    fetched: usize,
    read: usize,
    awaiting_ocr: usize,
    gone: usize,
    unreadable: usize,
    refused: usize,
    failed: usize,
}

#[derive(Debug, Default)]
struct OcrCounts {
    read: usize,
    pages: usize,
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
        let fetching = AtomicBool::new(true);
        let (f, o) = tokio::try_join!(
            async {
                let f = fetch(&repo, &client, &bucket, pace, deadline).await;
                fetching.store(false, Ordering::Release);
                f
            },
            async {
                match &ocr {
                    Some(reader) => read_scans(&repo, &bucket, reader, deadline, &fetching).await,
                    None => Ok(OcrCounts::default()),
                }
            },
        )?;
        tracing::info!(
            fetched = f.fetched,
            read = f.read,
            awaiting_ocr = f.awaiting_ocr,
            gone = f.gone,
            unreadable = f.unreadable,
            refused = f.refused,
            failed = f.failed,
            "raa: fetched"
        );
        tracing::info!(
            read = o.read,
            pages = o.pages,
            failed = o.failed,
            "raa: OCR"
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
    start_pace: Duration,
    deadline: Option<Instant>,
) -> Result<FetchCounts> {
    let mut c = FetchCounts::default();
    let mut pace = start_pace;
    let mut calm_since = Instant::now();
    let (mut refusals, mut transport) = (0u32, 0u32);
    let mut last: Option<Instant> = None;
    'run: loop {
        let queue = repo.raa_queue(QUEUE_BATCH).await?;
        if queue.is_empty() {
            break;
        }
        for doc in queue {
            if deadline.is_some_and(|d| Instant::now() >= d) {
                break 'run;
            }
            if pace > start_pace && calm_since.elapsed() >= CALM_BEFORE_PACE_RESET {
                pace = start_pace;
                tracing::info!(
                    pace_s = pace.as_secs(),
                    "raa: an hour without refusal, pace reset"
                );
            }
            if let Some(last) = last {
                tokio::time::sleep_until((last + pace).into()).await;
            }
            last = Some(Instant::now());
            let pause = match client.fetch(&doc.url).await {
                Ok(Fetched::Pdf(bytes)) => {
                    (refusals, transport) = (0, 0);
                    c.fetched += 1;
                    store(repo, bucket, &doc, bytes, &mut c).await?;
                    false
                }
                Ok(Fetched::Gone) => {
                    repo.raa_close(doc.id, "gone").await?;
                    c.gone += 1;
                    false
                }
                Ok(Fetched::Refused(refusal)) => {
                    c.refused += 1;
                    refusals += 1;
                    tracing::warn!(%refusal, refusals, url = %doc.url, "raa: request refused");
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
                calm_since = Instant::now();
            }
        }
    }
    Ok(c)
}

/// Keeps the PDF in the bucket and its text layer in the row.
async fn store(
    repo: &DecisionRepository<'_>,
    bucket: &Bucket,
    doc: &RaaQueued,
    bytes: Vec<u8>,
    c: &mut FetchCounts,
) -> Result<()> {
    let checksum = xxhash_rust::xxh3::xxh3_64(&bytes) as i64;
    let bytes = Bytes::from(bytes);
    let file = key::raa(doc.id, checksum);
    if !bucket.contains(&file).await? {
        bucket.put(&file, bytes.clone()).await?;
    }
    let layer = tokio::task::spawn_blocking(move || lj_sources::pdf::pdftotext_extract(&bytes))
        .await
        .map_err(|e| anyhow!("pdftotext task: {e}"))?;
    let pages = match layer {
        Ok(layer) => layer_pages(&layer),
        Err(e) => {
            tracing::warn!(url = %doc.url, error = %e, "raa: unreadable pdf");
            repo.raa_close(doc.id, "unreadable").await?;
            c.unreadable += 1;
            return Ok(());
        }
    };
    let body = full_body(&pages);
    if body.is_some() {
        c.read += 1;
    } else {
        c.awaiting_ocr += 1;
    }
    repo.raa_write(&RaaRead {
        id: doc.id,
        pdf_checksum: checksum,
        pages: &pages,
        ocr_pages: &[],
        body: body.as_deref(),
    })
    .await?;
    Ok(())
}

/// Runs the OCR of the recueils `fetch` leaves with scanned pages, beside it.
async fn read_scans(
    repo: &DecisionRepository<'_>,
    bucket: &Bucket,
    reader: &Ocr,
    deadline: Option<Instant>,
    fetching: &AtomicBool,
) -> Result<OcrCounts> {
    let mut c = OcrCounts::default();
    loop {
        // Read before the queue: once false, every row `fetch` wrote is in it.
        let more = fetching.load(Ordering::Acquire);
        let queue = repo.raa_ocr_queue(QUEUE_BATCH).await?;
        if queue.is_empty() {
            if !more {
                return Ok(c);
            }
            tokio::time::sleep(OCR_IDLE).await;
            continue;
        }
        for doc in queue {
            if deadline.is_some_and(|d| Instant::now() >= d) {
                return Ok(c);
            }
            if !read_scan(repo, bucket, reader, doc, deadline, &mut c).await? {
                if reader.spent() {
                    tracing::warn!("raa: every OCR key is spent, the scans wait for the next run");
                    return Ok(c);
                }
                tracing::warn!(pause_s = OCR_PAUSE.as_secs(), "raa: OCR paused");
                let wake = Instant::now() + OCR_PAUSE;
                tokio::time::sleep_until(deadline.map_or(wake, |d| d.min(wake)).into()).await;
            }
        }
    }
}

/// Mistral's answer for each page of `numbers`, `None` for a page the OCR invented on twice
/// or one holding an image too large for it, found by reading alone each page of its batch.
async fn read_pages(
    reader: &Ocr,
    pdf: &Bytes,
    numbers: &[u32],
    url: &str,
) -> Result<Vec<Option<serde_json::Value>>> {
    let too_large = |e: &anyhow::Error| {
        matches!(
            e.downcast_ref::<MistralError>(),
            Some(MistralError::ImageTooLarge)
        )
    };
    let subset = pages_of(pdf, numbers, url).await?;
    match reader.layout_pages(&subset, numbers.len(), url).await {
        Ok(answers) => Ok(answers.into_iter().map(Result::ok).collect()),
        Err(e) if too_large(&e) => {
            let mut answers = Vec::with_capacity(numbers.len());
            for &n in numbers {
                let one = pages_of(pdf, &[n], url).await?;
                answers.push(match reader.layout_pages(&one, 1, url).await {
                    Ok(mut answer) => answer.pop().and_then(Result::ok),
                    Err(e) if too_large(&e) => {
                        tracing::warn!(
                            url,
                            page = n,
                            "raa: an image too large for the OCR, page left empty"
                        );
                        None
                    }
                    Err(e) => return Err(e),
                });
            }
            Ok(answers)
        }
        Err(e) => Err(e),
    }
}

async fn pages_of(pdf: &Bytes, numbers: &[u32], url: &str) -> Result<Vec<u8>> {
    let (pdf, numbers) = (pdf.clone(), numbers.to_vec());
    tokio::task::spawn_blocking(move || lj_sources::pdf::pages_extract(&pdf, &numbers))
        .await
        .map_err(|e| anyhow!("pages task: {e}"))?
        .map_err(|e| anyhow!("pages_extract {url}: {e}"))
}

/// False when the OCR failed: the recueil keeps the pages read so far, and goes
/// to the back of the queue. A page [`read_pages`] gives nothing of is left empty.
/// Mistral's answer for each page read is kept beside the PDF.
async fn read_scan(
    repo: &DecisionRepository<'_>,
    bucket: &Bucket,
    reader: &Ocr,
    doc: RaaScanned,
    deadline: Option<Instant>,
    c: &mut OcrCounts,
) -> Result<bool> {
    let RaaScanned {
        id,
        url,
        pdf_checksum,
        mut pages,
        mut ocr_pages,
    } = doc;
    let file = key::raa(id, pdf_checksum);
    let Some(bytes) = bucket.get(&file).await? else {
        bail!("raa {url}: {file} is not in the bucket");
    };
    let missing: Vec<u32> = (0..pages.len() as u32)
        .filter(|&i| pages[i as usize].is_none())
        .collect();
    let (mut read, mut ok) = (0, true);
    for batch in missing.chunks(ocr::BATCH) {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
        let numbers: Vec<u32> = batch.iter().map(|i| i + 1).collect();
        match read_pages(reader, &bytes, &numbers, &url).await {
            Ok(answers) => {
                futures::future::try_join_all(batch.iter().zip(&answers).filter_map(
                    |(&i, page)| {
                        let page = page.as_ref()?;
                        let at = key::ocr_page(&file, i + 1);
                        Some(async move { ocr::keep(bucket, &at, page).await })
                    },
                ))
                .await?;
                for (&i, page) in batch.iter().zip(&answers) {
                    pages[i as usize] = Some(page.as_ref().map(ocr::markdown).unwrap_or_default());
                    if page.is_some() {
                        ocr_pages.push(i as i32);
                    }
                }
                read += batch.len();
            }
            Err(e) => {
                tracing::warn!(%url, error = %e, "raa: OCR failed");
                ok = false;
                break;
            }
        }
    }
    c.pages += read;
    if read > 0 {
        ocr_pages.sort_unstable();
        let body = full_body(&pages);
        if body.is_some() {
            c.read += 1;
        }
        repo.raa_write(&RaaRead {
            id,
            pdf_checksum,
            pages: &pages,
            ocr_pages: &ocr_pages,
            body: body.as_deref(),
        })
        .await?;
    }
    reader.park_spent(repo).await?;
    if !ok && !reader.spent() {
        c.failed += 1;
        repo.raa_failed(id).await?;
    }
    Ok(ok)
}

/// Rewrites every page the OCR read from the answer kept beside the PDF, and the body.
pub async fn rebuild_raa_ocr() -> Result<()> {
    let settings = Settings::from_env()?;
    let bucket = settings.bucket()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    if !repo.raa_lock().await? {
        bail!("raa: another run holds the lock");
    }
    let (mut after, mut recueils, mut pages_rebuilt, mut unkept) =
        (i64::MIN, 0usize, 0usize, 0usize);
    loop {
        let rows = repo.raa_ocr_read(after, QUEUE_BATCH).await?;
        let Some(last) = rows.last() else {
            break;
        };
        after = last.id;
        for mut doc in rows {
            let file = key::raa(doc.id, doc.pdf_checksum);
            let mut rebuilt = 0usize;
            for &i in &doc.ocr_pages {
                match ocr::kept::<serde_json::Value>(&bucket, &key::ocr_page(&file, i as u32 + 1))
                    .await?
                {
                    Some(page) => {
                        doc.pages[i as usize] = Some(ocr::markdown(&page));
                        rebuilt += 1;
                    }
                    None => unkept += 1,
                }
            }
            if rebuilt == 0 {
                continue;
            }
            let body = full_body(&doc.pages);
            repo.raa_write(&RaaRead {
                id: doc.id,
                pdf_checksum: doc.pdf_checksum,
                pages: &doc.pages,
                ocr_pages: &doc.ocr_pages,
                body: body.as_deref(),
            })
            .await?;
            recueils += 1;
            pages_rebuilt += rebuilt;
        }
    }
    tracing::info!(
        recueils,
        pages = pages_rebuilt,
        unkept,
        "raa: OCR pages rebuilt from their answers"
    );
    Ok(())
}

/// Empties the OCR pages of the recueils their PDF leaves blank, redrawing only the
/// [`ocr::suspect`] ones, in the kept answers and the recueils' pages and body.
pub async fn blank_raa_ocr(dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let bucket = settings.bucket()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    if !dry_run && !repo.raa_lock().await? {
        bail!("raa: another run holds the lock");
    }
    let mut tally = ocr::Emptied::default();
    let mut after = i64::MIN;
    loop {
        let rows = repo.raa_ocr_read(after, QUEUE_BATCH).await?;
        let Some(last) = rows.last() else {
            break;
        };
        after = last.id;
        for mut doc in rows {
            tally.docs_read += 1;
            let suspects: Vec<u32> = doc
                .ocr_pages
                .iter()
                .filter(|&&i| doc.pages[i as usize].as_deref().is_some_and(ocr::suspect))
                .map(|&i| i as u32 + 1)
                .collect();
            if suspects.is_empty() {
                continue;
            }
            let file = key::raa(doc.id, doc.pdf_checksum);
            let Some(blank) = ocr::redraw(&bucket, &file, &suspects, &mut tally).await? else {
                continue;
            };
            if blank.is_empty() {
                continue;
            }
            for &n in &blank {
                let text = doc.pages[n as usize - 1].replace(String::new());
                tally.empty(text.as_deref().unwrap_or_default());
            }
            tally.docs += 1;
            if dry_run {
                continue;
            }
            for &n in &blank {
                let at = key::ocr_page(&file, n);
                if let Some(page) = ocr::kept::<serde_json::Value>(&bucket, &at).await? {
                    ocr::keep(&bucket, &at, &ocr::emptied(&page)).await?;
                }
            }
            let body = full_body(&doc.pages);
            repo.raa_write(&RaaRead {
                id: doc.id,
                pdf_checksum: doc.pdf_checksum,
                pages: &doc.pages,
                ocr_pages: &doc.ocr_pages,
                body: body.as_deref(),
            })
            .await?;
        }
    }
    tally.log("raa", dry_run);
    Ok(())
}

/// The body once every page is read.
fn full_body(pages: &[Option<String>]) -> Option<String> {
    pages.iter().all(Option::is_some).then(|| {
        let joined: Vec<&str> = pages.iter().flatten().map(String::as_str).collect();
        lj_parse::reflow_page_text(&joined.join("\u{c}"))
    })
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
