//! Rewrites of the article texts and text bodies an unchanged source now reads
//! differently, with the citation spans that index into them.

use std::collections::HashSet;

use chrono::NaiveDate;
use lj_core::normalizer::normalize_chars;

use super::types::{TextCaseCitationRow, TextLegalCitationRow};
use super::DecisionRepository;
use crate::error::Result;

/// An article as its source member reads it.
pub struct ArticleTextRead {
    pub source_uid: String,
    pub content_checksum: u64,
    pub texte: Option<String>,
    pub nota: Option<String>,
}

/// A stored version whose unchanged member reads differently, with the text and
/// nota to write, normalised as the upsert writes them.
pub struct ArticleTextDrift {
    pub text_uid: String,
    pub num_key: String,
    pub date_debut: NaiveDate,
    pub content_checksum: u64,
    pub texte: Option<String>,
    pub nota: Option<String>,
}

/// A drifted version to write, with the spans of its new text.
pub struct ArticleTextWrite {
    pub drift: ArticleTextDrift,
    pub cases: Vec<TextCaseCitationRow>,
    pub cites: Vec<TextLegalCitationRow>,
}

/// A body to write over the one read, with the spans of the new body.
pub struct BodyTextWrite {
    pub text_uid: String,
    pub stored: String,
    pub body: String,
    pub cases: Vec<TextCaseCitationRow>,
    pub cites: Vec<TextLegalCitationRow>,
}

/// A span owner: an article version, or a body when both keys are `None`.
type Owner<'a> = (&'a str, Option<&'a str>, Option<NaiveDate>);

impl DecisionRepository<'_> {
    /// The stored versions `reads` would change: same `source_uid`, same
    /// checksum, and a text or nota that differs once normalised.
    #[tracing::instrument(name = "db.article_text_drift", skip_all, fields(db.system = "postgresql"))]
    pub async fn article_text_drift(
        &self,
        reads: &[ArticleTextRead],
    ) -> Result<Vec<ArticleTextDrift>> {
        let uids: Vec<&str> = reads.iter().map(|r| r.source_uid.as_str()).collect();
        let checksums: Vec<i64> = reads
            .iter()
            .map(|r| i64::from_ne_bytes(r.content_checksum.to_ne_bytes()))
            .collect();
        let textes: Vec<Option<String>> = reads
            .iter()
            .map(|r| r.texte.as_deref().map(normalize_chars))
            .collect();
        let notas: Vec<Option<String>> = reads
            .iter()
            .map(|r| r.nota.as_deref().map(normalize_chars))
            .collect();
        let rows = self
            .conn
            .query(
                "SELECT a.text_uid, a.num_key, a.date_debut, v.i
                 FROM unnest($1::text[], $2::bigint[], $3::text[], $4::text[])
                      WITH ORDINALITY AS v(source_uid, checksum, texte, nota, i)
                 JOIN legal_article a
                   ON a.source_uid = v.source_uid AND a.content_checksum = v.checksum
                 WHERE a.texte IS DISTINCT FROM v.texte OR a.nota IS DISTINCT FROM v.nota",
                &[&uids, &checksums, &textes, &notas],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let i = r.get::<_, i64>(3) as usize - 1;
                ArticleTextDrift {
                    text_uid: r.get(0),
                    num_key: r.get(1),
                    date_debut: r.get(2),
                    content_checksum: reads[i].content_checksum,
                    texte: textes[i].clone(),
                    nota: notas[i].clone(),
                }
            })
            .collect())
    }

    /// Writes the drifted versions and the spans of those written, in one
    /// transaction; a version changed since it was read is left alone.
    #[tracing::instrument(name = "db.rewrite_article_texts", skip_all, fields(db.system = "postgresql"))]
    pub async fn rewrite_article_texts(&self, writes: &[ArticleTextWrite]) -> Result<u64> {
        let uids: Vec<&str> = writes.iter().map(|w| w.drift.text_uid.as_str()).collect();
        let nums: Vec<&str> = writes.iter().map(|w| w.drift.num_key.as_str()).collect();
        let dates: Vec<NaiveDate> = writes.iter().map(|w| w.drift.date_debut).collect();
        let checksums: Vec<i64> = writes
            .iter()
            .map(|w| i64::from_ne_bytes(w.drift.content_checksum.to_ne_bytes()))
            .collect();
        let textes: Vec<Option<&str>> = writes.iter().map(|w| w.drift.texte.as_deref()).collect();
        let notas: Vec<Option<&str>> = writes.iter().map(|w| w.drift.nota.as_deref()).collect();
        self.conn.batch_execute("BEGIN").await?;
        let done: Result<u64> = async {
            let rows = self
                .conn
                .query(
                    "UPDATE legal_article a SET texte = v.texte, nota = v.nota
                     FROM unnest($1::text[], $2::text[], $3::date[], $4::bigint[],
                                 $5::text[], $6::text[])
                          AS v(text_uid, num_key, date_debut, checksum, texte, nota)
                     WHERE (a.text_uid, a.num_key, a.date_debut)
                           = (v.text_uid, v.num_key, v.date_debut)
                       AND a.content_checksum = v.checksum
                       AND (a.texte IS DISTINCT FROM v.texte OR a.nota IS DISTINCT FROM v.nota)
                     RETURNING a.text_uid, a.num_key, a.date_debut",
                    &[&uids, &nums, &dates, &checksums, &textes, &notas],
                )
                .await?;
            let written: HashSet<(String, String, NaiveDate)> = rows
                .iter()
                .map(|r| (r.get(0), r.get(1), r.get(2)))
                .collect();
            let kept: Vec<&ArticleTextWrite> = writes
                .iter()
                .filter(|w| {
                    let d = &w.drift;
                    written.contains(&(d.text_uid.clone(), d.num_key.clone(), d.date_debut))
                })
                .collect();
            let owners: Vec<Owner> = kept
                .iter()
                .map(|w| {
                    let d = &w.drift;
                    (
                        d.text_uid.as_str(),
                        Some(d.num_key.as_str()),
                        Some(d.date_debut),
                    )
                })
                .collect();
            let cases: Vec<(&str, &TextCaseCitationRow)> = kept
                .iter()
                .flat_map(|w| w.cases.iter().map(|r| (w.drift.text_uid.as_str(), r)))
                .collect();
            let cites: Vec<(&str, &TextLegalCitationRow)> = kept
                .iter()
                .flat_map(|w| w.cites.iter().map(|r| (w.drift.text_uid.as_str(), r)))
                .collect();
            self.replace_owner_spans(&owners, &cases, &cites).await?;
            Ok(written.len() as u64)
        }
        .await;
        self.end_transaction(done).await
    }

    /// `text_uid` of the texts under `prefix` that carry a body.
    pub async fn legal_text_uids_with_body(&self, prefix: &str) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid FROM legal_text
                 WHERE body IS NOT NULL AND starts_with(text_uid, $1)",
                &[&prefix],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// `(text_uid, body)` of those of `uids` that carry a body.
    pub async fn legal_text_bodies_of(&self, uids: &[String]) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, body FROM legal_text
                 WHERE text_uid = ANY($1) AND body IS NOT NULL",
                &[&uids],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Writes each body over the one read and the spans of those written, in one
    /// transaction; a body changed since it was read is left alone.
    #[tracing::instrument(name = "db.rewrite_legal_text_bodies", skip_all, fields(db.system = "postgresql"))]
    pub async fn rewrite_legal_text_bodies(&self, writes: &[BodyTextWrite]) -> Result<u64> {
        let uids: Vec<&str> = writes.iter().map(|w| w.text_uid.as_str()).collect();
        let stored: Vec<&str> = writes.iter().map(|w| w.stored.as_str()).collect();
        let bodies: Vec<&str> = writes.iter().map(|w| w.body.as_str()).collect();
        self.conn.batch_execute("BEGIN").await?;
        let done: Result<u64> = async {
            let rows = self
                .conn
                .query(
                    "UPDATE legal_text t SET body = v.body
                     FROM unnest($1::text[], $2::text[], $3::text[]) AS v(text_uid, stored, body)
                     WHERE t.text_uid = v.text_uid AND t.body = v.stored
                     RETURNING t.text_uid",
                    &[&uids, &stored, &bodies],
                )
                .await?;
            let written: HashSet<String> = rows.iter().map(|r| r.get(0)).collect();
            let kept: Vec<&BodyTextWrite> = writes
                .iter()
                .filter(|w| written.contains(&w.text_uid))
                .collect();
            let owners: Vec<Owner> = kept
                .iter()
                .map(|w| (w.text_uid.as_str(), None, None))
                .collect();
            let cases: Vec<(&str, &TextCaseCitationRow)> = kept
                .iter()
                .flat_map(|w| w.cases.iter().map(|r| (w.text_uid.as_str(), r)))
                .collect();
            let cites: Vec<(&str, &TextLegalCitationRow)> = kept
                .iter()
                .flat_map(|w| w.cites.iter().map(|r| (w.text_uid.as_str(), r)))
                .collect();
            self.replace_owner_spans(&owners, &cases, &cites).await?;
            Ok(written.len() as u64)
        }
        .await;
        self.end_transaction(done).await
    }

    /// Replaces the spans of each owner, matched on the keys the unique span
    /// index folds, so the delete walks it.
    async fn replace_owner_spans(
        &self,
        owners: &[Owner<'_>],
        cases: &[(&str, &TextCaseCitationRow)],
        cites: &[(&str, &TextLegalCitationRow)],
    ) -> Result<()> {
        let uids: Vec<&str> = owners.iter().map(|o| o.0).collect();
        let nums: Vec<Option<&str>> = owners.iter().map(|o| o.1).collect();
        let dates: Vec<Option<NaiveDate>> = owners.iter().map(|o| o.2).collect();
        self.conn
            .execute(
                "DELETE FROM text_case_citation c
                 USING unnest($1::text[], $2::text[], $3::date[]) AS o(text_uid, num_key, date_debut)
                 WHERE c.owner_text_uid = o.text_uid
                   AND COALESCE(c.owner_num_key, '') = COALESCE(o.num_key, '')
                   AND COALESCE(c.owner_date_debut, DATE '0001-01-01')
                       = COALESCE(o.date_debut, DATE '0001-01-01')",
                &[&uids, &nums, &dates],
            )
            .await?;
        self.conn
            .execute(
                "DELETE FROM text_legal_citation c
                 USING unnest($1::text[], $2::text[], $3::date[]) AS o(text_uid, num_key, date_debut)
                 WHERE c.owner_text_uid = o.text_uid
                   AND COALESCE(c.owner_num_key, '') = COALESCE(o.num_key, '')
                   AND COALESCE(c.owner_date_debut, DATE '0001-01-01')
                       = COALESCE(o.date_debut, DATE '0001-01-01')",
                &[&uids, &nums, &dates],
            )
            .await?;
        self.copy_text_case_citations(cases).await?;
        self.copy_text_legal_citations(cites).await
    }

    async fn end_transaction(&self, done: Result<u64>) -> Result<u64> {
        match done {
            Ok(n) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(n)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }
}
