//! `parliament_document`, a chamber's documents under their native ids.

use std::collections::HashMap;

use chrono::NaiveDate;

use super::DecisionRepository;
use crate::error::Result;

pub struct ParliamentDocumentRow {
    pub uid: String,
    pub chamber: String,
    pub kind: String,
    pub legislature: Option<i16>,
    pub number: Option<String>,
    pub title: Option<String>,
    pub analysis: Option<String>,
    pub authors: serde_json::Value,
    pub ministry: Option<String>,
    pub dated: Option<NaiveDate>,
    pub printings: serde_json::Value,
    pub url: Option<String>,
    pub fate: Option<String>,
    pub text_uid: Option<String>,
    pub source: String,
    pub source_uid: String,
    pub content_checksum: u64,
}

impl DecisionRepository<'_> {
    /// The checksum of each document `source` wrote, by uid.
    pub async fn parliament_checksums(&self, source: &str) -> Result<HashMap<String, u64>> {
        let rows = self
            .conn
            .query(
                "SELECT uid, content_checksum FROM parliament_document WHERE source = $1",
                &[&source],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let checksum: i64 = r.get(1);
                (r.get(0), u64::from_ne_bytes(checksum.to_ne_bytes()))
            })
            .collect())
    }

    pub async fn upsert_parliament_document(&self, row: &ParliamentDocumentRow) -> Result<()> {
        let checksum = i64::from_ne_bytes(row.content_checksum.to_ne_bytes());
        self.conn
            .execute(
                "INSERT INTO parliament_document (uid, chamber, kind, legislature, number, title,
                     analysis, authors, ministry, dated, printings, url, fate, text_uid, source,
                     source_uid, content_checksum)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
                 ON CONFLICT (uid) DO UPDATE SET
                     chamber = EXCLUDED.chamber, kind = EXCLUDED.kind,
                     legislature = EXCLUDED.legislature, number = EXCLUDED.number,
                     title = EXCLUDED.title, analysis = EXCLUDED.analysis,
                     authors = EXCLUDED.authors, ministry = EXCLUDED.ministry,
                     dated = EXCLUDED.dated, printings = EXCLUDED.printings, url = EXCLUDED.url,
                     fate = EXCLUDED.fate, text_uid = EXCLUDED.text_uid, source = EXCLUDED.source,
                     source_uid = EXCLUDED.source_uid, content_checksum = EXCLUDED.content_checksum,
                     updated_at = now()
                 WHERE parliament_document.content_checksum <> EXCLUDED.content_checksum",
                &[
                    &row.uid,
                    &row.chamber,
                    &row.kind,
                    &row.legislature,
                    &row.number,
                    &row.title,
                    &row.analysis,
                    &row.authors,
                    &row.ministry,
                    &row.dated,
                    &row.printings,
                    &row.url,
                    &row.fate,
                    &row.text_uid,
                    &row.source,
                    &row.source_uid,
                    &checksum,
                ],
            )
            .await?;
        Ok(())
    }
}
