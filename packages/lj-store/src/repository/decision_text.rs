//! `decision_text`, one text per decision and language.

use super::support::now;
use super::DecisionRepository;
use crate::error::{Result, StoreError};
use serde_json::Value;

/// A decision text as its source member reads it.
pub struct DecisionTextRead {
    pub source_uid: String,
    pub content_checksum: String,
    pub full_text: String,
}

/// A stored read text its unchanged member reads differently.
pub struct DecisionTextDrift {
    pub text_id: i64,
    pub decision_id: i64,
    pub stored: String,
    pub full_text: String,
}

/// A stored text holding a character reference, with the checksum its
/// provenance was read at.
pub struct StoredText {
    pub text_id: i64,
    pub decision_id: i64,
    /// The text read of its decision.
    pub read: bool,
    pub source_uid: String,
    pub content_checksum: String,
    pub full_text: String,
}

/// A stored French text mostly in capitals.
pub struct CapitalText {
    pub text_id: i64,
    pub decision_id: i64,
    /// The text read of its decision, whose fields the extraction writes.
    pub extracted: bool,
    pub full_text: String,
}

/// The ISO 639-1 language of a provenance's text, from the code its ingester
/// writes in `source_fields[field]`. A source that names none is French.
pub(super) fn text_lang(source_fields: &Value, field: &str) -> Result<Option<&'static str>> {
    let Some(code) = source_fields.get(field).and_then(Value::as_str) else {
        return Ok(None);
    };
    lj_vocab::languages::iso_639_1_of_eu(&code.to_ascii_uppercase())
        .map(Some)
        .ok_or_else(|| {
            StoreError::Invalid(format!("source_fields.{field}: unknown language {code}"))
        })
}

/// The `annex_start` column of `text`.
fn annex_start(text: &str) -> Option<i32> {
    lj_core::annexes::start(text).map(|at| at as i32)
}

/// `(chunk_id, decision_id, char_start, char_end, zones)`.
pub type PassageSpan = (i64, i64, i32, i32, Vec<String>);

impl DecisionRepository<'_> {
    /// Writes a provenance's text in its language, when it is the first
    /// provenance of that language.
    pub(super) async fn put_text(
        &self,
        decision_id: i64,
        source_uid: &str,
        source_fields: &Value,
        text: &str,
    ) -> Result<()> {
        let lang = text_lang(source_fields, "lang")?.unwrap_or("fr");
        self.conn
            .execute(
                "SELECT decision_text_put($1, $2, $3, $4, $5)",
                &[&decision_id, &source_uid, &lang, &text, &annex_start(text)],
            )
            .await?;
        Ok(())
    }

    /// The read texts `reads` would change: written from the same active
    /// `source_uid` at the same checksum, and different. A decision extracted by
    /// a later version is left to it.
    #[tracing::instrument(name = "db.drifted_read_texts", skip_all, fields(db.system = "postgresql"))]
    pub async fn drifted_read_texts(
        &self,
        reads: &[DecisionTextRead],
    ) -> Result<Vec<DecisionTextDrift>> {
        let uids: Vec<&str> = reads.iter().map(|r| r.source_uid.as_str()).collect();
        let checksums: Vec<&str> = reads.iter().map(|r| r.content_checksum.as_str()).collect();
        let texts: Vec<&str> = reads.iter().map(|r| r.full_text.as_str()).collect();
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT t.id, t.decision_id, t.full_text, v.i
                 FROM unnest($1::text[], $2::text[], $3::text[])
                      WITH ORDINALITY AS v(source_uid, checksum, full_text, i)
                 JOIN decision_sources s ON s.source_uid = v.source_uid
                  AND s.content_checksum = v.checksum AND s.deleted_at IS NULL
                 JOIN decision_text t ON t.decision_id = s.decision_id
                  AND t.source_uid = s.source_uid AND {read}
                 JOIN decisions d ON d.id = t.decision_id
                 WHERE t.full_text <> v.full_text
                   AND (d.extract_version IS NULL OR d.extract_version <= $4)",
                    read = super::pick::read("t")
                ),
                &[&uids, &checksums, &texts, &lj_versions::EXTRACT_VERSION],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| DecisionTextDrift {
                text_id: r.get(0),
                decision_id: r.get(1),
                stored: r.get(2),
                full_text: reads[r.get::<_, i64>(3) as usize - 1].full_text.clone(),
            })
            .collect())
    }

    /// The texts of the provenances in `[from, to)` that hold a character reference or open on
    /// a line repeated within the next nine, `limit` of them after `after` in `source_uid` order.
    #[tracing::instrument(name = "db.texts_to_clean", skip(self), fields(db.system = "postgresql"))]
    pub async fn texts_to_clean(
        &self,
        from: &str,
        to: &str,
        after: &str,
        limit: i64,
    ) -> Result<Vec<StoredText>> {
        let rows = self
            .conn
            .query(
                &format!("SELECT t.id, t.decision_id, {read} AS read, s.source_uid, s.content_checksum, t.full_text
                 FROM decision_sources s
                 JOIN decision_text t ON t.decision_id = s.decision_id AND t.source_uid = s.source_uid
                 WHERE s.source_uid >= $1 AND s.source_uid < $2 AND s.source_uid > $3
                   AND s.deleted_at IS NULL
                   AND (t.full_text ~ '&(#[0-9]+|#[xX][0-9A-Fa-f]+|[A-Za-z][A-Za-z0-9]*);'
                        OR left(t.full_text, 1000) ~ '^([^\\n]{{1,80}})\\n([^\\n]*\\n){{1,8}}\\1\\n')
                 ORDER BY s.source_uid
                 LIMIT $4", read = super::pick::read("t")),
                &[&from, &to, &after, &limit],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| StoredText {
                text_id: r.get(0),
                decision_id: r.get(1),
                read: r.get(2),
                source_uid: r.get(3),
                content_checksum: r.get(4),
                full_text: r.get(5),
            })
            .collect())
    }

    /// The French texts of the decisions in `(after, upto]` whose first 2,000
    /// characters hold four capitals for one lowercase letter or more, a superset
    /// of the texts in capitals the caller tells apart.
    #[tracing::instrument(name = "db.texts_in_capitals", skip(self), fields(db.system = "postgresql"))]
    pub async fn texts_in_capitals(&self, after: i64, upto: i64) -> Result<Vec<CapitalText>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT t.id, t.decision_id,
                        {read} AND (d.extract_version IS NULL OR d.extract_version <= $3),
                        t.full_text
                 FROM decision_text t
                 JOIN decisions d ON d.id = t.decision_id
                 WHERE t.decision_id > $1 AND t.decision_id <= $2
                   AND t.lang = 'fr' AND t.deleted_at IS NULL
                   AND length(regexp_replace(substr(t.full_text, 1, 2000), '[^a-zà-ÿ]', '', 'g')) * 4
                     <= length(regexp_replace(substr(t.full_text, 1, 2000), '[^A-ZÀ-Þ]', '', 'g'))",
                    read = super::pick::read("t")
                ),
                &[&after, &upto, &lj_versions::EXTRACT_VERSION],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| CapitalText {
                text_id: r.get(0),
                decision_id: r.get(1),
                extracted: r.get(2),
                full_text: r.get(3),
            })
            .collect())
    }

    /// The texts of the decisions in `(after, upto]` that name an annex heading
    /// or carry an `annex_start`: `(text_id, decision_id, read, full_text)`,
    /// `read` when the passages of its decision are cut from it.
    #[tracing::instrument(name = "db.texts_with_annexes", skip(self), fields(db.system = "postgresql"))]
    pub async fn texts_with_annexes(
        &self,
        after: i64,
        upto: i64,
    ) -> Result<Vec<(i64, i64, bool, String)>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT t.id, t.decision_id, {read}, t.full_text FROM decision_text t
                 WHERE t.decision_id > $1 AND t.decision_id <= $2
                   AND (t.annex_start IS NOT NULL
                        OR t.full_text ~ 'MOYENS?[ \t]+(ANNEX|PRODUIT)|Moyens?[ \t]+(annex|produit)')",
                    read = super::pick::read("t")
                ),
                &[&after, &upto],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
            .collect())
    }

    /// The passages of `decision_ids`.
    #[tracing::instrument(name = "db.passages_of", skip_all, fields(db.system = "postgresql"))]
    pub async fn passages_of(&self, decision_ids: &[i64]) -> Result<Vec<PassageSpan>> {
        let rows = self
            .conn
            .query(
                "SELECT id, decision_id, char_start, char_end, zones FROM decision_chunks
                 WHERE decision_id = ANY($1)",
                &[&decision_ids],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4)))
            .collect())
    }

    /// Sets the `zones` of each passage by id. Returns how many changed.
    #[tracing::instrument(name = "db.set_passage_zones", skip_all, fields(db.system = "postgresql"))]
    pub async fn set_passage_zones(&self, zones: &[(i64, Vec<&'static str>)]) -> Result<u64> {
        let mut changed = 0;
        for (id, tags) in zones {
            changed += self
                .conn
                .execute(
                    "UPDATE decision_chunks SET zones = $2
                     WHERE id = $1 AND array_remove(zones, 'headnote') <> $2",
                    &[id, tags],
                )
                .await?;
        }
        Ok(changed)
    }

    /// Tags the passages of the decisions in `(after, before]` whose text
    /// carries the court's summary and that lack the tag. Returns how many
    /// changed.
    #[tracing::instrument(name = "db.tag_headnote_passages", skip(self), fields(db.system = "postgresql"))]
    pub async fn tag_headnote_passages(&self, after: i64, before: i64) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "UPDATE decision_chunks c SET zones = c.zones
                 WHERE c.decision_id IN (SELECT decision_id FROM decision_text
                       WHERE summary_quote_start IS NOT NULL
                         AND decision_id > $1 AND decision_id <= $2)
                   AND NOT ($3 = ANY(c.zones))",
                &[&after, &before, &lj_vocab::passage_zones::HEADNOTE],
            )
            .await?)
    }

    /// Sets the `annex_start` of each text by id. Returns how many changed.
    #[tracing::instrument(name = "db.set_annex_starts", skip_all, fields(db.system = "postgresql"))]
    pub async fn set_annex_starts(&self, starts: &[(i64, Option<i32>)]) -> Result<u64> {
        let ids: Vec<i64> = starts.iter().map(|s| s.0).collect();
        let annexes: Vec<Option<i32>> = starts.iter().map(|s| s.1).collect();
        Ok(self
            .conn
            .execute(
                "UPDATE decision_text t SET annex_start = v.annex_start
                 FROM unnest($1::bigint[], $2::int[]) AS v(id, annex_start)
                 WHERE t.id = v.id AND t.annex_start IS DISTINCT FROM v.annex_start",
                &[&ids, &annexes],
            )
            .await?)
    }

    /// Writes each text over the one read, a text changed since it was read
    /// left alone. Returns how many were written.
    #[tracing::instrument(name = "db.overwrite_texts", skip_all, fields(db.system = "postgresql"))]
    pub async fn overwrite_texts(&self, drifts: &[DecisionTextDrift]) -> Result<u64> {
        let ids: Vec<i64> = drifts.iter().map(|d| d.text_id).collect();
        let stored: Vec<&str> = drifts.iter().map(|d| d.stored.as_str()).collect();
        let texts: Vec<&str> = drifts.iter().map(|d| d.full_text.as_str()).collect();
        let annexes: Vec<Option<i32>> = texts.iter().map(|t| annex_start(t)).collect();
        Ok(self
            .conn
            .execute(
                "UPDATE decision_text t SET full_text = v.full_text, annex_start = v.annex_start
                 FROM unnest($1::bigint[], $2::text[], $3::text[], $4::int[])
                      AS v(id, stored, full_text, annex_start)
                 WHERE t.id = v.id AND t.full_text = v.stored",
                &[&ids, &stored, &texts, &annexes],
            )
            .await?)
    }

    /// Writes each read text over the one stored, in the caller's transaction,
    /// and drops the passages of the decisions written, cut from the old text;
    /// a text changed since it was read is left alone. Returns the decisions
    /// written.
    #[tracing::instrument(name = "db.rewrite_read_texts", skip_all, fields(db.system = "postgresql"))]
    pub async fn rewrite_read_texts(&self, drifts: &[DecisionTextDrift]) -> Result<Vec<i64>> {
        let ids: Vec<i64> = drifts.iter().map(|d| d.text_id).collect();
        let stored: Vec<&str> = drifts.iter().map(|d| d.stored.as_str()).collect();
        let texts: Vec<&str> = drifts.iter().map(|d| d.full_text.as_str()).collect();
        let annexes: Vec<Option<i32>> = texts.iter().map(|t| annex_start(t)).collect();
        let written: Vec<i64> = self
            .conn
            .query(
                "UPDATE decision_text t SET full_text = v.full_text, annex_start = v.annex_start
                 FROM unnest($1::bigint[], $2::text[], $3::text[], $4::int[])
                      AS v(id, stored, full_text, annex_start)
                 WHERE t.id = v.id AND t.full_text = v.stored
                 RETURNING t.decision_id",
                &[&ids, &stored, &texts, &annexes],
            )
            .await?
            .iter()
            .map(|r| r.get(0))
            .collect();
        self.drop_passages(&written).await?;
        self.conn
            .execute(
                "UPDATE decisions SET updated_at = $2 WHERE id = ANY($1)",
                &[&written, &now()],
            )
            .await?;
        Ok(written)
    }
}
