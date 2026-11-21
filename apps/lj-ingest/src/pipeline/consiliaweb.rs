//! The Conseil d'État's consultative opinions, from ConsiliaWeb's census and their files.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use lj_bucket::{key, Bucket};
use lj_core::decision::Decision;
use lj_parse::{
    clean_ocr_markdown, consiliaweb_number, consiliaweb_source_fields, FromSourceFields,
};
use lj_sources::consiliaweb::{file_url, ConsiliaClient};
use lj_store::repository::DecisionRepository;
use serde_json::Value;

use super::batch::drain_batch;
use super::europe::europe_pool;
use super::juradm_site::file_text;
use super::{content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode};
use crate::config::Settings;
use crate::ocr::{self, Ocr};

const BATCH: usize = 64;
/// A scan's text layer holds a few characters, 2 to 11 on the three sampled.
const SCAN_CHARS: usize = 200;

/// Writes the opinions the base lacks, newest first until a page it holds whole, every page if `full`.
pub async fn sync_consiliaweb(full: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let client = ConsiliaClient::new(&settings.egress_proxy)?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let ocr = Ocr::from_pool(&repo).await?;
    let scans = Scans {
        ocr: ocr.as_ref(),
        bucket: settings.bucket()?,
        cache: settings.state_dir.join("consiliaweb").join("ocr"),
        read: Cell::new(0),
    };

    let mut counts = IngestCounts::default();
    let mut batch = Vec::new();
    let mut skipped: HashMap<&'static str, usize> = HashMap::new();
    let (mut listed, mut held, mut pages) = (0usize, 0usize, 0u32);
    for page in 1.. {
        let census = client.census_page(page).await?;
        pages = page;
        let hits: Vec<(String, Value)> = census
            .documents
            .into_iter()
            .filter_map(|hit| Some((format!("consiliaweb/{}", consiliaweb_number(&hit)?), hit)))
            .collect();
        listed += hits.len();
        let uids: Vec<String> = hits.iter().map(|(uid, _)| uid.clone()).collect();
        let known = repo.known_source_uids(&uids).await?;
        held += known.len();
        let fresh: Vec<(String, Value)> = hits
            .into_iter()
            .filter(|(uid, _)| !known.contains(uid))
            .collect();
        let last = (fresh.is_empty() && !full) || page >= census.page_count;
        for (uid, hit) in fresh {
            read(&client, &scans, uid, hit, &mut batch, &mut skipped).await?;
            if batch.len() >= BATCH {
                drain_batch(
                    &conn,
                    std::mem::take(&mut batch),
                    IngestMode::MissingHash,
                    &mut counts,
                )
                .await?;
            }
        }
        if last {
            break;
        }
    }
    drain_batch(&conn, batch, IngestMode::MissingHash, &mut counts).await?;
    if let Some(ocr) = &ocr {
        ocr.park_spent(&repo).await?;
    }
    tracing::info!(
        pages,
        listed,
        held,
        created = counts.created,
        updated = counts.updated,
        scans_read = scans.read.get(),
        skipped = ?skipped,
        "consiliaweb"
    );
    Ok(())
}

/// Rewrites every opinion the base holds from its stored text and census fields, the site left unread,
/// a scan's text from Mistral's answer kept in the bucket. An opinion resting on another source's
/// judgment leaves it, for the next full pass to file apart.
pub async fn rewrite_consiliaweb() -> Result<()> {
    let bucket = Settings::from_env()?.bucket()?;
    let answers: HashSet<String> = bucket.keys("consiliaweb/").await?.into_iter().collect();
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let mut held = Vec::new();
    for provenance in repo.source_provenances("consiliaweb").await? {
        let (id, uid) = (provenance.0, &provenance.1);
        if repo.authority_source_uid(id).await?.as_ref() == Some(uid) {
            held.push(provenance);
        } else {
            repo.delete_provenance(uid).await?;
            tracing::warn!(
                uid,
                decision_id = id,
                "consiliaweb: opinion taken off another source's judgment"
            );
        }
    }
    let mut counts = IngestCounts::default();
    for chunk in held.chunks(BATCH) {
        let ids: Vec<i64> = chunk.iter().map(|(id, ..)| *id).collect();
        let inputs: HashMap<i64, (String, Vec<(String, Value)>)> = repo
            .fetch_reextract_inputs_batch(&ids)
            .await?
            .into_iter()
            .map(|(id, text, provenances, _)| (id, (text, provenances)))
            .collect();
        let mut batch = Vec::with_capacity(chunk.len());
        for (id, uid, checksum, format) in chunk {
            let (text, provenances) = inputs
                .get(id)
                .ok_or_else(|| anyhow!("{uid}: no text in the base"))?;
            let fields = provenances
                .iter()
                .find(|(u, _)| u == uid)
                .map(|(_, f)| f.clone())
                .ok_or_else(|| anyhow!("{uid}: no source fields in the base"))?;
            let answer = key::consiliaweb_ocr(uid.trim_start_matches("consiliaweb/"));
            let text = if answers.contains(&answer) {
                let pages: Vec<Value> = ocr::kept(&bucket, &answer)
                    .await?
                    .ok_or_else(|| anyhow!("{answer}: gone from the bucket"))?;
                clean_ocr_markdown(&ocr::text(&pages))
            } else {
                text.clone()
            };
            batch.push(Candidate {
                decision_id: Some(*id),
                public_id: generate_public_id(),
                content_checksum: checksum.clone(),
                decision: Decision::from_source_fields(&text, &fields, uid),
                prebuilt_source_fields: Some(fields),
                prebuilt_extracted: None,
                raw_payload: Vec::new(),
                payload_format: format.clone(),
                write_mode: super::WriteMode::Full,
                passages: true,
                distinct: false,
            });
        }
        drain_batch(&conn, batch, IngestMode::All, &mut counts).await?;
    }
    tracing::info!(
        held = held.len(),
        updated = counts.updated,
        skipped = counts.skipped,
        "consiliaweb: rewritten from the base"
    );
    Ok(())
}

/// The OCR that reads a scan, and where its reading is kept so that a run never pays twice.
struct Scans<'a> {
    ocr: Option<&'a Ocr>,
    bucket: Bucket,
    /// The markdown of the scans read before Mistral's answer was kept.
    cache: PathBuf,
    read: Cell<usize>,
}

impl Scans<'_> {
    /// The text of a scanned opinion, from Mistral's answer kept in the bucket, else from the
    /// markdown cached before, else read by the OCR, its answer kept. `None` while no OCR reads it.
    async fn text(&self, uid: &str, pdf: &[u8]) -> Result<Option<String>> {
        let number = uid.trim_start_matches("consiliaweb/");
        let answer = key::consiliaweb_ocr(number);
        let cached = self.cache.join(format!("{number}.md"));
        let markdown = if let Some(pages) = ocr::kept::<Vec<Value>>(&self.bucket, &answer).await? {
            ocr::text(&pages)
        } else if cached.exists() {
            std::fs::read_to_string(&cached)?
        } else {
            let Some(ocr) = self.ocr else {
                return Ok(None);
            };
            match ocr.document_kept(&self.bucket, &answer, pdf, uid).await {
                Ok(reading) => reading.text(),
                Err(e) => {
                    tracing::warn!(uid, error = %e, "consiliaweb: the OCR of a scan failed");
                    return Ok(None);
                }
            }
        };
        self.read.set(self.read.get() + 1);
        Ok(Some(clean_ocr_markdown(&markdown)))
    }
}

/// Reads the file of `hit` and queues its opinion, or counts why it is not read.
async fn read(
    client: &ConsiliaClient,
    scans: &Scans<'_>,
    uid: String,
    hit: Value,
    batch: &mut Vec<Candidate>,
    skipped: &mut HashMap<&'static str, usize>,
) -> Result<()> {
    let id = hit["Id"]
        .as_str()
        .ok_or_else(|| anyhow!("{uid}: a census hit without an Id"))?
        .to_string();
    let bytes = client.file(&id).await?;
    let Some((format, text)) = file_text(&bytes) else {
        tracing::warn!(uid, "consiliaweb: a file neither PDF nor Word");
        *skipped.entry("format").or_default() += 1;
        return Ok(());
    };
    let text = if text.chars().filter(|c| !c.is_whitespace()).count() >= SCAN_CHARS {
        text
    } else if format != "pdf" {
        tracing::warn!(uid, format, "consiliaweb: a Word file without text");
        *skipped.entry("empty").or_default() += 1;
        return Ok(());
    } else if let Some(text) = scans.text(&uid, &bytes).await? {
        text
    } else {
        *skipped.entry("scan").or_default() += 1;
        return Ok(());
    };
    let fields = consiliaweb_source_fields(hit, &file_url(&id));
    batch.push(Candidate {
        decision_id: None,
        public_id: generate_public_id(),
        content_checksum: content_checksum(&bytes),
        decision: Decision::from_source_fields(&text, &fields, &uid),
        prebuilt_source_fields: Some(fields),
        prebuilt_extracted: None,
        raw_payload: Vec::new(),
        payload_format: format.to_string(),
        write_mode: super::WriteMode::Full,
        passages: true,
        distinct: false,
    });
    Ok(())
}
