//! The courts' own summaries, pasted on the text of their decision in their language.

use std::collections::HashMap;

use super::support::now;
use super::DecisionRepository;
use crate::error::Result;

/// The head of a decision's text, then the heads and pourvois of the other live decisions
/// of its court and day that share its Judilibre summary.
pub type CassationArrets = (String, Vec<(String, Vec<String>)>);

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
    /// words, under `engine` and `generation`, and sends the decision back to the embedder
    /// when that text is cut into its passages. False when no live text is in that language,
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
        let sql = format!(
            "WITH t AS (UPDATE decision_text u SET summary = $3, summary_engine = $4, \
                        summary_prompt_version = $5, summary_quote_start = 0, \
                        summary_quote_end = $6, summary_body_start = 0, summary_body_end = $6 \
                        WHERE decision_id = $1 AND lang = $2 AND deleted_at IS NULL \
                        AND (summary, summary_engine, summary_prompt_version) \
                            IS DISTINCT FROM ($3, $4, $5) \
                        RETURNING u.decision_id, {embedded} AS embedded) \
             UPDATE decisions d SET updated_at = $7, \
                embed_summary_engine = CASE WHEN t.embedded THEN NULL \
                                            ELSE d.embed_summary_engine END, \
                embed_summary_prompt_version = CASE WHEN t.embedded THEN NULL \
                                                    ELSE d.embed_summary_prompt_version END \
             FROM t WHERE d.id = t.decision_id",
            embedded = super::pick::embedded("u")
        );
        let written = self
            .conn
            .execute(
                sql.as_str(),
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

    /// The [`CassationArrets`] of each decision.
    #[tracing::instrument(name = "db.cassation_arrets", skip(self), fields(db.system = "postgresql"))]
    pub async fn cassation_arrets(
        &self,
        decision_ids: &[i64],
    ) -> Result<HashMap<i64, CassationArrets>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT DISTINCT d.id, t.id, {head}, t.docket_numbers \
                     FROM decisions d \
                     JOIN decision_sources ds ON ds.decision_id = d.id \
                       AND ds.deleted_at IS NULL AND ds.source = 'judilibre' \
                     JOIN decisions t ON t.jurisdiction_code = d.jurisdiction_code \
                       AND t.date_lecture = d.date_lecture AND t.deleted_at IS NULL \
                     JOIN decision_sources s ON s.decision_id = t.id \
                       AND s.deleted_at IS NULL AND s.source = 'judilibre' \
                       AND s.source_fields->>'summary' = ds.source_fields->>'summary' \
                     WHERE d.id = ANY($1)",
                    head = super::pick::text("left(p.full_text, 64)", "t"),
                ),
                &[&decision_ids],
            )
            .await?;
        let mut arrets: HashMap<i64, CassationArrets> = HashMap::new();
        for r in rows {
            let (id, arret): (i64, i64) = (r.get(0), r.get(1));
            let head = r.get::<_, Option<String>>(2).unwrap_or_default();
            let entry = arrets.entry(id).or_default();
            if arret == id {
                entry.0 = head;
            } else {
                let dockets = r.get::<_, Option<Vec<String>>>(3).unwrap_or_default();
                entry.1.push((head, dockets));
            }
        }
        Ok(arrets)
    }
}
