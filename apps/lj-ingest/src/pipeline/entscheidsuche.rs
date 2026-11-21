//! entscheidsuche.ch: the decisions of the Swiss courts its index serves, each page loaded
//! by the standard upsert path then kept under `<state_dir>/ingest/entscheidsuche/pages/`.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

use lj_bucket::{key, Bucket};
use lj_core::chamber::Seat;
use lj_llm::mistral::MistralError;
use lj_parse::{court_names, entscheidsuche_scan, parse_entscheidsuche, SWISS_COURTS};
use lj_pg::db::Connection;
use lj_sources::entscheidsuche::{EntscheidsucheClient, Hit, Listed, PAGE, TEXT_CAP};
use lj_store::repository::{DecisionRepository, ExtractedFields, JurisdictionRow};

use crate::config::Settings;
use crate::ocr::{self, Ocr};

use super::batch::run_stream;
use super::europe::europe_pool;
use super::{content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode, WriteMode};

/// The daily run starts no page past this, the rest waiting for the next run.
const BUDGET: Duration = Duration::from_secs(1_200);
/// The weekly catch-up starts no fetch past this, its listing of 750,000 ids taking 1,500 s.
const CATCH_UP_BUDGET: Duration = Duration::from_secs(4_200);

fn dir(settings: &Settings) -> PathBuf {
    settings.state_dir.join("ingest/entscheidsuche")
}

/// The stock's last id until it is read whole, then the latest day scraped; `after` also holds
/// where a daily run stopped, the next one resuming there over the same days.
#[derive(Default, Serialize, Deserialize)]
struct Cursor {
    after: Option<String>,
    stock_done: bool,
    scraped: Option<String>,
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
        write_json(path, self)
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let part = path.with_extension("part");
    std::fs::write(&part, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&part, path)?;
    Ok(())
}

/// The documents a run read without loading their whole text, refused by the parse or cut by the
/// index with no original to read, by id, with their [`visit`]: the catch-up reads one again only
/// once its spider scraped it again or the parse changed.
fn read_unread(path: &Path) -> Result<HashMap<String, String>> {
    match std::fs::read(path) {
        Ok(raw) => Ok(serde_json::from_slice(&raw)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(e.into()),
    }
}

/// The day a document was scraped and the parse's version, `2026-11-01/1`.
fn visit(scrapedate: &str) -> String {
    format!("{scrapedate}/{}", lj_parse::ENTSCHEIDSUCHE_PARSE_VERSION)
}

/// Each chamber's French label, by its id.
fn chamber_labels(facets: &Value) -> HashMap<String, String> {
    let mut labels = HashMap::new();
    for canton in facets.as_object().into_iter().flat_map(|o| o.values()) {
        for court in canton
            .get("gerichte")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|o| o.values())
        {
            for (id, chamber) in court
                .get("kammern")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                if let Some(fr) = chamber.get("fr").and_then(Value::as_str) {
                    labels.insert(id.clone(), fr.trim().to_string());
                }
            }
        }
    }
    labels
}

/// The document without its `source` fields, which name the index run that harvested it and
/// change at every reindexing of an unchanged decision.
fn checksummed(mut doc: Value) -> Vec<u8> {
    if let Some(doc) = doc.as_object_mut() {
        doc.remove("source");
    }
    if let Some(attachment) = doc.get_mut("attachment").and_then(Value::as_object_mut) {
        attachment.remove("source");
    }
    serde_json::to_vec(&doc).expect("a JSON value serialises")
}

fn candidate(
    hit: &Hit,
    chambers: &HashMap<String, String>,
    original: Option<&str>,
) -> Result<Candidate> {
    let raw = hit.source.get();
    let doc: Value = serde_json::from_str(raw)?;
    let chamber_id: Option<String> = doc
        .get("hierarchy")
        .and_then(|h| h.get(2))
        .and_then(Value::as_str)
        .map(str::to_string);
    let chamber = chamber_id
        .and_then(|id| chambers.get(&id))
        .map(String::as_str);
    let parsed = parse_entscheidsuche(raw, chamber, original)?;
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
            place: Some(court.place.to_string()),
        }),
        ..Default::default()
    };
    let mut payload = checksummed(doc);
    // A text read from the original is another payload than the index's cut one, which the
    // checksum alone would skip.
    if original.is_some() {
        payload.extend_from_slice(b"\ntext_from_original");
    }
    Ok(Candidate {
        decision_id: None,
        public_id: generate_public_id(),
        content_checksum: content_checksum(&payload),
        prebuilt_source_fields: Some(parsed.source_fields),
        prebuilt_extracted: Some(extracted),
        decision: parsed.decision,
        raw_payload: Vec::new(),
        payload_format: "json".to_string(),
        write_mode: WriteMode::Full,
        passages: true,
        distinct: false,
    })
}

/// The text of the file the index read a document from, when the index cut its own text: `None`
/// for a whole text.
async fn original_text(client: &EntscheidsucheClient, hit: &Hit) -> Result<Option<String>> {
    let doc: Value = serde_json::from_str(hit.source.get())?;
    let attachment = doc.get("attachment");
    let field = |k: &str| attachment.and_then(|a| a.get(k));
    if field("content_length")
        .and_then(Value::as_u64)
        .is_none_or(|l| l < TEXT_CAP)
    {
        return Ok(None);
    }
    let url = field("content_url")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("entscheidsuche {}: cut text without content_url", hit.id))?;
    let bytes = client.original(url).await?;
    let text = match field("content_type").and_then(Value::as_str) {
        Some("application/pdf") => lj_sources::pdf::pdftotext_extract(&bytes)?,
        _ => lj_sources::html_strip::strip_html(&String::from_utf8_lossy(&bytes)),
    };
    Ok(Some(text))
}

/// What the OCR made of a scan's file.
enum Scan {
    /// Its text, from the answer the bucket keeps or read now.
    Read(String),
    /// Its file unreadable, or refused by the OCR.
    Unread,
    /// No key reads it now, or the run is past its budget: no run visited it.
    Waiting,
}

/// Whether the OCR failed for want of a key rather than on the file.
fn ocr_waits(e: &anyhow::Error) -> bool {
    match e.downcast_ref::<MistralError>() {
        Some(MistralError::Status(code)) => matches!(code, 401 | 402 | 429) || *code >= 500,
        Some(MistralError::ImageTooLarge) | None => false,
        Some(_) => true,
    }
}

/// Pages the cursor trails the fetch by: the stream holds at most 1,280 candidates, 6.4 pages, a
/// crash can leave unwritten.
const CURSOR_LAG: usize = 8;

/// One page of hits after another into the stream, each page kept, what it could not read whole
/// and the latest day scraped noted.
struct Loader<'a> {
    client: &'a EntscheidsucheClient,
    chambers: &'a HashMap<String, String>,
    pages: &'a Path,
    bucket: &'a Bucket,
    ocr: Option<Ocr>,
    conn: &'a Connection,
    /// Past it, a scan waits for the next run rather than for the OCR.
    deadline: Option<Instant>,
    unread: HashMap<String, String>,
    read: usize,
    refusals: usize,
    originals: usize,
    originals_missing: usize,
    scans: usize,
    scans_waiting: usize,
    scraped: Option<String>,
}

fn scrapedate(hit: &Hit) -> Result<String> {
    let doc: Value = serde_json::from_str(hit.source.get())?;
    Ok(doc
        .get("scrapedate")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

impl Loader<'_> {
    /// The text of an [`entscheidsuche_scan`]'s file, `None` for any other document: Mistral's
    /// answer the bucket keeps, else the file fetched and read by the OCR, its answer kept.
    async fn scan(&self, hit: &Hit) -> Result<Option<Scan>> {
        let doc: Value = serde_json::from_str(hit.source.get())?;
        if !entscheidsuche_scan(&doc) {
            return Ok(None);
        }
        let answer = key::entscheidsuche_ocr(&hit.id);
        if let Some(pages) = ocr::kept::<Vec<Value>>(self.bucket, &answer).await? {
            return Ok(Some(Scan::Read(ocr::text(&pages))));
        }
        let past = self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline);
        let Some(ocr) = self.ocr.as_ref().filter(|ocr| !ocr.spent() && !past) else {
            return Ok(Some(Scan::Waiting));
        };
        let url = doc
            .pointer("/attachment/content_url")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("entscheidsuche {}: scan without content_url", hit.id))?;
        let pdf = match self.client.original(url).await {
            Ok(pdf) => pdf,
            Err(e) => {
                tracing::debug!(id = %hit.id, error = %e, "entscheidsuche scan unreadable");
                return Ok(Some(Scan::Unread));
            }
        };
        Ok(Some(
            match ocr.document_kept(self.bucket, &answer, &pdf, &hit.id).await {
                Ok(reading) => Scan::Read(reading.text()),
                Err(e) if ocr_waits(&e) => {
                    tracing::warn!(id = %hit.id, error = %e, "entscheidsuche: a scan waits for a Mistral key");
                    Scan::Waiting
                }
                Err(e) => {
                    tracing::warn!(id = %hit.id, error = %e, "entscheidsuche: the OCR of a scan failed");
                    Scan::Unread
                }
            },
        ))
    }

    /// `false` once the stream stopped taking candidates.
    async fn load(&mut self, hits: &[Hit], tx: &mpsc::Sender<Candidate>) -> Result<bool> {
        for hit in hits {
            let (original, missing) = match original_text(self.client, hit).await {
                Ok(None) => match self.scan(hit).await? {
                    Some(Scan::Read(text)) => {
                        self.scans += 1;
                        (Some(text), false)
                    }
                    Some(Scan::Unread) => (None, true),
                    Some(Scan::Waiting) => {
                        self.scans_waiting += 1;
                        continue;
                    }
                    None => (None, false),
                },
                Ok(original) => (original, false),
                Err(e) => {
                    tracing::debug!(id = %hit.id, error = %e, "entscheidsuche original unreadable");
                    (None, true)
                }
            };
            self.originals += usize::from(original.is_some());
            self.originals_missing += usize::from(missing);
            match candidate(hit, self.chambers, original.as_deref()) {
                Ok(c) => {
                    let day = c.prebuilt_source_fields.as_ref().and_then(|sf| {
                        sf.get("scrapedate")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    });
                    if day > self.scraped {
                        self.scraped = day;
                    }
                    if missing {
                        self.unread.insert(hit.id.clone(), visit(&scrapedate(hit)?));
                    } else {
                        self.unread.remove(&hit.id);
                    }
                    if tx.send(c).await.is_err() {
                        return Ok(false);
                    }
                }
                Err(e) => {
                    tracing::debug!(id = %hit.id, error = %e, "entscheidsuche document refused");
                    self.unread.insert(hit.id.clone(), visit(&scrapedate(hit)?));
                    self.refusals += 1;
                }
            }
        }
        if let Some(last) = hits.last() {
            let lines: String = hits
                .iter()
                .map(|h| format!("{}\n", h.source.get()))
                .collect();
            std::fs::write(
                self.pages.join(format!("{}.jsonl", file_name(&last.id))),
                lines,
            )?;
        }
        self.read += hits.len();
        if let Some(ocr) = &self.ocr {
            ocr.park_spent(&DecisionRepository::new(self.conn)).await?;
        }
        tracing::info!(
            read = self.read,
            refused = self.refusals,
            originals = self.originals,
            originals_missing = self.originals_missing,
            scans = self.scans,
            scans_waiting = self.scans_waiting,
            "entscheidsuche sync"
        );
        Ok(true)
    }

    /// Writes what it could not read whole over the file as it stands now, which another set of
    /// courts may have written since this run read it.
    fn save_unread(&self, path: &Path, read: &HashMap<String, String>) -> Result<()> {
        let mut now = read_unread(path)?;
        for id in read.keys().filter(|id| !self.unread.contains_key(*id)) {
            now.remove(id);
        }
        now.extend(self.unread.clone());
        write_json(path, &now)
    }
}

/// Why the catch-up reads a document again.
#[derive(Debug, Default, PartialEq)]
struct Wanted {
    ids: Vec<String>,
    absent: usize,
    rescraped: usize,
    cut: usize,
}

/// The documents of the listing the base does not hold as the index serves them: absent, scraped
/// again since the base read them, or cut by the index where the base kept the cut text; an
/// absent or cut one only when no run read it at its scrape day under the current parse.
fn wanted(
    listed: &[Listed],
    held: &HashMap<String, (Option<String>, bool)>,
    unread: &HashMap<String, String>,
) -> Wanted {
    let mut w = Wanted::default();
    for l in listed {
        let visited =
            unread.get(&l.id) == Some(&visit(l.scrapedate.as_deref().unwrap_or_default()));
        let want = match held.get(&l.id) {
            None => {
                w.absent += usize::from(!visited);
                !visited
            }
            Some((scraped, _)) if *scraped != l.scrapedate => {
                w.rescraped += 1;
                true
            }
            Some((_, from_original)) if l.cut && !from_original && !visited => {
                w.cut += 1;
                true
            }
            Some(_) => false,
        };
        if want {
            w.ids.push(l.id.clone());
        }
    }
    w
}

/// Reads the stock from where it stopped, then the days scraped since the last run. `courts`
/// narrows it to some courts or chambers, each set with its own cursor; `full` spends no time
/// budget; `limit` stops after that many documents. `missing` lists the index's ids instead and
/// reads every document the base does not hold as the index serves it, or with `rewrite` every
/// listed one, its decision written again.
pub async fn sync_entscheidsuche(
    full: bool,
    limit: Option<usize>,
    courts: &[String],
    missing: bool,
    rewrite: bool,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = dir(&settings);
    let pages = dir.join("pages");
    std::fs::create_dir_all(&pages)?;
    let mut courts: Vec<&str> = courts.iter().map(String::as_str).collect();
    let taken = |id: &str| {
        SWISS_COURTS.iter().any(|court| {
            id.strip_prefix(court.id)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('_'))
        })
    };
    if let Some(unknown) = courts.iter().find(|c| !taken(c)) {
        bail!("entscheidsuche: court {unknown} is not taken");
    }
    courts.sort_unstable();
    courts.dedup();
    let set = courts.join("+");
    let cursor_path = if courts.is_empty() {
        dir.join("cursor.json")
    } else {
        dir.join(format!("cursor-{set}.json"))
    };
    if courts.is_empty() {
        courts = SWISS_COURTS.iter().map(|c| c.id).collect();
    }
    let unread_path = dir.join("unread.json");
    let unread = read_unread(&unread_path)?;

    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let write_conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    if !DecisionRepository::new(&conn)
        .entscheidsuche_lock(&set)
        .await?
    {
        tracing::info!(courts = %set, "entscheidsuche sync: another run holds the lock");
        return Ok(());
    }
    let client = EntscheidsucheClient::new(&settings.egress_proxy)?;
    let facets = client.facets().await?;
    let chambers = chamber_labels(&facets);
    let named = DecisionRepository::new(&write_conn)
        .write_jurisdiction_names(&court_names(&facets))
        .await?;
    tracing::info!(named, "entscheidsuche sync: court names written");
    let bucket = settings.bucket()?;
    let mut loader = Loader {
        client: &client,
        chambers: &chambers,
        pages: &pages,
        bucket: &bucket,
        ocr: Ocr::from_pool(&DecisionRepository::new(&conn)).await?,
        conn: &conn,
        deadline: None,
        unread: unread.clone(),
        read: 0,
        refusals: 0,
        originals: 0,
        originals_missing: 0,
        scans: 0,
        scans_waiting: 0,
        scraped: None,
    };
    if missing {
        catch_up(
            &conn,
            &write_conn,
            &mut loader,
            &courts,
            full,
            limit,
            &unread,
            rewrite,
        )
        .await?;
    } else {
        window(
            &conn,
            &write_conn,
            &mut loader,
            &courts,
            full,
            limit,
            &cursor_path,
        )
        .await?;
    }
    loader.save_unread(&unread_path, &unread)?;
    Ok(())
}

/// The stock from the cursor's id, or once it is read whole, the days scraped since the cursor's.
async fn window(
    conn: &Connection,
    write_conn: &Connection,
    loader: &mut Loader<'_>,
    courts: &[&str],
    full: bool,
    limit: Option<usize>,
    cursor_path: &Path,
) -> Result<()> {
    let mut cursor = Cursor::read(cursor_path)?;
    let since = cursor.stock_done.then(|| cursor.scraped.clone()).flatten();
    loader.scraped = cursor.scraped.clone();
    tracing::info!(stock = !cursor.stock_done, after = ?cursor.after, since = ?since, "entscheidsuche sync: start");

    let mut ended = false;
    loader.deadline = (!full).then(|| Instant::now() + BUDGET);
    let (cursor_ref, ended_ref, loader_ref) = (&mut cursor, &mut ended, &mut *loader);
    let counts = run_stream(conn, write_conn, IngestMode::MissingHash, |tx| async move {
        let started = Instant::now();
        let mut after = cursor_ref.after.clone();
        let mut trail: VecDeque<String> = VecDeque::new();
        while limit.is_none_or(|l| loader_ref.read < l) && (full || started.elapsed() < BUDGET) {
            let hits = loader_ref
                .client
                .page(courts, after.as_deref(), since.as_deref())
                .await?;
            let Some(last) = hits.last() else {
                *ended_ref = true;
                break;
            };
            let last = last.id.clone();
            if !loader_ref.load(&hits, &tx).await? {
                return Ok(IngestCounts::default());
            }
            after = Some(last.clone());
            trail.push_back(last);
            if trail.len() > CURSOR_LAG {
                cursor_ref.after = trail.pop_front();
                if !cursor_ref.stock_done {
                    cursor_ref.scraped = loader_ref.scraped.clone();
                }
                cursor_ref.write(cursor_path)?;
            }
        }
        Ok(IngestCounts::default())
    })
    .await?;
    if ended {
        cursor.stock_done = true;
        cursor.after = None;
        cursor.scraped = loader.scraped.clone();
        cursor.write(cursor_path)?;
    } else {
        tracing::warn!(
            read = loader.read,
            after = ?cursor.after,
            "entscheidsuche sync: stopped before the end, the next run resumes"
        );
    }
    tracing::warn!(
        read = loader.read,
        refused = loader.refusals,
        originals = loader.originals,
        originals_missing = loader.originals_missing,
        scans = loader.scans,
        scans_waiting = loader.scans_waiting,
        created = counts.created,
        updated = counts.updated,
        skipped = counts.skipped,
        errors = counts.errors,
        "entscheidsuche sync done"
    );
    Ok(())
}

/// Lists the index's ids for `courts` and reads the documents [`wanted`] names, or every listed
/// one when `rewrite`.
#[allow(clippy::too_many_arguments)]
async fn catch_up(
    conn: &Connection,
    write_conn: &Connection,
    loader: &mut Loader<'_>,
    courts: &[&str],
    full: bool,
    limit: Option<usize>,
    unread: &HashMap<String, String>,
    rewrite: bool,
) -> Result<()> {
    let started = Instant::now();
    let mut listed: Vec<Listed> = Vec::new();
    loop {
        let page = loader
            .client
            .listing(courts, listed.last().map(|l| l.id.as_str()))
            .await?;
        if page.is_empty() {
            break;
        }
        listed.extend(page);
        tracing::info!(listed = listed.len(), "entscheidsuche catch-up: listing");
    }
    let held: HashMap<String, (Option<String>, bool)> = DecisionRepository::new(conn)
        .entscheidsuche_held()
        .await?
        .into_iter()
        .map(|(id, scraped, original)| (id, (scraped, original)))
        .collect();
    let in_index: std::collections::HashSet<&str> = listed.iter().map(|l| l.id.as_str()).collect();
    let gone = held
        .keys()
        .filter(|id| {
            courts
                .iter()
                .any(|c| id.strip_prefix(c).is_some_and(|rest| rest.starts_with('_')))
                && !in_index.contains(id.as_str())
        })
        .count();
    let w = wanted(&listed, &held, unread);
    tracing::warn!(
        listed = listed.len(),
        absent = w.absent,
        rescraped = w.rescraped,
        cut = w.cut,
        held_gone_from_index = gone,
        rewrite,
        "entscheidsuche catch-up: to read"
    );
    let (ids, mode) = if rewrite {
        (
            listed.iter().map(|l| l.id.clone()).collect::<Vec<_>>(),
            IngestMode::All,
        )
    } else {
        (w.ids, IngestMode::MissingHash)
    };
    loader.deadline = (!full).then(|| started + CATCH_UP_BUDGET);
    let loader_ref = &mut *loader;
    let counts = run_stream(conn, write_conn, mode, |tx| async move {
        for chunk in ids.chunks(PAGE) {
            if limit.is_some_and(|l| loader_ref.read >= l)
                || (!full && started.elapsed() >= CATCH_UP_BUDGET)
            {
                tracing::warn!(
                    read = loader_ref.read,
                    "entscheidsuche catch-up: stopped before the end, the next run resumes"
                );
                break;
            }
            let chunk: Vec<&str> = chunk.iter().map(String::as_str).collect();
            let hits = loader_ref.client.by_ids(&chunk).await?;
            if !loader_ref.load(&hits, &tx).await? {
                break;
            }
        }
        Ok(IngestCounts::default())
    })
    .await?;
    tracing::warn!(
        read = loader.read,
        refused = loader.refusals,
        originals = loader.originals,
        originals_missing = loader.originals_missing,
        scans = loader.scans,
        scans_waiting = loader.scans_waiting,
        created = counts.created,
        updated = counts.updated,
        skipped = counts.skipped,
        errors = counts.errors,
        "entscheidsuche catch-up done"
    );
    Ok(())
}

fn file_name(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(id: &str, scrapedate: &str, cut: bool) -> Listed {
        Listed {
            id: id.to_string(),
            scrapedate: Some(scrapedate.to_string()),
            cut,
        }
    }

    #[test]
    fn the_catch_up_reads_what_the_base_does_not_hold_as_the_index_serves_it() {
        let listing = [
            listed("GE_CJ_001_A", "2026-11-01", false),
            listed("GE_CJ_001_B", "2026-11-01", false),
            listed("GE_CJ_001_C", "2026-11-01", false),
            listed("GE_CJ_001_D", "2026-11-16", false),
            listed("GE_CJ_001_E", "2026-11-01", false),
            listed("GE_CJ_001_I", "2026-11-01", false),
            listed("CH_BVGE_001_F", "2026-10-01", true),
            listed("CH_BVGE_001_G", "2026-10-01", true),
            listed("CH_BVGE_001_H", "2026-10-01", true),
        ];
        let held = HashMap::from([
            (
                "GE_CJ_001_A".to_string(),
                (Some("2026-11-01".to_string()), false),
            ),
            (
                "GE_CJ_001_D".to_string(),
                (Some("2026-07-15".to_string()), false),
            ),
            (
                "CH_BVGE_001_F".to_string(),
                (Some("2026-10-01".to_string()), false),
            ),
            (
                "CH_BVGE_001_G".to_string(),
                (Some("2026-10-01".to_string()), true),
            ),
            (
                "CH_BVGE_001_H".to_string(),
                (Some("2026-10-01".to_string()), false),
            ),
        ]);
        let unread = HashMap::from([
            ("GE_CJ_001_C".to_string(), visit("2026-11-01")),
            ("GE_CJ_001_E".to_string(), visit("2026-09-01")),
            ("GE_CJ_001_I".to_string(), "2026-11-01".to_string()),
            ("CH_BVGE_001_H".to_string(), visit("2026-10-01")),
        ]);
        let w = wanted(&listing, &held, &unread);
        assert_eq!(
            w,
            Wanted {
                ids: [
                    "GE_CJ_001_B",
                    "GE_CJ_001_D",
                    "GE_CJ_001_E",
                    "GE_CJ_001_I",
                    "CH_BVGE_001_F",
                ]
                .map(String::from)
                .to_vec(),
                absent: 3,
                rescraped: 1,
                cut: 1,
            }
        );
    }
}
