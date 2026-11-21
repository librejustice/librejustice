//! Rebasage one-shot des clés d'article vers la clé publique slug :
//! `legal_article.num_key`, `legal_citation.ref_num_key`,
//! `legal_toc_edge.child_num_key`, `legal_link.owner/target_num_key`,
//! `text_case_citation.owner_num_key`. Le mapping vieille clé → clé publique
//! est calculé côté Rust (`lj_norm::article_key`, unique définition de
//! l'alphabet) et posé en table temporaire de session — toutes les passes
//! partagent donc la même connexion. Les composites
//! `decisions/decision_chunks.legal_article_composite` se resynchronisent
//! ensuite depuis `legal_citation` via [`Self::resync_legal_arrays_range`].

use tokio_postgres::types::ToSql;

use super::DecisionRepository;
use crate::error::Result;

/// Tables dont une ligne appartient à une version d'article par son triplet
/// propriétaire `(text_uid, num_key, date_debut)` : elle tombe avec elle.
const OWNED_BY_VERSION: [(&str, &str, &str, &str); 4] = [
    (
        "legal_link",
        "owner_text_uid",
        "owner_num_key",
        "owner_date_debut",
    ),
    ("article_scope", "text_uid", "num_key", "date_debut"),
    (
        "text_legal_citation",
        "owner_text_uid",
        "owner_num_key",
        "owner_date_debut",
    ),
    (
        "text_case_citation",
        "owner_text_uid",
        "owner_num_key",
        "owner_date_debut",
    ),
];

/// Ligne que le désétoilage ferait collisionner sur la PK de `legal_article`.
/// `degenerate` : `date_fin <= date_debut`. `overlaps` : l'intervalle recouvre
/// celui d'une autre version de la clé cible, jour de relais compris.
pub struct StarConflict {
    pub text_uid: String,
    pub new_key: String,
    pub num: String,
    pub num_key: String,
    pub date_debut: String,
    pub date_fin: Option<String>,
    pub status: String,
    pub degenerate: bool,
    pub overlaps: bool,
}

impl<'a> DecisionRepository<'a> {
    /// Clés d'article distinctes de toutes les colonnes porteuses.
    pub async fn distinct_article_keys(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT k FROM (
                     SELECT num_key AS k FROM legal_article
                     UNION SELECT el->>3 FROM legal_citation, jsonb_array_elements(spans) AS el
                           WHERE el->>3 IS NOT NULL
                     UNION SELECT child_num_key FROM legal_toc_edge WHERE child_num_key IS NOT NULL
                     UNION SELECT owner_num_key FROM legal_link WHERE owner_num_key <> ''
                     UNION SELECT target_num_key FROM legal_link WHERE target_num_key IS NOT NULL
                     UNION SELECT owner_num_key FROM text_case_citation WHERE owner_num_key IS NOT NULL
                 ) keys(k)",
                &[],
            )
            .await?;
        Ok(rows.into_iter().map(|r| r.get(0)).collect())
    }

    /// Pose le mapping `vieille clé → clé publique` (paires différentes
    /// uniquement) en table temporaire de session.
    pub async fn create_rekey_map(&self, pairs: &[(String, String)]) -> Result<()> {
        self.conn
            .batch_execute("CREATE TEMP TABLE rekey_map (old text PRIMARY KEY, new text NOT NULL)")
            .await?;
        for chunk in pairs.chunks(1000) {
            let mut params: Vec<&(dyn ToSql + Sync)> = Vec::with_capacity(chunk.len() * 2);
            let mut values = String::new();
            for (i, (old, new)) in chunk.iter().enumerate() {
                if i > 0 {
                    values.push(',');
                }
                values.push_str(&format!("(${},${})", i * 2 + 1, i * 2 + 2));
                params.push(old);
                params.push(new);
            }
            self.conn
                .execute(
                    &format!("INSERT INTO rekey_map (old, new) VALUES {values}"),
                    &params,
                )
                .await?;
        }
        self.conn.batch_execute("ANALYZE rekey_map").await?;
        Ok(())
    }

    /// Fusionne les doublons de `legal_article` que le rebasage ferait
    /// collisionner sur la PK `(text_uid, num_key, date_debut)` — variantes
    /// typographiques du même article (« 11 bis » / « 11 BIS »). Garde la
    /// ligne au texte le plus long (départage : num_key max), supprime les
    /// perdantes et leurs `legal_link` possédés. Renvoie (articles supprimés,
    /// liens supprimés).
    pub async fn rekey_merge_duplicate_articles(&self) -> Result<(u64, u64)> {
        self.conn
            .batch_execute(
                "CREATE TEMP TABLE rekey_losers AS
                 WITH mapped AS (
                     SELECT a.text_uid, a.num_key, a.date_debut,
                            coalesce(m.new, a.num_key) AS nk,
                            length(coalesce(a.texte, '')) AS len
                     FROM legal_article a
                     LEFT JOIN rekey_map m ON m.old = a.num_key
                 ), ranked AS (
                     SELECT text_uid, num_key, date_debut,
                            row_number() OVER (
                                PARTITION BY text_uid, nk, date_debut
                                ORDER BY len DESC, num_key DESC
                            ) AS rn
                     FROM mapped
                 )
                 SELECT text_uid, num_key, date_debut FROM ranked WHERE rn > 1",
            )
            .await?;
        let links = self
            .conn
            .execute(
                "DELETE FROM legal_link l USING rekey_losers x
                 WHERE l.owner_text_uid = x.text_uid
                   AND l.owner_num_key = x.num_key
                   AND l.owner_date_debut = x.date_debut",
                &[],
            )
            .await?;
        let articles = self
            .conn
            .execute(
                "DELETE FROM legal_article a USING rekey_losers x
                 WHERE a.text_uid = x.text_uid
                   AND a.num_key = x.num_key
                   AND a.date_debut = x.date_debut",
                &[],
            )
            .await?;
        self.conn.batch_execute("DROP TABLE rekey_losers").await?;
        Ok((articles, links))
    }

    /// Clés d'article portant une astérisque, toutes colonnes porteuses
    /// confondues, blobs `legal_citation.spans` compris.
    pub async fn starred_article_keys(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT k FROM (
                     SELECT num_key AS k FROM legal_article WHERE num_key LIKE '%*%'
                     UNION SELECT num_key FROM legal_article_usage WHERE num_key LIKE '%*%'
                     UNION SELECT num_key FROM article_scope WHERE num_key LIKE '%*%'
                     UNION SELECT num_key FROM article_commentaire WHERE num_key LIKE '%*%'
                     UNION SELECT child_num_key FROM legal_toc_edge WHERE child_num_key LIKE '%*%'
                     UNION SELECT owner_num_key FROM legal_link WHERE owner_num_key LIKE '%*%'
                     UNION SELECT target_num_key FROM legal_link WHERE target_num_key LIKE '%*%'
                     UNION SELECT owner_num_key FROM text_legal_citation
                            WHERE owner_num_key LIKE '%*%'
                     UNION SELECT ref_num_key FROM text_legal_citation
                            WHERE ref_num_key LIKE '%*%'
                     UNION SELECT owner_num_key FROM text_case_citation
                            WHERE owner_num_key LIKE '%*%'
                     UNION SELECT el->>3 FROM legal_citation, jsonb_array_elements(spans) AS el
                            WHERE el->>3 LIKE '%*%'
                 ) keys(k)",
                &[],
            )
            .await?;
        Ok(rows.into_iter().map(|r| r.get(0)).collect())
    }

    /// Lignes de `legal_article` que le mapping de session ferait collisionner
    /// sur la PK, groupe par groupe, jugées par [`StarConflict`].
    pub async fn star_pk_conflicts(&self) -> Result<Vec<StarConflict>> {
        let rows = self
            .conn
            .query(
                "WITH mapped AS (
                     SELECT a.text_uid, coalesce(m.new, a.num_key) AS nk, a.num, a.num_key,
                            a.date_debut, a.date_fin, a.status
                     FROM legal_article a LEFT JOIN rekey_map m ON m.old = a.num_key
                 ), dup AS (
                     SELECT text_uid, nk, date_debut FROM mapped
                     GROUP BY 1, 2, 3 HAVING count(*) > 1
                 )
                 SELECT c.text_uid, c.nk, c.num, c.num_key, c.date_debut::text,
                        c.date_fin::text, c.status,
                        c.date_fin IS NOT NULL AND c.date_fin <= c.date_debut,
                        EXISTS (
                            SELECT 1 FROM mapped o
                            WHERE o.text_uid = c.text_uid AND o.nk = c.nk
                              AND o.date_debut <> c.date_debut
                              AND o.date_debut <= coalesce(c.date_fin, 'infinity')
                              AND coalesce(o.date_fin, 'infinity') >= c.date_debut
                        )
                 FROM mapped c JOIN dup d USING (text_uid, nk, date_debut)
                 ORDER BY c.text_uid, c.nk, c.date_debut, c.num_key",
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| StarConflict {
                text_uid: r.get(0),
                new_key: r.get(1),
                num: r.get(2),
                num_key: r.get(3),
                date_debut: r.get(4),
                date_fin: r.get(5),
                status: r.get(6),
                degenerate: r.get(7),
                overlaps: r.get(8),
            })
            .collect())
    }

    /// Supprime des versions d'article par PK, et avec elles tout ce que le
    /// triplet propriétaire leur attache. Renvoie une ligne par table touchée.
    pub async fn drop_article_versions(
        &self,
        rows: &[(String, String, String)],
    ) -> Result<Vec<(&'static str, u64)>> {
        let uids: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        let keys: Vec<&str> = rows.iter().map(|r| r.1.as_str()).collect();
        let dates: Vec<&str> = rows.iter().map(|r| r.2.as_str()).collect();
        let mut counts = Vec::new();
        for (table, uid_col, key_col, date_col) in OWNED_BY_VERSION {
            let n = self
                .conn
                .execute(
                    &format!(
                        "DELETE FROM {table} t
                         USING unnest($1::text[], $2::text[], $3::text[]) AS x(uid, key, jour)
                         WHERE t.{uid_col} = x.uid AND t.{key_col} = x.key
                           AND t.{date_col} = x.jour::date"
                    ),
                    &[&uids, &keys, &dates],
                )
                .await?;
            counts.push((table, n));
        }
        let articles = self
            .conn
            .execute(
                "DELETE FROM legal_article a
                 USING unnest($1::text[], $2::text[], $3::text[]) AS x(uid, key, jour)
                 WHERE a.text_uid = x.uid AND a.num_key = x.key AND a.date_debut = x.jour::date",
                &[&uids, &keys, &dates],
            )
            .await?;
        counts.push(("legal_article", articles));
        Ok(counts)
    }

    /// Salit l'ordre de lecture des textes dont une clé va bouger, donc à
    /// appeler **avant** le rebasage : la clé cible se confond avec des clés
    /// banales (`annexe`, `2`), la clé source non.
    pub async fn invalidate_rekeyed_reading_ranks(&self) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "UPDATE legal_article SET reading_rank = NULL
                 WHERE id IN (
                     SELECT s.id FROM (
                         SELECT DISTINCT a.text_uid FROM legal_article a
                         JOIN rekey_map m ON m.old = a.num_key) t
                     CROSS JOIN LATERAL (
                         SELECT a.id FROM legal_article a
                         WHERE a.text_uid = t.text_uid AND a.reading_rank IS NOT NULL
                         LIMIT 1) s)",
                &[],
            )
            .await?)
    }

    /// Lignes qu'un rebasage de cette colonne toucherait, sans rien écrire.
    pub async fn count_rekey_column(&self, table: &str, column: &str) -> Result<i64> {
        Ok(self
            .conn
            .query_one(
                &format!("SELECT count(*) FROM {table} t JOIN rekey_map m ON t.{column} = m.old"),
                &[],
            )
            .await?
            .get(0))
    }

    /// Rebase une colonne clé d'article via le mapping de session. Renvoie le
    /// nombre de lignes réécrites.
    pub async fn rekey_column(&self, table: &str, column: &str) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                &format!(
                    "UPDATE {table} t SET {column} = m.new
                     FROM rekey_map m WHERE t.{column} = m.old"
                ),
                &[],
            )
            .await?)
    }

    /// Décisions dont un blob `legal_citation.spans` porte une clé du mapping,
    /// sans rien écrire.
    pub async fn count_rekey_citation_spans(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_one(
                "SELECT count(*) FROM legal_citation lc
                 WHERE EXISTS (
                     SELECT 1 FROM jsonb_array_elements(lc.spans) AS e
                     JOIN rekey_map m ON m.old = e->>3)",
                &[],
            )
            .await?
            .get(0))
    }

    /// Rebase les `ref_num_key` portés par les blobs `legal_citation.spans` via le
    /// mapping de session. Renvoie le nombre de décisions réécrites.
    pub async fn rekey_citation_spans(&self) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "UPDATE legal_citation lc
                 SET spans = (
                     SELECT jsonb_agg(
                         CASE WHEN m.new IS NOT NULL
                              THEN jsonb_set(s.el, '{3}', to_jsonb(m.new))
                              ELSE s.el END
                         ORDER BY s.i)
                     FROM jsonb_array_elements(lc.spans) WITH ORDINALITY AS s(el, i)
                     LEFT JOIN rekey_map m ON m.old = s.el->>3)
                 WHERE EXISTS (
                     SELECT 1 FROM jsonb_array_elements(lc.spans) AS e
                     JOIN rekey_map m ON m.old = e->>3)",
                &[],
            )
            .await?)
    }

    /// A keyset page of the numbered rows of `legal_article` for the identity
    /// rekey: `(id, text_uid, num, num_key, date_debut::text)`, by `id`.
    pub async fn article_identity_page(
        &self,
        after_id: i64,
        limit: i64,
    ) -> Result<Vec<(i64, String, String, String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT id, text_uid, num, num_key, date_debut::text
                 FROM legal_article WHERE id > $1 AND num <> '' ORDER BY id LIMIT $2",
                &[&after_id, &limit],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4)))
            .collect())
    }

    /// Applique un lot de clés d'identité : réécrit `legal_article.num_key` par
    /// `id`, puis suit les `legal_link` possédés (le triplet `(text_uid, ancienne
    /// clé, date_debut)` identifie l'article pré-rebasage — c'est sa PK). Une ligne
    /// dont la cible PK est déjà occupée (variante typographique du même article)
    /// est sautée — comptée dans le deuxième champ du retour. Renvoie (articles
    /// réécrits, sautés, liens suivis).
    pub async fn apply_article_identity_keys(
        &self,
        rows: &[(i64, String, String, String, String)],
        new_keys: &[String],
    ) -> Result<(u64, u64, u64)> {
        let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
        let updated_ids: std::collections::HashSet<i64> = self
            .conn
            .query(
                // `reading_rank = NULL` : le `num_key` porte l'ordre de lecture, le réécrire
                // salit le texte.
                "UPDATE legal_article a SET num_key = m.new_key, reading_rank = NULL
                 FROM unnest($1::bigint[], $2::text[]) AS m(id, new_key)
                 WHERE a.id = m.id
                   AND NOT EXISTS (
                       SELECT 1 FROM legal_article b
                       WHERE b.text_uid = a.text_uid
                         AND b.num_key = m.new_key
                         AND b.date_debut = a.date_debut
                   )
                 RETURNING a.id",
                &[&ids, &new_keys],
            )
            .await?
            .into_iter()
            .map(|r| r.get(0))
            .collect();
        // Liens possédés : uniquement pour les articles effectivement rebasés
        // (une ligne sautée garde ses liens sous l'ancienne clé, cohérente).
        let (mut text_uids, mut old_keys, mut dates, mut keys) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (row, new_key) in rows.iter().zip(new_keys) {
            if updated_ids.contains(&row.0) {
                text_uids.push(row.1.as_str());
                old_keys.push(row.3.as_str());
                dates.push(row.4.as_str());
                keys.push(new_key.as_str());
            }
        }
        let links = self
            .conn
            .execute(
                "UPDATE legal_link l SET owner_num_key = m.new_key
                 FROM unnest($1::text[], $2::text[], $3::text[], $4::text[])
                      AS m(text_uid, old_key, date_debut, new_key)
                 WHERE l.owner_text_uid = m.text_uid
                   AND l.owner_num_key = m.old_key
                   AND l.owner_date_debut = m.date_debut::date",
                &[&text_uids, &old_keys, &dates, &keys],
            )
            .await?;
        Ok((
            updated_ids.len() as u64,
            rows.len() as u64 - updated_ids.len() as u64,
            links,
        ))
    }

    /// Page keyset des arêtes TOC article pour le rebasage identité :
    /// `(owner_uid, seq, label, child_num_key)`, ordonnée par la PK.
    pub async fn toc_identity_page(
        &self,
        after: Option<(&str, i32)>,
        limit: i64,
    ) -> Result<Vec<(String, i32, String, String)>> {
        let (owner, seq) = after.unwrap_or(("", -1));
        let rows = self
            .conn
            .query(
                "SELECT owner_uid, seq, label, child_num_key
                 FROM legal_toc_edge
                 WHERE child_kind = 'article' AND child_num_key IS NOT NULL
                   AND (owner_uid, seq) > ($1, $2)
                 ORDER BY owner_uid, seq LIMIT $3",
                &[&owner, &seq, &limit],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
            .collect())
    }

    /// Applique un lot de clés d'identité aux arêtes TOC (par PK).
    pub async fn apply_toc_identity_keys(
        &self,
        owners: &[&str],
        seqs: &[i32],
        new_keys: &[String],
    ) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "UPDATE legal_toc_edge e SET child_num_key = m.new_key
                 FROM unnest($1::text[], $2::int[], $3::text[]) AS m(owner_uid, seq, new_key)
                 WHERE e.owner_uid = m.owner_uid AND e.seq = m.seq",
                &[&owners, &seqs, &new_keys],
            )
            .await?)
    }

    /// Supprime les arêtes TOC devenues des doublons exacts après fusion de
    /// variantes (même propriétaire, même clé, même fenêtre) — garde le `seq`
    /// minimal. Restreint aux clés rebasées.
    pub async fn rekey_dedup_toc_edges(&self) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "DELETE FROM legal_toc_edge e USING legal_toc_edge k
                 WHERE e.owner_uid = k.owner_uid
                   AND e.child_kind = k.child_kind
                   AND e.child_num_key = k.child_num_key
                   AND e.child_num_key IN (SELECT new FROM rekey_map)
                   AND e.date_debut IS NOT DISTINCT FROM k.date_debut
                   AND e.date_fin IS NOT DISTINCT FROM k.date_fin
                   AND e.seq > k.seq",
                &[],
            )
            .await?)
    }
}
