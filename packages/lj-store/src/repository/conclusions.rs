//! `conclusions`, the opinion of an Advocate General or of a rapporteur public, an annex
//! of the rulings that share one of its case numbers.

use std::collections::HashMap;

use chrono::NaiveDate;
use serde_json::Value;

use super::DecisionRepository;
use crate::error::Result;

pub struct ConclusionsWrite<'a> {
    pub source: &'a str,
    pub source_uid: &'a str,
    pub jurisdiction_code: &'a str,
    pub case_numbers: &'a [String],
    pub ruled_on: Option<NaiveDate>,
    pub delivered_on: Option<NaiveDate>,
    pub author: Option<&'a str>,
    pub langs: &'a [String],
    pub source_fields: &'a Value,
    pub public_id: Option<&'a str>,
    pub summary: Option<&'a str>,
    pub summary_engine: Option<i16>,
}

/// Conclusions as a ruling shows them.
pub struct ConclusionsRow {
    pub source: String,
    pub source_uid: String,
    pub ruled_on: Option<NaiveDate>,
    pub delivered_on: Option<NaiveDate>,
    pub author: Option<String>,
    pub langs: Vec<String>,
    pub summary: Option<String>,
    /// Its live page, `(jurisdiction_type, jurisdiction_code, public_id)`.
    pub page: Option<(String, Option<String>, String)>,
}

/// A CJUE opinion without a live page.
pub struct ConclusionsPage {
    pub id: i64,
    pub celex: String,
    pub langs: Vec<String>,
    pub source_fields: Value,
    pub summary: Option<String>,
    pub summary_engine: Option<i16>,
    /// The row its page had, and that row's provenances, `(source_uid, source_fields)`.
    pub retired: Option<(i64, Vec<(String, Value)>)>,
}

impl DecisionRepository<'_> {
    /// Writes a row of `conclusions`, its languages joining those already held.
    #[tracing::instrument(name = "db.upsert_conclusions", skip(self, w), fields(db.system = "postgresql"))]
    pub async fn upsert_conclusions(&self, w: &ConclusionsWrite<'_>) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO conclusions (source, source_uid, jurisdiction_code, case_numbers, ruled_on,
                                          delivered_on, author, langs, source_fields, public_id, summary,
                                          summary_engine)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                 ON CONFLICT (source_uid) DO UPDATE SET
                     case_numbers = EXCLUDED.case_numbers,
                     ruled_on = EXCLUDED.ruled_on,
                     delivered_on = EXCLUDED.delivered_on,
                     author = coalesce(EXCLUDED.author, conclusions.author),
                     langs = ARRAY(SELECT DISTINCT unnest(conclusions.langs || EXCLUDED.langs) ORDER BY 1),
                     source_fields = EXCLUDED.source_fields,
                     public_id = coalesce(conclusions.public_id, EXCLUDED.public_id),
                     summary = coalesce(EXCLUDED.summary, conclusions.summary),
                     summary_engine = coalesce(EXCLUDED.summary_engine, conclusions.summary_engine),
                     updated_at = now()",
                &[
                    &w.source,
                    &w.source_uid,
                    &w.jurisdiction_code,
                    &w.case_numbers,
                    &w.ruled_on,
                    &w.delivered_on,
                    &w.author,
                    &w.langs,
                    &w.source_fields,
                    &w.public_id,
                    &w.summary,
                    &w.summary_engine,
                ],
            )
            .await?;
        Ok(())
    }

    /// The languages and the source fields held for each of `source_uids`.
    pub async fn conclusions_held(
        &self,
        source_uids: &[String],
    ) -> Result<HashMap<String, (Vec<String>, Value)>> {
        let rows = self
            .conn
            .query(
                "SELECT source_uid, langs, source_fields FROM conclusions WHERE source_uid = ANY($1)",
                &[&source_uids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get(0), (r.get(1), r.get(2))))
            .collect())
    }

    /// `(id, title, author)` of each row of `source` whose source fields carry a title.
    pub async fn conclusions_titles(
        &self,
        source: &str,
    ) -> Result<Vec<(i64, String, Option<String>)>> {
        let rows = self
            .conn
            .query(
                "SELECT id, source_fields->>'expression_title', author FROM conclusions
                 WHERE source = $1 AND source_fields ? 'expression_title'",
                &[&source],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    pub async fn set_conclusions_author(&self, id: i64, author: Option<&str>) -> Result<()> {
        self.conn
            .execute(
                "UPDATE conclusions SET author = $2, updated_at = now() WHERE id = $1",
                &[&id, &author],
            )
            .await?;
        Ok(())
    }

    /// The conclusions of a ruling's court sharing one of its case numbers, read the
    /// same day when they name the day of their ruling.
    #[tracing::instrument(name = "db.conclusions_of_ruling", skip(self), fields(db.system = "postgresql"))]
    pub async fn conclusions_of_ruling(
        &self,
        jurisdiction_code: &str,
        docket_numbers: &[String],
        date_lecture: Option<NaiveDate>,
        public_id: &str,
    ) -> Result<Vec<ConclusionsRow>> {
        if docket_numbers.is_empty() {
            return Ok(Vec::new());
        }
        let rows = self
            .conn
            .query(
                "SELECT c.source, c.source_uid, c.ruled_on, c.delivered_on, c.author, c.langs, c.summary,
                        p.jurisdiction_type, p.jurisdiction_code, p.public_id
                 FROM conclusions c
                 LEFT JOIN decisions p ON p.public_id = c.public_id AND p.deleted_at IS NULL
                 WHERE c.jurisdiction_code = $1 AND c.case_numbers && $2
                   AND (c.ruled_on IS NULL OR c.ruled_on = $3)
                   AND c.public_id IS DISTINCT FROM $4
                 ORDER BY c.delivered_on, c.source_uid",
                &[
                    &jurisdiction_code,
                    &docket_numbers,
                    &date_lecture,
                    &public_id,
                ],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| ConclusionsRow {
                source: r.get(0),
                source_uid: r.get(1),
                ruled_on: r.get(2),
                delivered_on: r.get(3),
                author: r.get(4),
                langs: r.get(5),
                summary: r.get(6),
                page: r
                    .get::<_, Option<String>>(9)
                    .map(|public_id| (r.get(7), r.get(8), public_id)),
            })
            .collect())
    }

    /// The ruling an opinion's old address leads to, `(jurisdiction_type, code, public_id)`:
    /// a judgment of one of its case numbers before an order, the earliest first.
    pub async fn conclusions_ruling(
        &self,
        public_id: &str,
    ) -> Result<Option<(String, Option<String>, String)>> {
        let row = self
            .conn
            .query_opt(
                "SELECT d.jurisdiction_type, d.jurisdiction_code, d.public_id
                 FROM conclusions c
                 JOIN decisions d ON d.jurisdiction_code = c.jurisdiction_code
                                 AND d.docket_numbers && c.case_numbers
                                 AND d.deleted_at IS NULL
                                 AND d.public_id <> c.public_id
                 WHERE c.public_id = $1
                 ORDER BY EXISTS (SELECT 1 FROM decision_sources s
                                  WHERE s.decision_id = d.id
                                    AND split_part(s.source_uid, '/', 2) ~ '^[0-9]{5}[CT]J') DESC,
                          d.date_lecture, d.id
                 LIMIT 1",
                &[&public_id],
            )
            .await?;
        Ok(row.map(|r| (r.get(0), r.get(1), r.get(2))))
    }

    /// The CJUE opinions without a live page, by id after `after`, `limit` at most.
    pub async fn cjue_conclusions_to_page(
        &self,
        after: i64,
        limit: i64,
    ) -> Result<Vec<ConclusionsPage>> {
        let rows = self
            .conn
            .query(
                "SELECT c.id, c.source_uid, c.langs, c.source_fields, c.summary, c.summary_engine, d.id
                 FROM conclusions c
                 LEFT JOIN decisions d ON d.public_id = c.public_id
                 WHERE c.source = 'eur-lex' AND c.id > $1
                   AND (d.id IS NULL OR d.deleted_at IS NOT NULL)
                 ORDER BY c.id
                 LIMIT $2",
                &[&after, &limit],
            )
            .await?;
        let retired: Vec<i64> = rows.iter().filter_map(|r| r.get(6)).collect();
        let mut provenances: HashMap<i64, Vec<(String, Value)>> = HashMap::new();
        for r in self
            .conn
            .query(
                "SELECT decision_id, source_uid, source_fields FROM decision_sources
                 WHERE decision_id = ANY($1) AND source = 'cjue'
                 ORDER BY decision_id, source_uid",
                &[&retired],
            )
            .await?
        {
            provenances
                .entry(r.get(0))
                .or_default()
                .push((r.get(1), r.get(2)));
        }
        Ok(rows
            .into_iter()
            .map(|r| {
                let retired = r
                    .get::<_, Option<i64>>(6)
                    .map(|id| (id, provenances.remove(&id).unwrap_or_default()));
                ConclusionsPage {
                    id: r.get(0),
                    celex: r.get(1),
                    langs: r.get(2),
                    source_fields: r.get(3),
                    summary: r.get(4),
                    summary_engine: r.get(5),
                    retired,
                }
            })
            .collect())
    }

    /// Names the page of each of `celexes` that has none, from its live provenances.
    /// Returns the rows named.
    pub async fn pin_conclusions_pages(&self, celexes: &[String]) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "UPDATE conclusions c SET public_id = d.public_id, updated_at = now()
                 FROM decision_sources s
                 JOIN decisions d ON d.id = s.decision_id AND d.deleted_at IS NULL
                 WHERE c.source = 'eur-lex' AND c.source_uid = ANY($1) AND c.public_id IS NULL
                   AND s.source = 'cjue' AND s.deleted_at IS NULL
                   AND split_part(s.source_uid, '/', 2) = c.source_uid",
                &[&celexes],
            )
            .await?)
    }

    /// The Conseil d'État decisions whose ArianeWeb bundle names conclusions that
    /// `conclusions` has never visited, `(case number, reading date)`, the latest first.
    pub async fn ce_conclusions_to_fetch(&self, limit: i64) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT s.source_fields->>'dossier', s.source_fields->>'date_lecture'
                 FROM decision_sources s
                 WHERE s.source = 'ariane-web' AND s.deleted_at IS NULL
                   AND s.source_fields->'commentaires' @> '[{\"kind\":\"conclusions\"}]'
                   AND s.source_fields ? 'dossier' AND s.source_fields ? 'date_lecture'
                   AND NOT EXISTS (
                       SELECT 1 FROM conclusions c
                       WHERE c.source_uid = (s.source_fields->>'dossier') || '@' || (s.source_fields->>'date_lecture'))
                 ORDER BY s.source_fields->>'date_lecture' DESC
                 LIMIT $1",
                &[&limit],
            )
            .await?;
        Ok(rows.into_iter().map(|r| (r.get(0), r.get(1))).collect())
    }
}
