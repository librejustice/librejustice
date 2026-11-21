//! The decisions of the AMF's Commission des sanctions, from its site.

use anyhow::{anyhow, Result};
use lj_bucket::{key, Bucket};
use lj_core::decision::Decision;
use lj_parse::{amf_named_until, amf_source_fields, reflow_page_text, FromSourceFields};
use lj_pg::db::Connection;
use lj_sources::amf::{append_page, read_pages, AmfClient, AmfPage};

use crate::config::Settings;

use super::batch::drain_batch;
use super::cnda::pdf_has_text_layer;
use super::europe::europe_pool;
use super::{
    content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode, BATCH_SIZE,
};

/// Writes the pages the sitemap dates later than the cache, every page with `full`.
pub async fn sync_amf(full: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.paths().amf();
    let bucket = settings.bucket()?;
    let client = AmfClient::new()?;
    let mut pages = read_pages(&dir)?;
    let listed = client.listed().await?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let mut writer = Writer::new(&bucket, &conn);
    for entry in listed {
        let cached = pages.get(&entry.url);
        if !full && cached.is_some_and(|p| p.lastmod == entry.lastmod) {
            continue;
        }
        let Some(page) = client.page(&entry).await? else {
            tracing::warn!(url = %entry.url, "an AMF decision page serves no file");
            continue;
        };
        if cached.is_none_or(|c| c.file != page.file) {
            let Some(bytes) = client.file(&page.file).await? else {
                tracing::warn!(url = %entry.url, file = %page.file, "an AMF decision page links a file the site does not serve");
                continue;
            };
            bucket.put(&key::amf(&page.number), bytes).await?;
        }
        if cached != Some(&page) {
            append_page(&dir, &page)?;
        }
        writer.push(&page).await?;
        pages.insert(entry.url, page);
    }
    writer.finish().await
}

struct Writer<'a> {
    bucket: Bucket,
    conn: &'a Connection,
    batch: Vec<Candidate>,
    counts: IngestCounts,
    expired: usize,
    without_text: usize,
}

impl<'a> Writer<'a> {
    fn new(bucket: &Bucket, conn: &'a Connection) -> Self {
        Self {
            bucket: bucket.clone(),
            conn,
            batch: Vec::new(),
            counts: IngestCounts::default(),
            expired: 0,
            without_text: 0,
        }
    }

    /// A page whose file is gone from the bucket serves the version its naming period ended.
    async fn push(&mut self, page: &AmfPage) -> Result<()> {
        let file = key::amf(&page.number);
        let Some(bytes) = self.bucket.get(&file).await? else {
            self.expired += 1;
            return Ok(());
        };
        let Some(raw) = lj_sources::pdf::pdftotext_extract(&bytes)
            .ok()
            .filter(|raw| pdf_has_text_layer(raw))
        else {
            tracing::warn!(number = %page.number, "an AMF decision file has no text layer");
            self.without_text += 1;
            return Ok(());
        };
        let text = reflow_page_text(&raw);
        let uid = format!("amf/{}", page.number);
        let source_fields = amf_source_fields(
            serde_json::to_value(page)?,
            &file,
            amf_named_until(&page.title, &text, &page.file),
        );
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
            drain_batch(self.conn, batch, IngestMode::MissingHash, &mut self.counts).await?;
        }
        Ok(())
    }

    async fn finish(mut self) -> Result<()> {
        if !self.batch.is_empty() {
            let batch = std::mem::take(&mut self.batch);
            drain_batch(self.conn, batch, IngestMode::MissingHash, &mut self.counts).await?;
        }
        tracing::info!(
            created = self.counts.created,
            updated = self.counts.updated,
            skipped = self.counts.skipped,
            expired = self.expired,
            without_text = self.without_text,
            "amf"
        );
        Ok(())
    }
}
