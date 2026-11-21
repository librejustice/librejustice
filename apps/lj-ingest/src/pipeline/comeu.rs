//! The European Commission's competition decisions, from its case search.

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use chrono::{DateTime, Datelike, Duration, Utc};
use lj_core::decision::Decision;
use lj_parse::{reflow_page_text, FromSourceFields, COMEU_ADOPTION_DATE};
use lj_pg::db::Connection;
use lj_sources::comeu::{ComeuClient, Record, ATTACHMENT, DECISION, INSTRUMENTS};
use lj_store::repository::DecisionRepository;
use lj_vocab::languages::{eu_of_iso_639_1, EU_OFFICIAL_LANGUAGES};
use serde_json::{json, Map, Value};

use super::batch::drain_batch;
use super::cnda::pdf_has_text_layer;
use super::europe::europe_pool;
use super::{
    content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode, WriteMode,
    BATCH_SIZE,
};

/// The days of documents the daily run asks for, so that a missed run is caught up.
const WINDOW_DAYS: i64 = 7;

/// Writes the decisions of the documents published in the last week, of every document with `full`,
/// in their original language, French and English.
pub async fn sync_comeu(full: bool) -> Result<()> {
    super::europe::write_europe_names().await?;
    let client = ComeuClient::new()?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // An antitrust decision cites hundreds of CJUE cases: resolving a batch's citations ran past 30 s.
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let now = Utc::now();
    let mut writer = Writer {
        client: &client,
        conn: &conn,
        batch: Vec::new(),
        counts: IngestCounts::default(),
        without_text: 0,
        unread: 0,
        orphans: 0,
    };
    for instrument in INSTRUMENTS {
        let (decisions, documents) = if full {
            (
                client.list(instrument, DECISION, now.year()).await?,
                client.list(instrument, ATTACHMENT, now.year()).await?,
            )
        } else {
            let since = (now - Duration::days(WINDOW_DAYS)).format("%Y-%m-%d");
            let mut cases: Vec<String> = client
                .attachments_since(instrument, &since.to_string())
                .await?
                .iter()
                .filter_map(|d| d.first("caseNumber").map(str::to_string))
                .collect();
            cases.sort_unstable();
            cases.dedup();
            let mut records = Vec::new();
            for case in &cases {
                records.extend(client.of_case(case).await?);
            }
            records
                .into_iter()
                .partition(|r| r.first("metadataType") == Some(DECISION))
        };
        writer.instrument(instrument, decisions, documents).await?;
    }
    writer.finish(full).await
}

struct Writer<'a> {
    client: &'a ComeuClient,
    conn: &'a Connection,
    batch: Vec<Candidate>,
    counts: IngestCounts,
    without_text: usize,
    unread: usize,
    orphans: usize,
}

impl Writer<'_> {
    async fn instrument(
        &mut self,
        instrument: &str,
        decisions: Vec<Record>,
        documents: Vec<Record>,
    ) -> Result<()> {
        let mut by_decision: HashMap<String, Vec<Record>> = HashMap::new();
        for document in documents {
            let (decision, _) = document.reference.rsplit_once("-ATT").ok_or_else(|| {
                anyhow!("comeu document {} names no decision", document.reference)
            })?;
            by_decision
                .entry(decision.to_string())
                .or_default()
                .push(document);
        }
        let mut dated = Vec::new();
        for decision in decisions {
            if let Some(documents) = by_decision.remove(&decision.reference) {
                dated.push((adoption_date(&decision)?, decision, documents));
            }
        }
        self.orphans += by_decision.values().map(Vec::len).sum::<usize>();
        dated.sort_by(|a, b| b.0.cmp(&a.0));
        for (date, decision, documents) in dated {
            self.decision(instrument, date.as_deref(), &decision, documents)
                .await?;
        }
        Ok(())
    }

    async fn decision(
        &mut self,
        instrument: &str,
        date: Option<&str>,
        decision: &Record,
        mut documents: Vec<Record>,
    ) -> Result<()> {
        documents.sort_by_cached_key(|d| (attachment_number(&d.reference), d.reference.clone()));
        documents.dedup_by(|a, b| a.reference == b.reference);
        let mut languages: Vec<(&'static str, Vec<Record>)> = Vec::new();
        for document in documents {
            let Some(lang) = language(&document) else {
                self.unread += 1;
                continue;
            };
            match languages.iter_mut().find(|(l, _)| *l == lang) {
                Some((_, same)) => same.push(document),
                None => languages.push((lang, vec![document])),
            }
        }
        let Some(&(original, _)) = languages.first() else {
            return Ok(());
        };
        languages.retain(|(lang, _)| [original, "fr", "en"].contains(lang));
        let uid = |lang: &str| format!("comeu/{}/{lang}", decision.reference);
        let uids: Vec<String> = languages.iter().map(|(l, _)| uid(l)).collect();
        let held = DecisionRepository::new(self.conn)
            .find_ingest_states(&uids)
            .await?;
        let mut read: Vec<(&'static str, Vec<Record>, String, String)> = Vec::new();
        for (lang, documents) in languages {
            let checksum = checksum(decision, &documents)?;
            if held
                .get(&uid(lang))
                .is_some_and(|s| s.content_checksum == checksum)
            {
                self.counts.skipped += 1;
                continue;
            }
            let mut parts = Vec::new();
            for document in &documents {
                let link = document
                    .first("attachmentLink")
                    .ok_or_else(|| anyhow!("comeu document {} has no link", document.reference))?;
                let Some(bytes) = self.client.pdf(instrument, link).await? else {
                    self.unread += 1;
                    continue;
                };
                match lj_sources::pdf::pdftotext_extract(&bytes)
                    .ok()
                    .filter(|raw| pdf_has_text_layer(raw))
                {
                    Some(raw) => parts.push(reflow_page_text(&raw)),
                    None => self.without_text += 1,
                }
            }
            if !parts.is_empty() {
                read.push((lang, documents, checksum, parts.join("\n\n")));
            }
        }
        for (lang, documents, checksum, text) in read {
            let fields = source_fields(decision, &documents, lang, original, date);
            let uid = uid(lang);
            self.batch.push(Candidate {
                decision_id: None,
                public_id: generate_public_id(),
                content_checksum: checksum,
                decision: Decision::from_source_fields(&text, &fields, &uid),
                prebuilt_source_fields: Some(fields),
                prebuilt_extracted: None,
                raw_payload: Vec::new(),
                payload_format: "pdf".to_string(),
                write_mode: WriteMode::Full,
                passages: true,
                distinct: false,
            });
        }
        if self.batch.len() >= BATCH_SIZE {
            let batch = std::mem::take(&mut self.batch);
            drain_batch(self.conn, batch, IngestMode::MissingHash, &mut self.counts).await?;
        }
        Ok(())
    }

    async fn finish(mut self, full: bool) -> Result<()> {
        if !self.batch.is_empty() {
            let batch = std::mem::take(&mut self.batch);
            drain_batch(self.conn, batch, IngestMode::MissingHash, &mut self.counts).await?;
        }
        if full && self.counts.created + self.counts.updated > 0 {
            tracing::warn!(
                created = self.counts.created,
                updated = self.counts.updated,
                "comeu: the full listing wrote texts the daily run had not"
            );
        }
        tracing::info!(
            created = self.counts.created,
            updated = self.counts.updated,
            skipped = self.counts.skipped,
            without_text = self.without_text,
            unread = self.unread,
            orphans = self.orphans,
            "comeu"
        );
        Ok(())
    }
}

/// The adoption day in Brussels: the search stores its midnight in UTC, 22:00 or 23:00 the day before.
fn adoption_date(decision: &Record) -> Result<Option<String>> {
    let Some(raw) = decision.first("decisionAdoptionDate") else {
        return Ok(None);
    };
    let at = DateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f%z")
        .map_err(|e| anyhow!("comeu {}: adoption date {raw}: {e}", decision.reference))?;
    Ok(Some(
        (at + Duration::hours(12))
            .date_naive()
            .format("%Y-%m-%d")
            .to_string(),
    ))
}

fn attachment_number(reference: &str) -> Option<u64> {
    reference.rsplit_once("-ATT")?.1.parse().ok()
}

/// The document's language, the search's own when the document names none of the Union's.
fn language(document: &Record) -> Option<&'static str> {
    let official = |code: &str| {
        let code = code.to_ascii_lowercase();
        EU_OFFICIAL_LANGUAGES.iter().find(|l| **l == code).copied()
    };
    document
        .first("attachmentLanguage")
        .and_then(official)
        .or_else(|| document.first("language").and_then(official))
}

fn checksum(decision: &Record, documents: &[Record]) -> Result<String> {
    let published: Vec<(&str, Option<&str>)> = documents
        .iter()
        .map(|d| {
            (
                d.reference.as_str(),
                d.first("attachmentPublicationBusinessDate"),
            )
        })
        .collect();
    Ok(content_checksum(&serde_json::to_vec(&json!([
        decision.metadata,
        published
    ]))?))
}

fn source_fields(
    decision: &Record,
    documents: &[Record],
    lang: &str,
    original: &str,
    date: Option<&str>,
) -> Value {
    let mut fields = Value::Object(decision.metadata.clone());
    fields["reference"] = json!(decision.reference);
    fields["attachments"] = documents
        .iter()
        .map(|d| {
            let mut own: Map<String, Value> = d
                .metadata
                .iter()
                .filter(|(k, _)| k.starts_with("attachment"))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            own.insert("reference".to_string(), json!(d.reference));
            Value::Object(own)
        })
        .collect();
    fields["lang"] = json!(eu_of_iso_639_1(lang));
    fields["original_lang"] = json!(eu_of_iso_639_1(original));
    if let Some(date) = date {
        fields[COMEU_ADOPTION_DATE] = json!(date);
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(reference: &str, metadata: Value) -> Record {
        Record {
            reference: reference.to_string(),
            metadata: metadata.as_object().unwrap().clone(),
        }
    }

    #[test]
    fn the_adoption_date_is_the_brussels_day() {
        let date = |raw: &str| {
            adoption_date(&record("M.1-DEC1", json!({"decisionAdoptionDate": [raw]}))).unwrap()
        };
        assert_eq!(
            date("2014-03-04T23:00:00.000+0000").as_deref(),
            Some("2014-03-05")
        );
        assert_eq!(
            date("2024-07-01T22:00:00.000+0000").as_deref(),
            Some("2024-07-02")
        );
        assert_eq!(
            date("2024-09-06T00:00:00.000+0000").as_deref(),
            Some("2024-09-06")
        );
        assert_eq!(
            adoption_date(&record("SA.1-DEC1", json!({}))).unwrap(),
            None
        );
    }

    #[test]
    fn a_document_without_an_official_language_takes_the_search_s() {
        let doc = |lang: &str| {
            record(
                "SA.1-DEC1-ATT2",
                json!({"attachmentLanguage": [lang], "language": ["en"]}),
            )
        };
        assert_eq!(language(&doc("DA")), Some("da"));
        assert_eq!(language(&doc("00")), Some("en"));
        assert_eq!(language(&doc("NS")), Some("en"));
        assert_eq!(
            attachment_number("AT.39984-DEC{2A76-E767}-ATT862"),
            Some(862)
        );
    }
}
