//! The courts' own summaries, pasted on the text of their decision in their language.

use std::collections::HashMap;

use super::support::now;
use super::DecisionRepository;
use crate::error::Result;

impl DecisionRepository<'_> {
    /// One entry per docket number and ISO date of the live decisions of `jurisdiction_type`.
    #[tracing::instrument(name = "db.docket_date_index", skip(self), fields(db.system = "postgresql"))]
    pub async fn docket_date_index(
        &self,
        jurisdiction_type: &str,
    ) -> Result<Vec<(String, String, i64)>> {
        let rows = self
            .conn
            .query(
                "SELECT unnest(docket_numbers), date_lecture::text, id \
                 FROM decisions \
                 WHERE jurisdiction_type = $1 AND deleted_at IS NULL \
                   AND date_lecture IS NOT NULL",
                &[&jurisdiction_type],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// The decision of each CELEX a live `cjue` provenance carries.
    #[tracing::instrument(name = "db.cjue_decisions_by_celex", skip(self), fields(db.system = "postgresql"))]
    pub async fn cjue_decisions_by_celex(&self) -> Result<HashMap<String, i64>> {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT split_part(source_uid, '/', 2), decision_id FROM decision_sources \
                 WHERE source = 'cjue' AND deleted_at IS NULL",
                &[],
            )
            .await?;
        Ok(rows.into_iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// The texts of the live decisions of `jurisdiction_type` by decision and language,
    /// `true` for those whose summary `engine` wrote.
    #[tracing::instrument(name = "db.court_summary_texts", skip(self), fields(db.system = "postgresql"))]
    pub async fn court_summary_texts(
        &self,
        jurisdiction_type: &str,
        engine: i16,
    ) -> Result<HashMap<(i64, String), bool>> {
        let rows = self
            .conn
            .query(
                "SELECT t.decision_id, t.lang, t.summary_engine IS NOT DISTINCT FROM $2 \
                 FROM decision_text t JOIN decisions d ON d.id = t.decision_id \
                 WHERE d.jurisdiction_type = $1 AND d.deleted_at IS NULL AND t.deleted_at IS NULL",
                &[&jurisdiction_type, &engine],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| ((r.get(0), r.get(1)), r.get(2)))
            .collect())
    }

    /// Pastes `summary` whole on the text in `lang` of `decision_id`, all of it the court's
    /// words, under `engine` and `generation`. False when no live text is in that language,
    /// or when it already carries that summary.
    #[tracing::instrument(name = "db.paste_court_summary", skip(self, summary), fields(db.system = "postgresql"))]
    pub async fn paste_court_summary(
        &self,
        decision_id: i64,
        lang: &str,
        summary: &str,
        engine: i16,
        generation: i16,
    ) -> Result<bool> {
        let len = summary.chars().count() as i32;
        let written = self
            .conn
            .execute(
                "WITH t AS (UPDATE decision_text SET summary = $3, summary_engine = $4, \
                            summary_prompt_version = $5, summary_quote_start = 0, \
                            summary_quote_end = $6, summary_body_start = 0, summary_body_end = $6 \
                            WHERE decision_id = $1 AND lang = $2 AND deleted_at IS NULL \
                            AND (summary, summary_engine, summary_prompt_version) \
                                IS DISTINCT FROM ($3, $4, $5) \
                            RETURNING decision_id) \
                 UPDATE decisions SET updated_at = $7 WHERE id = (SELECT decision_id FROM t)",
                &[
                    &decision_id,
                    &lang,
                    &summary,
                    &engine,
                    &generation,
                    &len,
                    &now(),
                ],
            )
            .await?;
        Ok(written == 1)
    }
}
