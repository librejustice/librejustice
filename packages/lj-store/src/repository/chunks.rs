//! Écriture des chunks (`decision_chunks`) : DELETE + multi-row INSERT avec
//! dénormalisation pre-filter.

use std::collections::HashMap;

use super::support::as_param_refs;
use super::types::{ChunkWrite, PassageInputs, SummaryStamp};
use super::DecisionRepository;
use crate::error::Result;
use lj_core::decision::Decision;
use pgvector::Vector;
use tokio_postgres::types::ToSql;

impl DecisionRepository<'_> {
    /// DELETE existant + multi-row INSERT. Renvoie le nb de chunks insérés.
    /// Initialise les colonnes dénormalisées de pre-filter par recopie de la ligne
    /// `decisions`, and stamps the decision with the summary they are embedded
    /// behind.
    #[tracing::instrument(name = "db.replace_chunks", skip(self, decision, chunks), fields(db.system = "postgresql", chunks = chunks.len()))]
    pub async fn replace_chunks(
        &self,
        decision_id: i64,
        decision: &Decision,
        chunks: &[ChunkWrite],
        summary: SummaryStamp,
    ) -> Result<usize> {
        self.conn
            .execute(
                "DELETE FROM decision_chunks WHERE decision_id = $1",
                &[&decision_id],
            )
            .await?;
        self.conn
            .execute(
                "UPDATE decisions SET embed_summary_engine = $2, embed_summary_prompt_version = $3
                 WHERE id = $1",
                &[&decision_id, &summary.0, &summary.1],
            )
            .await?;
        if chunks.is_empty() {
            return Ok(0);
        }

        let jur_type = &decision.jurisdiction_type;

        // Sous-requêtes dénormalisées (pre-filter). `$1` factorise le decision_id. Les
        // quatre colonnes sont **recopiées de la ligne `decisions`**, seul porteur
        // canonique : les arrays légaux y sont écrits par `sync_citation_facet_arrays`
        // dans l'upsert, donc AVANT ce `replace_chunks`, avec l'expansion des familles
        // « et suivants » et l'ordre `COLLATE "C"` des fonctions
        // `_sync_*_legal_instruments_for`. `search_title` n'est porté que par le chunk 0
        // (un doc-titre par décision dans `chunks_bm25`).
        const DENORM: &str = "\
            (SELECT publication_codes FROM decisions WHERE id=(SELECT did FROM did_cte)),\
            (SELECT date_lecture FROM decisions WHERE id=(SELECT did FROM did_cte)),\
            (SELECT legal_instruments FROM decisions WHERE id=(SELECT did FROM did_cte)),\
            (SELECT legal_article_composite FROM decisions WHERE id=(SELECT did FROM did_cte))";

        let cols = "decision_id, chunk_index, jurisdiction_type, char_start, char_end, \
                    embedding, publication_codes, date_lecture, \
                    legal_instruments, legal_article_composite, search_title, zones";

        // Positional params: $1 = decision_id (CTE), then 7 per chunk.
        let mut params: Vec<Box<dyn ToSql + Sync>> = Vec::with_capacity(1 + chunks.len() * 7);
        params.push(Box::new(decision_id));

        let mut value_rows: Vec<String> = Vec::with_capacity(chunks.len());
        let mut idx = 2; // $1 réservé au CTE.
        for c in chunks {
            // (decision_id, chunk_index, jurisdiction_type, char_start, char_end,
            //  quantize_to_rabitq8($n::vector)::rabitq8(1024), <denorm>, search_title)
            let row = format!(
                "(${},${},${},${},${},quantize_to_rabitq8(${}::vector)::rabitq8(1024),{},\
                 CASE WHEN ${} = 0 THEN \
                 (SELECT search_title FROM decisions WHERE id=(SELECT did FROM did_cte)) END,${})",
                idx,
                idx + 1,
                idx + 2,
                idx + 3,
                idx + 4,
                idx + 5,
                DENORM,
                idx + 1,
                idx + 6,
            );
            value_rows.push(row);
            idx += 7;

            params.push(Box::new(decision_id));
            params.push(Box::new(c.chunk_index));
            params.push(Box::new(jur_type.clone()));
            params.push(Box::new(c.char_start));
            params.push(Box::new(c.char_end));
            params.push(Box::new(Vector::from(c.embedding.clone())));
            params.push(Box::new(c.zones.clone()));
        }

        let sql = format!(
            "WITH did_cte AS (SELECT $1::bigint AS did) \
             INSERT INTO decision_chunks ({}) VALUES {}",
            cols,
            value_rows.join(","),
        );
        let refs = as_param_refs(&params);
        self.conn.execute(sql.as_str(), &refs).await?;
        Ok(chunks.len())
    }

    /// The stored `source_fields` and text of each active provenance of
    /// `source_uids` whose text the decision holds, when that decision has
    /// passages: what they were cut from.
    #[tracing::instrument(name = "db.passage_inputs", skip(self, source_uids), fields(db.system = "postgresql"))]
    pub async fn passage_inputs(
        &self,
        source_uids: &[String],
    ) -> Result<HashMap<String, PassageInputs>> {
        if source_uids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = self
            .conn
            .query(
                "
                SELECT s.source_uid, s.source_fields, t.full_text
                FROM decision_sources s
                JOIN decision_text t ON t.decision_id = s.decision_id AND t.source_uid = s.source_uid
                WHERE s.source_uid = ANY($1) AND s.deleted_at IS NULL
                  AND EXISTS (SELECT 1 FROM decision_chunks c WHERE c.decision_id = s.decision_id)
                ",
                &[&source_uids],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| (row.get(0), (row.get(1), row.get(2))))
            .collect())
    }

    /// Whether `source_uid` wrote one of the texts the passages of `decision_id` read.
    #[tracing::instrument(name = "db.writes_passage_text", skip(self), fields(db.system = "postgresql"))]
    pub async fn writes_passage_text(&self, decision_id: i64, source_uid: &str) -> Result<bool> {
        let row = self
            .conn
            .query_one(
                &format!(
                    "SELECT EXISTS (SELECT 1 FROM decision_text t
                     WHERE t.decision_id = $1 AND t.source_uid = $2 AND ({passages} OR {read}))",
                    passages = super::pick::passages("t"),
                    read = super::pick::read("t")
                ),
                &[&decision_id, &source_uid],
            )
            .await?;
        Ok(row.get(0))
    }

    /// Drops the passages of `ids` and their stamps, for `embed-missing` to cut
    /// them again. Returns the passages dropped.
    #[tracing::instrument(name = "db.drop_passages", skip(self, ids), fields(db.system = "postgresql"))]
    pub async fn drop_passages(&self, ids: &[i64]) -> Result<u64> {
        let dropped = self
            .conn
            .execute(
                "DELETE FROM decision_chunks WHERE decision_id = ANY($1)",
                &[&ids],
            )
            .await?;
        self.conn
            .execute(
                "UPDATE decisions SET embed_version = NULL, embed_summary_engine = NULL,
                        embed_summary_prompt_version = NULL
                 WHERE id = ANY($1)",
                &[&ids],
            )
            .await?;
        Ok(dropped)
    }

    /// Sets the `embed_version` of `ids`.
    #[tracing::instrument(name = "db.set_embed_version", skip(self, ids), fields(db.system = "postgresql"))]
    pub async fn set_embed_version(&self, ids: &[i64], embed_version: i16) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "UPDATE decisions SET embed_version = $2 WHERE id = ANY($1)",
                &[&ids, &embed_version],
            )
            .await?)
    }
}
