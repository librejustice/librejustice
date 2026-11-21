//! `norm_visit`, what an official publisher served at the last visit of each of its items, and
//! the stored versions of a text the visits let a run rebuild without reading its states again.

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, Utc};

use super::DecisionRepository;
use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormVisit {
    pub visited_at: DateTime<Utc>,
    pub content_checksum: u64,
}

/// One stored version of an article, as `legal_article` holds it.
pub struct StoredArticleVersion {
    pub num: String,
    pub position: Option<i32>,
    pub title_path: Option<String>,
    pub nota: Option<String>,
    pub texte: String,
    pub date_debut: NaiveDate,
    pub date_fin: Option<NaiveDate>,
}

impl DecisionRepository<'_> {
    /// Every item of `source` visited, by item.
    pub async fn norm_visits(&self, source: &str) -> Result<HashMap<String, NormVisit>> {
        let rows = self
            .conn
            .query(
                "SELECT item, visited_at, content_checksum FROM norm_visit WHERE source = $1",
                &[&source],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let checksum: i64 = r.get(2);
                (
                    r.get(0),
                    NormVisit {
                        visited_at: r.get(1),
                        content_checksum: u64::from_ne_bytes(checksum.to_ne_bytes()),
                    },
                )
            })
            .collect())
    }

    /// Records the items of `source` read now, each with the checksum of its payload.
    pub async fn put_norm_visits(&self, source: &str, read: &[(String, u64)]) -> Result<()> {
        let items: Vec<&str> = read.iter().map(|(item, _)| item.as_str()).collect();
        let checksums: Vec<i64> = read
            .iter()
            .map(|(_, c)| i64::from_ne_bytes(c.to_ne_bytes()))
            .collect();
        self.conn
            .execute(
                "INSERT INTO norm_visit (source, item, content_checksum)
                 SELECT $1, item, checksum FROM unnest($2::text[], $3::bigint[]) AS v(item, checksum)
                 ON CONFLICT (source, item) DO UPDATE
                 SET visited_at = now(), content_checksum = EXCLUDED.content_checksum",
                &[&source, &items, &checksums],
            )
            .await?;
        Ok(())
    }

    /// The versions of `text_uid` that have a body: row, article, first day they no longer apply.
    pub async fn article_version_ends(
        &self,
        text_uid: &str,
    ) -> Result<Vec<(i64, String, Option<NaiveDate>)>> {
        let rows = self
            .conn
            .query(
                "SELECT id, num, date_fin FROM legal_article
                 WHERE text_uid = $1 AND texte IS NOT NULL",
                &[&text_uid],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// Every version of the articles of `text_uid` that has a body, each article's oldest first.
    pub async fn stored_article_versions(
        &self,
        text_uid: &str,
    ) -> Result<Vec<StoredArticleVersion>> {
        let rows = self
            .conn
            .query(
                "SELECT num, position, title_path, nota, texte, date_debut, date_fin
                 FROM legal_article
                 WHERE text_uid = $1 AND texte IS NOT NULL
                 ORDER BY num_key, date_debut",
                &[&text_uid],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| StoredArticleVersion {
                num: r.get(0),
                position: r.get(1),
                title_path: r.get(2),
                nota: r.get(3),
                texte: r.get(4),
                date_debut: r.get(5),
                date_fin: r.get(6),
            })
            .collect())
    }
}
