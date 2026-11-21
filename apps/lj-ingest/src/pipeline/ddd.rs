//! The decisions of the Défenseur des droits and of the authorities it succeeded, and the court
//! decisions it files beside them, from its catalogue.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use bytes::Bytes;
use lj_bucket::{key, Bucket};
use lj_core::decision::Decision;
use lj_parse::{
    clean_ocr_markdown, ddd_concerns_minor, ddd_jurisprudence_court, ddd_jurisprudence_source_uid,
    ddd_source_fields, reflow_page_text, FromSourceFields,
};
use lj_pg::db::Connection;
use lj_sources::ddd::{append_notice, ocr_path, read_notices, DddClient, DddNotice};
use serde_json::Value;

use crate::config::Settings;
use crate::ocr::{self, Ocr};

use super::batch::drain_batch;
use super::cnda::pdf_has_text_layer;
use super::europe::europe_pool;
use super::legalis::{placed, pseudonymised};
use super::{
    content_checksum, extract_ctx, generate_public_id, Candidate, ExtractCtx, IngestCounts,
    IngestMode, BATCH_SIZE,
};

/// The newest page of the decisions list then of the court decisions list, every page with `full`.
pub async fn sync_ddd(full: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.paths().ddd();
    let bucket = settings.bucket()?;
    let client = DddClient::new(&settings.egress_proxy)?;
    let mut notices = read_notices(&dir)?;
    let pages = (!full).then_some(1);
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    if !lj_store::repository::DecisionRepository::new(&conn)
        .ddd_lock()
        .await?
    {
        tracing::info!("ddd sync: another run holds the lock");
        return Ok(());
    }
    let mut added = Vec::new();

    let listed = client.decision_ids(pages).await?;
    let uids: Vec<String> = listed.iter().map(|id| format!("ddd/{id}")).collect();
    let mut writer = Writer::open(&dir, &bucket, &conn, IngestMode::MissingHash, &uids).await?;
    for id in listed {
        if let Some(notice) =
            cached_or_fetched(&client, &bucket, &dir, &mut notices, id, &mut added).await?
        {
            writer.push(notice).await?;
        }
    }
    writer.finish().await?;

    let listed: Vec<u64> = client
        .jurisprudence_rows(pages)
        .await?
        .into_iter()
        .filter(|row| ddd_jurisprudence_court(&row.author, "").is_some())
        .map(|row| row.id)
        .collect();
    let uids: Vec<String> = listed
        .iter()
        .map(|&id| ddd_jurisprudence_source_uid(id))
        .collect();
    let mut writer = Writer::open(&dir, &bucket, &conn, IngestMode::MissingHash, &uids).await?;
    for (id, uid) in listed.into_iter().zip(uids) {
        if writer.written.contains(&uid) {
            continue;
        }
        if let Some(notice) =
            cached_or_fetched(&client, &bucket, &dir, &mut notices, id, &mut added).await?
        {
            writer.push(notice).await?;
        }
    }
    writer.finish().await?;

    if full && !added.is_empty() {
        tracing::warn!(notices = ?added, "the full reading of the lists added notices");
    }
    Ok(())
}

/// The notice `id` from the cache, else from the catalogue with its first file, none for a court
/// decision that concerns a minor; `None` for an id the catalogue has no page under.
async fn cached_or_fetched<'n>(
    client: &DddClient,
    bucket: &Bucket,
    dir: &Path,
    notices: &'n mut HashMap<u64, DddNotice>,
    id: u64,
    added: &mut Vec<u64>,
) -> Result<Option<&'n DddNotice>> {
    match notices.entry(id) {
        Entry::Occupied(cached) => Ok(Some(cached.into_mut())),
        Entry::Vacant(slot) => {
            let Some(notice) = client.notice(id).await? else {
                tracing::warn!(notice = id, "a listed notice has no page");
                return Ok(None);
            };
            let minor = notice.is_jurisprudence() && ddd_concerns_minor(&notice.fields);
            if let (false, Some(&first)) = (minor, notice.explnum.first()) {
                let file = key::ddd(first);
                if !bucket.contains(&file).await? {
                    bucket.put(&file, client.file(first).await?).await?;
                }
            }
            append_notice(dir, &notice)?;
            added.push(id);
            Ok(Some(slot.insert(notice)))
        }
    }
}

/// Writes every notice the cache holds, without reading the catalogue.
pub async fn ingest_ddd(mode: IngestMode) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.paths().ddd();
    let bucket = settings.bucket()?;
    let notices = read_notices(&dir)?;
    let mut ids: Vec<u64> = notices.keys().copied().collect();
    ids.sort_unstable();
    let uids: Vec<String> = ids
        .iter()
        .filter_map(|id| source_uid(&notices[id]))
        .collect();
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let mut writer = Writer::open(&dir, &bucket, &conn, mode, &uids).await?;
    for id in ids {
        writer.push(&notices[&id]).await?;
    }
    writer.finish().await
}

fn source_uid(notice: &DddNotice) -> Option<String> {
    if notice.is_decision() {
        Some(format!("ddd/{}", notice.id))
    } else if notice.is_jurisprudence() {
        Some(ddd_jurisprudence_source_uid(notice.id))
    } else {
        None
    }
}

struct Writer<'a> {
    dir: PathBuf,
    bucket: Bucket,
    conn: &'a Connection,
    ctx: &'static ExtractCtx,
    codes: HashSet<&'static str>,
    ocr: Option<Ocr>,
    mode: IngestMode,
    /// The notices already written: a notice's file never changes, so only mode `all` reads it
    /// again.
    written: HashSet<String>,
    batch: Vec<Candidate>,
    counts: IngestCounts,
    without_text: usize,
    minors: usize,
    unplaced: usize,
}

impl<'a> Writer<'a> {
    async fn open(
        dir: &Path,
        bucket: &Bucket,
        conn: &'a Connection,
        mode: IngestMode,
        uids: &[String],
    ) -> Result<Self> {
        let repo = lj_store::repository::DecisionRepository::new(conn);
        let ocr = Ocr::from_pool(&repo).await?;
        let written = match mode {
            IngestMode::MissingHash => repo.known_source_uids(uids).await?,
            IngestMode::All => HashSet::new(),
        };
        let ctx = extract_ctx(conn).await?;
        Ok(Self {
            dir: dir.to_path_buf(),
            bucket: bucket.clone(),
            conn,
            ctx,
            codes: ctx.jur_codes.values().map(String::as_str).collect(),
            ocr,
            mode,
            written,
            batch: Vec::new(),
            counts: IngestCounts::default(),
            without_text: 0,
            minors: 0,
            unplaced: 0,
        })
    }

    async fn push(&mut self, notice: &DddNotice) -> Result<()> {
        let Some(uid) = source_uid(notice) else {
            return Ok(());
        };
        if self.written.contains(&uid) {
            return Ok(());
        }
        let source_fields = ddd_source_fields(notice.id, &notice.fields, &notice.explnum);
        let court_decision = notice.is_jurisprudence();
        if court_decision {
            if ddd_concerns_minor(&notice.fields) {
                self.minors += 1;
                return Ok(());
            }
            let decision = Decision::from_source_fields("", &source_fields, &uid);
            if placed(&decision, self.ctx, &self.codes).is_none() {
                tracing::info!(notice = notice.id, court = ?decision.jurisdiction_name, "ddd court decision waits: no court placed");
                self.unplaced += 1;
                return Ok(());
            }
        }
        let Some((text, bytes)) =
            text_of(&self.bucket, &self.dir, notice, self.ocr.as_ref()).await?
        else {
            self.without_text += 1;
            return Ok(());
        };
        let text = if court_decision {
            pseudonymised(&text)?
        } else {
            text
        };
        self.batch.push(Candidate {
            decision_id: None,
            public_id: generate_public_id(),
            content_checksum: content_checksum(&bytes),
            decision: Decision::from_source_fields(&text, &source_fields, &uid),
            prebuilt_source_fields: Some(source_fields),
            prebuilt_extracted: None,
            raw_payload: Vec::new(),
            payload_format: "pdf".to_string(),
            write_mode: super::WriteMode::Full,
            passages: true,
            distinct: false,
        });
        if self.batch.len() >= BATCH_SIZE {
            let batch = std::mem::take(&mut self.batch);
            drain_batch(self.conn, batch, self.mode, &mut self.counts).await?;
        }
        Ok(())
    }

    async fn finish(mut self) -> Result<()> {
        if !self.batch.is_empty() {
            let batch = std::mem::take(&mut self.batch);
            drain_batch(self.conn, batch, self.mode, &mut self.counts).await?;
        }
        tracing::info!(
            created = self.counts.created,
            updated = self.counts.updated,
            skipped = self.counts.skipped,
            held = self.counts.held,
            without_text = self.without_text,
            minors = self.minors,
            unplaced = self.unplaced,
            "ddd"
        );
        Ok(())
    }
}

/// The text of the notice's first file, the decision its others translate or follow; `None` with
/// no file, or a scan no OCR could read. A scan is read from Mistral's answer kept beside its file,
/// else from the markdown cached before the answer was kept, else by the OCR, its answer kept.
async fn text_of(
    bucket: &Bucket,
    dir: &Path,
    notice: &DddNotice,
    ocr: Option<&Ocr>,
) -> Result<Option<(String, Bytes)>> {
    let Some(&first) = notice.explnum.first() else {
        return Ok(None);
    };
    let bytes = bucket.get(&key::ddd(first)).await?.ok_or_else(|| {
        anyhow!(
            "the file {first} of notice {} is not in the bucket",
            notice.id
        )
    })?;
    if let Some(raw) = lj_sources::pdf::pdftotext_extract(&bytes)
        .ok()
        .filter(|raw| pdf_has_text_layer(raw))
    {
        return Ok(Some((reflow_page_text(&raw), bytes)));
    }
    let answer = key::ocr(&key::ddd(first));
    let cached = ocr_path(dir, first);
    let markdown = if let Some(pages) = ocr::kept::<Vec<Value>>(bucket, &answer).await? {
        ocr::text(&pages)
    } else if cached.exists() {
        std::fs::read_to_string(&cached)?
    } else {
        let Some(client) = ocr else {
            tracing::warn!(
                notice = notice.id,
                "a scanned decision waits for a Mistral key"
            );
            return Ok(None);
        };
        match client
            .document_kept(bucket, &answer, &bytes, &notice.id.to_string())
            .await
        {
            Ok(reading) => reading.text(),
            Err(e) => {
                tracing::warn!(notice = notice.id, error = %e, "the OCR of a scanned decision failed");
                return Ok(None);
            }
        }
    };
    Ok(Some((clean_ocr_markdown(&markdown), bytes)))
}
