//! Rebasage one-shot des clés d'article vers la clé publique slug (ADR 0209) :
//! `legal_article.num_key`, `legal_citation.ref_num_key`,
//! `legal_toc_edge.child_num_key`, `legal_link.owner/target_num_key`,
//! `text_case_citation.owner_num_key`. Le mapping vieille clé → clé publique
//! est calculé côté Rust (`lj_core::article_key`, unique définition de
//! l'alphabet) et posé en table temporaire de session — toutes les passes
//! partagent donc la même connexion. Les composites
//! `decisions/decision_chunks.legal_article_composite` se resynchronisent
//! ensuite depuis `legal_citation` via [`Self::resync_legal_arrays_range`].

use tokio_postgres::types::ToSql;

use super::DecisionRepository;
use crate::error::Result;

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

    /// Rebase les `ref_num_key` portés par les blobs `legal_citation.spans`
    /// (ADR 0247) via le mapping de session. Renvoie le nombre de décisions
    /// réécrites.
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

    /// Page keyset de `legal_article` pour le rebasage **identité** (ADR 0236) :
    /// `(id, text_uid, num, num_key, date_debut::text)`, ordonnée par `id`.
    pub async fn article_identity_page(
        &self,
        after_id: i64,
        limit: i64,
    ) -> Result<Vec<(i64, String, String, String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT id, text_uid, num, num_key, date_debut::text
                 FROM legal_article WHERE id > $1 ORDER BY id LIMIT $2",
                &[&after_id, &limit],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4)))
            .collect())
    }

    /// Applique un lot de clés d'identité (ADR 0236) : réécrit
    /// `legal_article.num_key` par `id`, puis suit les `legal_link` possédés
    /// (le triplet `(text_uid, ancienne clé, date_debut)` identifie l'article
    /// pré-rebasage — c'est sa PK). Une ligne dont la cible PK est déjà
    /// occupée (variante typographique du même article) est sautée — comptée
    /// dans le deuxième champ du retour. Renvoie (articles réécrits, sautés,
    /// liens suivis).
    pub async fn apply_article_identity_keys(
        &self,
        rows: &[(i64, String, String, String, String)],
        new_keys: &[String],
    ) -> Result<(u64, u64, u64)> {
        let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
        let updated_ids: std::collections::HashSet<i64> = self
            .conn
            .query(
                "UPDATE legal_article a SET num_key = m.new_key
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
