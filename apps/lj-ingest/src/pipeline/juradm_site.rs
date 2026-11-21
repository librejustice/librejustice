//! The decisions the administrative courts publish on their own sites, from the PDFs they link.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use lj_bucket::{key, Bucket};
use lj_core::decision::Decision;
use lj_parse::{
    juradm_decision, juradm_site_source_fields, parse_juradm_head, reflow_page_text,
    FromSourceFields, JuradmCourt, JuradmRefused, JURADM_COURTS,
};
use lj_pg::db::Connection;
use lj_sources::juradm_site::{
    append_item, file_id, read_items, Fetched, FileRead, ItemRead, JuradmClient,
};
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

use super::batch::drain_batch;
use super::cnda::pdf_has_text_layer;
use super::europe::europe_pool;
use super::{
    content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode, BATCH_SIZE,
};

/// Every listing page, then the items not read yet, none started past `minutes`; `full` writes the
/// items read before again and deletes the provenances their files no longer give.
pub async fn sync_juradm_site(full: bool, courts: &[String], minutes: Option<u64>) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.paths().juradm_site();
    let bucket = settings.bucket()?;
    let client = JuradmClient::new(&settings.egress_proxy)?;
    let courts: Vec<&JuradmCourt> = if courts.is_empty() {
        JURADM_COURTS.iter().collect()
    } else {
        courts
            .iter()
            .map(|code| lj_parse::juradm_court(code).ok_or_else(|| anyhow!("no court site {code}")))
            .collect::<Result<_>>()?
    };
    let deadline = minutes.map(|m| Instant::now() + Duration::from_secs(m * 60));
    let mut items = read_items(&dir)?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let mut writer = Writer::new(&bucket, &conn, &dir);
    let mut left = 0usize;
    for court in courts {
        let mut listed = Vec::new();
        for page in 1.. {
            let items = client.listing(court, page).await?;
            if items.is_empty() {
                break;
            }
            listed.extend(items);
        }
        tracing::info!(
            court = court.code,
            items = listed.len(),
            "juradm-site listing"
        );
        for entry in listed {
            if let Some(read) = items.get(&entry.url) {
                if full {
                    for file in &read.files {
                        if let Some(kept) = &file.key {
                            let bytes = bucket
                                .get(kept)
                                .await?
                                .ok_or_else(|| anyhow!("{kept} is gone from the bucket"))?;
                            let (format, raw) =
                                file_text(&bytes).expect("a kept file is a PDF or a Word document");
                            writer
                                .push(&read.url, &file.url, kept, format, &bytes, &raw)
                                .await?;
                        }
                    }
                }
                continue;
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                left += 1;
                continue;
            }
            let mut read = ItemRead {
                court: court.code.to_string(),
                url: entry.url.clone(),
                title: entry.title.clone(),
                date: entry.date.clone(),
                files: Vec::new(),
            };
            let Some(urls) = client.files(court, &entry).await? else {
                tracing::warn!(item = %entry.url, "juradm-site: an item's page is not served");
                continue;
            };
            let mut whole = true;
            for link in urls {
                let (url, key) = match client.file(&link).await? {
                    Fetched::Served { url, bytes } => {
                        let key = writer.read(court, &entry.url, &url, &bytes).await?;
                        (url, key)
                    }
                    Fetched::Gone => {
                        tracing::warn!(item = %entry.url, file = %link, "juradm-site: a file the item links is gone");
                        (link, None)
                    }
                    Fetched::Failing => {
                        tracing::warn!(item = %entry.url, file = %link, "juradm-site: a file the item links is not served");
                        whole = false;
                        continue;
                    }
                };
                read.files.push(FileRead { url, key });
            }
            if whole {
                items.insert(read.url.clone(), read.clone());
                writer.pending.push(read);
            }
        }
        writer.drain().await?;
    }
    if left > 0 {
        tracing::info!(left, "juradm-site: items left for the next run");
    }
    writer.finish(full).await
}

struct Writer<'a> {
    bucket: Bucket,
    conn: &'a Connection,
    dir: &'a Path,
    batch: Vec<Candidate>,
    /// Items read whole, kept in the cache once their decisions are written.
    pending: Vec<ItemRead>,
    /// The `source_uid` each file read gives, by its bucket key, `None` when it gives none.
    given: HashMap<String, Option<String>>,
    counts: IngestCounts,
    refused: HashMap<&'static str, usize>,
}

impl<'a> Writer<'a> {
    fn new(bucket: &Bucket, conn: &'a Connection, dir: &'a Path) -> Self {
        Self {
            bucket: bucket.clone(),
            conn,
            dir,
            batch: Vec::new(),
            pending: Vec::new(),
            given: HashMap::new(),
            counts: IngestCounts::default(),
            refused: HashMap::new(),
        }
    }

    /// Keeps in the bucket a PDF or a Word document that holds a decision or a scan, and writes
    /// the decision.
    async fn read(
        &mut self,
        court: &JuradmCourt,
        page: &str,
        url: &str,
        bytes: &[u8],
    ) -> Result<Option<String>> {
        let Some((format, raw)) = file_text(bytes) else {
            *self.refused.entry("not_a_document").or_default() += 1;
            return Ok(None);
        };
        if pdf_has_text_layer(&raw) && juradm_decision(&raw).is_none() {
            *self.refused.entry("not_a_decision").or_default() += 1;
            return Ok(None);
        }
        let Some(id) = file_id(url) else {
            tracing::warn!(
                page,
                file = url,
                "juradm-site: a file outside /content/download/"
            );
            *self.refused.entry("no_file_id").or_default() += 1;
            return Ok(None);
        };
        let file = key::juradm_site(court.code, id, format);
        self.bucket.put(&file, bytes.to_vec()).await?;
        self.push(page, url, &file, format, bytes, &raw).await?;
        Ok(Some(file))
    }

    async fn push(
        &mut self,
        page: &str,
        url: &str,
        file: &str,
        format: &'static str,
        bytes: &[u8],
        raw: &str,
    ) -> Result<()> {
        self.given.insert(file.to_string(), None);
        if !pdf_has_text_layer(raw) {
            let why = if format == "pdf" {
                "scan"
            } else {
                "unreadable"
            };
            *self.refused.entry(why).or_default() += 1;
            return Ok(());
        }
        let head = match parse_juradm_head(raw) {
            Ok(head) => head,
            Err(why) => {
                tracing::warn!(
                    page,
                    file = url,
                    ?why,
                    "juradm-site: a decision whose head is not read"
                );
                *self.refused.entry(refused_label(why)).or_default() += 1;
                return Ok(());
            }
        };
        let uid = format!(
            "juradm-site/{}/{}/{}",
            head.court.code, head.numbers[0], head.decision_date
        );
        self.given.insert(file.to_string(), Some(uid.clone()));
        let decision = juradm_decision(raw).expect("a head read is a decision's");
        let text = match format {
            "pdf" => reflow_page_text(decision),
            _ => decision.to_string(),
        };
        let source_fields = juradm_site_source_fields(page, url, file, &head);
        self.batch.push(Candidate {
            decision_id: None,
            public_id: generate_public_id(),
            content_checksum: content_checksum(bytes),
            decision: Decision::from_source_fields(&text, &source_fields, &uid),
            prebuilt_source_fields: Some(source_fields),
            prebuilt_extracted: None,
            raw_payload: Vec::new(),
            payload_format: format.to_string(),
            write_mode: super::WriteMode::Full,
            passages: true,
            distinct: false,
        });
        if self.batch.len() >= BATCH_SIZE {
            self.drain().await?;
        }
        Ok(())
    }

    async fn drain(&mut self) -> Result<()> {
        let batch = std::mem::take(&mut self.batch);
        drain_batch(self.conn, batch, IngestMode::MissingHash, &mut self.counts).await?;
        for item in self.pending.drain(..) {
            append_item(self.dir, &item)?;
        }
        Ok(())
    }

    /// Deletes the provenances a file read gave under another `source_uid`, or gives no more.
    async fn sweep(&self) -> Result<usize> {
        let repo = DecisionRepository::new(self.conn);
        let mut swept = 0;
        for (uid, file) in repo.juradm_site_files().await? {
            if self
                .given
                .get(&file)
                .is_some_and(|given| given.as_deref() != Some(uid.as_str()))
            {
                tracing::warn!(
                    uid,
                    file,
                    "juradm-site: a provenance its file no longer gives, deleted"
                );
                repo.delete_provenance(&uid).await?;
                swept += 1;
            }
        }
        Ok(swept)
    }

    async fn finish(mut self, full: bool) -> Result<()> {
        self.drain().await?;
        let swept = if full { self.sweep().await? } else { 0 };
        tracing::info!(
            created = self.counts.created,
            updated = self.counts.updated,
            skipped = self.counts.skipped,
            swept,
            refused = ?self.refused,
            "juradm-site"
        );
        Ok(())
    }
}

/// The format of a PDF or a Word document and its text, a Word document's one paragraph a line;
/// a file a reader fails on reads as empty, as a scan does.
pub(super) fn file_text(bytes: &[u8]) -> Option<(&'static str, String)> {
    if bytes.starts_with(b"%PDF") {
        return Some((
            "pdf",
            lj_sources::pdf::pdftotext_extract(bytes).unwrap_or_default(),
        ));
    }
    let format = match lj_office::Format::of(bytes).ok()? {
        lj_office::Format::Doc => "doc",
        lj_office::Format::Docx => "docx",
        lj_office::Format::Odt => "odt",
        lj_office::Format::Html => return None,
    };
    let text = lj_office::open(bytes)
        .map(|document| document.text())
        .unwrap_or_default();
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    Some((format, lines.join("\n")))
}

fn refused_label(why: JuradmRefused) -> &'static str {
    match why {
        JuradmRefused::NotADecision => "not_a_decision",
        JuradmRefused::UnknownCourt => "unknown_court",
        JuradmRefused::NoDocket => "no_docket",
        JuradmRefused::NoDate => "no_date",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_copy_takes_the_address_of_the_registry_copy() {
        for (court, number, date, address) in [
            (
                "ta_cergy_pontoise",
                "2623718",
                "2026-10-30",
                "tribunal administratif de cergy pontoise|2623718|2026-10-30",
            ),
            (
                "ta_amiens",
                "2201345",
                "2024-04-18",
                "tribunal administratif d amiens|2201345|2024-04-18",
            ),
            (
                "ta_saint_barthelemy",
                "2500012",
                "2025-05-02",
                "tribunal administratif de st barthelemy|2500012|2025-05-02",
            ),
            (
                "caa_lyon",
                "25LY01973",
                "2025-09-11",
                "caa|25ly01973|2025-09-11",
            ),
        ] {
            let head = lj_parse::JuradmHead {
                court: lj_parse::juradm_court(court).unwrap(),
                numbers: vec![number.to_string()],
                kind: "Décision",
                decision_date: date.to_string(),
                audience_date: None,
            };
            let fields = juradm_site_source_fields("page", "pdf", "file", &head);
            let uid = format!("juradm-site/{court}/{number}/{date}");
            let decision = Decision::from_source_fields("Vu la procédure suivante", &fields, &uid);
            assert_eq!(
                lj_extract_rows::canonical_ref(&decision).as_deref(),
                Some(address)
            );
        }
    }
}
