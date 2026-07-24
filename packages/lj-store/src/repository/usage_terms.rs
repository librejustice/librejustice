//! Champ `usage_terms` (ADR 0248) : matérialisation des sacs de n-grammes des
//! contextes de citation sur `legal_article`, par le job `lj-ingest
//! usage-terms`. Le pool vit en table temporaire — toutes les méthodes doivent
//! s'exécuter sur la même connexion (une instance de repository).

use super::DecisionRepository;
use crate::error::Result;

impl DecisionRepository<'_> {
    /// Vide la table des sacs (le job est un overwrite complet : une identité
    /// passée sous le seuil de citations ne garde pas un sac périmé).
    pub async fn usage_terms_reset(&self) -> Result<u64> {
        let n = self
            .conn
            .execute("DELETE FROM legal_article_usage", &[])
            .await?;
        Ok(n)
    }

    /// Pool des identités citées ≥ `min_citations` fois, découpé en `chunks`
    /// paquets par hash. Table temporaire liée à la connexion courante.
    /// `legal_citation` au grain décision (spans jsonb, ADR 0247) : élément
    /// positionnel `[char_start, char_end, ref_text_uid, ref_num_key, suivants]`.
    pub async fn usage_terms_build_pool(&self, chunks: i32, min_citations: i64) -> Result<i64> {
        self.conn
            .execute("DROP TABLE IF EXISTS pg_temp.usage_terms_pool", &[])
            .await?;
        self.conn
            .execute(
                "CREATE TEMP TABLE usage_terms_pool AS
                 SELECT el->>2 AS uid, el->>3 AS num,
                        (abs(hashtext((el->>2) || '/' || (el->>3))) % $1::int) AS chunk
                 FROM legal_citation lc, jsonb_array_elements(lc.spans) el
                 WHERE el->>3 IS NOT NULL
                 GROUP BY 1, 2
                 HAVING count(*) >= $2",
                &[&chunks, &min_citations],
            )
            .await?;
        let row = self
            .conn
            .query_one("SELECT count(*) FROM pg_temp.usage_terms_pool", &[])
            .await?;
        Ok(row.get(0))
    }

    /// Matérialise les sacs d'un chunk du pool : ≤300 contextes de citation
    /// (±200 chars autour des spans, tirage déterministe par hash du
    /// `decision_id` — reproductible, sans biais temporel ; mesuré : le bruit
    /// de tirage passe de ±0,034 nDCG à ±0,006 entre 100 et 300 contextes),
    /// n-grammes uni+bi (bigrammes joints `_`), seuil df ≥ max(2, 3 % des
    /// contextes), tf log-compressé min(⌈2·log₂(df+1)⌉, 24) (répétition du
    /// gramme — la table est faite pour BM25 ; la compression préserve l'ordre
    /// d'importance réel là où un cap dur l'aplatit, à taille de sac égale).
    /// Ne garde que les identités qui ont une version VIGUEUR.
    pub async fn usage_terms_fill_chunk(&self, chunk: i32) -> Result<u64> {
        let n = self
            .conn
            .execute(
                r#"
                WITH ctx AS (
                  SELECT p.uid, p.num,
                         row_number() OVER (PARTITION BY p.uid, p.num) AS rn,
                         lower(substr(d.full_text, greatest(1, (c.el->>0)::int - 200), 200) || ' ' ||
                               substr(d.full_text, (c.el->>1)::int + 1, 200)) AS t
                  FROM pg_temp.usage_terms_pool p
                  CROSS JOIN LATERAL (
                    SELECT lc.decision_id, el.el
                    FROM legal_citation lc
                    CROSS JOIN LATERAL jsonb_array_elements(lc.spans)
                      WITH ORDINALITY el(el, ord)
                    WHERE public.lj_cit_terms(lc.spans) @> ARRAY[p.uid || '|' || p.num]
                      AND el.el->>2 = p.uid AND el.el->>3 = p.num
                    ORDER BY hashtext(lc.decision_id::text), el.ord
                    LIMIT 300) c
                  JOIN decisions d ON d.id = c.decision_id
                  WHERE p.chunk = $1 AND d.full_text IS NOT NULL
                ),
                nctx AS (SELECT uid, num, count(*) AS n FROM ctx GROUP BY 1, 2),
                words AS (
                  SELECT uid, num, rn,
                         array_remove(regexp_split_to_array(
                           regexp_replace(t, '[^a-zà-ÿ -]', ' ', 'g'), '\s+'), '') AS w
                  FROM ctx
                ),
                grams AS (
                  SELECT uid, num, rn, w[i] AS g
                  FROM words, generate_subscripts(w, 1) i WHERE length(w[i]) > 1
                  UNION ALL
                  SELECT uid, num, rn, w[i] || '_' || w[i + 1]
                  FROM words, generate_subscripts(w, 1) i WHERE i + 1 <= array_length(w, 1)
                ),
                df AS (
                  SELECT g.uid, g.num, g.g, count(DISTINCT g.rn) AS df, min(nc.n) AS n
                  FROM grams g JOIN nctx nc ON nc.uid = g.uid AND nc.num = g.num
                  GROUP BY 1, 2, 3
                ),
                kept AS (
                  SELECT uid, num, g,
                         least(ceil(2 * ln(df + 1) / ln(2))::int, 24) AS tf
                  FROM df WHERE df >= greatest(2, ceil(0.03 * n))
                ),
                bags AS (
                  SELECT uid, num, string_agg(repeat(g || ' ', tf::int), '' ORDER BY g) AS bag
                  FROM kept GROUP BY uid, num
                )
                INSERT INTO legal_article_usage (text_uid, num_key, terms)
                SELECT b.uid, b.num, b.bag FROM bags b
                WHERE EXISTS (SELECT 1 FROM legal_article a
                              WHERE a.text_uid = b.uid AND a.num_key = b.num
                                AND a.status = 'VIGUEUR')
                "#,
                &[&chunk],
            )
            .await?;
        Ok(n)
    }
}
