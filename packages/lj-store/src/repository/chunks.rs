//! Écriture des chunks (`decision_chunks`) : DELETE + multi-row INSERT avec
//! dénormalisation pre-filter (cf. ADR 0033).

use super::support::as_param_refs;
use super::types::ChunkWrite;
use super::DecisionRepository;
use crate::error::Result;
use lj_core::decision::Decision;
use pgvector::Vector;
use tokio_postgres::types::ToSql;

impl DecisionRepository<'_> {
    /// DELETE existant + multi-row INSERT. Renvoie le nb de chunks insérés.
    /// Initialise les colonnes dénormalisées de pre-filter (cf. ADR 0033) via
    /// sous-requêtes SELECT FROM decisions/legal_citation.
    #[tracing::instrument(name = "db.replace_chunks", skip(self, decision, chunks), fields(db.system = "postgresql", chunks = chunks.len()))]
    pub async fn replace_chunks(
        &self,
        decision_id: i64,
        decision: &Decision,
        chunks: &[ChunkWrite],
    ) -> Result<usize> {
        self.conn
            .execute(
                "DELETE FROM decision_chunks WHERE decision_id = $1",
                &[&decision_id],
            )
            .await?;
        if chunks.is_empty() {
            return Ok(0);
        }

        let jur_type = &decision.jurisdiction_type;

        // Sous-requêtes dénormalisées (pre-filter ADR 0033). `$1` factorise le
        // decision_id. Les arrays légaux (`legal_instruments` = `ref_text_uid`,
        // `legal_article_composite` = `ref_text_uid||'|'||ref_num_key`) sont lus
        // depuis `legal_citation` (ADR 0145, M4) — même token/sémantique que les
        // fonctions `_sync_*_legal_instruments_for` (migration 0098). Les lignes du
        // jour sont déjà écrites (`replace_citations` tourne dans l'upsert, AVANT
        // `replace_chunks`). `search_title` n'est porté que par le chunk 0 (un
        // doc-titre par décision dans `chunks_bm25`, ADR 0073).
        const DENORM: &str = "\
            (SELECT publication_codes FROM decisions WHERE id=(SELECT did FROM did_cte)),\
            (SELECT date_lecture FROM decisions WHERE id=(SELECT did FROM did_cte)),\
            (SELECT array_agg(DISTINCT el->>2 ORDER BY el->>2) \
                    FILTER (WHERE el->>2 IS NOT NULL) \
             FROM legal_citation lc, jsonb_array_elements(lc.spans) AS el \
             WHERE lc.decision_id=(SELECT did FROM did_cte)),\
            (SELECT array_agg(DISTINCT (el->>2) || '|' || (el->>3) \
                              ORDER BY (el->>2) || '|' || (el->>3)) \
                    FILTER (WHERE el->>3 IS NOT NULL) \
             FROM legal_citation lc, jsonb_array_elements(lc.spans) AS el \
             WHERE lc.decision_id=(SELECT did FROM did_cte))";

        let cols = "decision_id, chunk_index, jurisdiction_type, char_start, char_end, \
                    embedding, publication_codes, date_lecture, \
                    legal_instruments, legal_article_composite, search_title";

        // Params positionnels : $1 = decision_id (CTE), puis 6 par chunk.
        let mut params: Vec<Box<dyn ToSql + Sync>> = Vec::with_capacity(1 + chunks.len() * 6);
        params.push(Box::new(decision_id));

        let mut value_rows: Vec<String> = Vec::with_capacity(chunks.len());
        let mut idx = 2; // $1 réservé au CTE.
        for c in chunks {
            // (decision_id, chunk_index, jurisdiction_type, char_start, char_end,
            //  quantize_to_rabitq8($n::vector)::rabitq8(1024), <denorm>, search_title)
            // `body` n'est plus stocké : le texte vit dans decisions.full_text (ADR 0084).
            let row = format!(
                "(${},${},${},${},${},quantize_to_rabitq8(${}::vector)::rabitq8(1024),{},\
                 CASE WHEN ${} = 0 THEN \
                 (SELECT search_title FROM decisions WHERE id=(SELECT did FROM did_cte)) END)",
                idx,
                idx + 1,
                idx + 2,
                idx + 3,
                idx + 4,
                idx + 5,
                DENORM,
                idx + 1,
            );
            value_rows.push(row);
            idx += 6;

            params.push(Box::new(decision_id));
            params.push(Box::new(c.chunk_index));
            params.push(Box::new(jur_type.clone()));
            params.push(Box::new(c.char_start));
            params.push(Box::new(c.char_end));
            params.push(Box::new(c.embedding.clone().map(Vector::from)));
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
}
