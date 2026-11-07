//! The decisions of the Défenseur des droits and of the authorities it succeeded, from its catalogue.

use std::collections::hash_map::Entry;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use bytes::Bytes;
use lj_bucket::{key, Bucket};
use lj_core::decision::Decision;
use lj_parse::{clean_ocr_markdown, ddd_source_fields, reflow_page_text, FromSourceFields};
use lj_pg::db::Connection;
use lj_sources::ddd::{append_notice, ocr_path, read_notices, DddClient, DddNotice};

use crate::config::Settings;
use crate::ocr::Ocr;

use super::batch::drain_batch;
use super::cnda::pdf_has_text_layer;
use super::europe::europe_pool;
use super::{
    content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode, BATCH_SIZE,
};

/// Reads the newest page of the decisions list, every page with `full`, fetches the notices the
/// cache lacks, and writes each decision listed as it goes.
pub async fn sync_ddd(full: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.paths().ddd();
    let bucket = settings.bucket()?;
    let client = DddClient::new(&settings.egress_proxy)?;
    let mut notices = read_notices(&dir)?;
    let listed = client.decision_ids((!full).then_some(1)).await?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let mut writer = Writer::open(&dir, &bucket, &conn, IngestMode::MissingHash, &listed).await?;
    let mut added = Vec::new();
    for id in listed {
        let notice = match notices.entry(id) {
            Entry::Occupied(cached) => cached.into_mut(),
            Entry::Vacant(slot) => {
                let Some(notice) = client.notice(id).await? else {
                    tracing::warn!(notice = id, "a notice of the decisions list has no page");
                    continue;
                };
                if let Some(&first) = notice.explnum.first() {
                    let file = key::ddd(first);
                    if !bucket.contains(&file).await? {
                        bucket.put(&file, client.file(first).await?).await?;
                    }
                }
                append_notice(&dir, &notice)?;
                added.push(id);
                slot.insert(notice)
            }
        };
        writer.push(notice).await?;
    }
    if full && !added.is_empty() {
        tracing::warn!(notices = ?added, "the full reading of the decisions list added notices");
    }
    writer.finish().await
}

/// Writes every decision the cache holds, without reading the catalogue.
pub async fn ingest_ddd(mode: IngestMode) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.paths().ddd();
    let bucket = settings.bucket()?;
    let notices = read_notices(&dir)?;
    let mut ids: Vec<u64> = notices.keys().copied().collect();
    ids.sort_unstable();
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let mut writer = Writer::open(&dir, &bucket, &conn, mode, &ids).await?;
    for id in ids {
        writer.push(&notices[&id]).await?;
    }
    writer.finish().await
}

struct Writer<'a> {
    dir: PathBuf,
    bucket: Bucket,
    conn: &'a Connection,
    ocr: Option<Ocr>,
    mode: IngestMode,
    /// The notices already written: a notice's file never changes, so only mode `all` reads it
    /// again.
    written: HashSet<String>,
    batch: Vec<Candidate>,
    counts: IngestCounts,
    without_text: usize,
}

impl<'a> Writer<'a> {
    async fn open(
        dir: &Path,
        bucket: &Bucket,
        conn: &'a Connection,
        mode: IngestMode,
        ids: &[u64],
    ) -> Result<Self> {
        let repo = lj_store::repository::DecisionRepository::new(conn);
        let ocr = Ocr::from_pool(&repo).await?;
        let written = match mode {
            IngestMode::MissingHash => {
                let uids: Vec<String> = ids.iter().map(|id| format!("ddd/{id}")).collect();
                repo.known_source_uids(&uids).await?
            }
            IngestMode::All => HashSet::new(),
        };
        Ok(Self {
            dir: dir.to_path_buf(),
            bucket: bucket.clone(),
            conn,
            ocr,
            mode,
            written,
            batch: Vec::new(),
            counts: IngestCounts::default(),
            without_text: 0,
        })
    }

    async fn push(&mut self, notice: &DddNotice) -> Result<()> {
        if !notice.is_decision() || self.written.contains(&format!("ddd/{}", notice.id)) {
            return Ok(());
        }
        let Some((text, bytes)) =
            text_of(&self.bucket, &self.dir, notice, self.ocr.as_ref()).await?
        else {
            self.without_text += 1;
            return Ok(());
        };
        self.batch.push(candidate(notice, &text, &bytes));
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
            without_text = self.without_text,
            "ddd"
        );
        Ok(())
    }
}

fn candidate(notice: &DddNotice, text: &str, bytes: &[u8]) -> Candidate {
    let uid = format!("ddd/{}", notice.id);
    let source_fields = ddd_source_fields(notice.id, &notice.fields, &notice.explnum);
    Candidate {
        decision_id: None,
        public_id: generate_public_id(),
        content_checksum: content_checksum(bytes),
        decision: Decision::from_source_fields(text, &source_fields, &uid),
        prebuilt_source_fields: Some(source_fields),
        prebuilt_extracted: None,
        raw_payload: Vec::new(),
        payload_format: "pdf".to_string(),
        write_mode: super::WriteMode::Full,
        passages: true,
        distinct: false,
    }
}

/// The text of the notice's first file, the decision its others translate or follow; `None` with
/// no file, or a scan no OCR could read.
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
    let cached = ocr_path(dir, first);
    let markdown = if cached.exists() {
        std::fs::read_to_string(&cached)?
    } else {
        let Some(client) = ocr else {
            tracing::warn!(
                notice = notice.id,
                "a scanned decision waits for a Mistral key"
            );
            return Ok(None);
        };
        let markdown = match client.document(&bytes, &notice.id.to_string()).await {
            Ok(markdown) => markdown,
            Err(e) => {
                tracing::warn!(notice = notice.id, error = %e, "the OCR of a scanned decision failed");
                return Ok(None);
            }
        };
        std::fs::create_dir_all(cached.parent().unwrap())?;
        std::fs::write(&cached, &markdown)?;
        markdown
    };
    Ok(Some((clean_ocr_markdown(&markdown), bytes)))
}
