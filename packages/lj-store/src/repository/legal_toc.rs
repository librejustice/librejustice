//! Arêtes `legal_toc_edge` (arbre structurel daté des textes DILA, ADR 0207) :
//! écriture par **remplacement par propriétaire** (même patron que
//! `legal_link`), lecture par CTE récursive filtrée à une date (sommaire réel,
//! vue-lecture d'une section), purge des orphelins après backfill.

use chrono::NaiveDate;
use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::types::Type;

use super::types::{TocEdgeRow, TocOwner, TocReadingRow, TocTreeRow};
use super::DecisionRepository;
use crate::error::Result;

/// Prédicat SQL de fenêtre de validité d'une arête à la date `$d` (alias `e`).
/// `date_debut` sentinelle (2999/2222) est une vraie date → jamais couvrante.
macro_rules! window {
    ($alias:literal, $param:literal) => {
        concat!(
            "(",
            $alias,
            ".date_debut IS NULL OR ",
            $alias,
            ".date_debut <= ",
            $param,
            ") AND (",
            $alias,
            ".date_fin IS NULL OR ",
            $alias,
            ".date_fin > ",
            $param,
            ")"
        )
    };
}

impl DecisionRepository<'_> {
    /// Remplace les arêtes de chaque propriétaire du batch : DELETE par clés
    /// puis COPY binaire, `seq` = ordre du `Vec`. Un propriétaire présent
    /// plusieurs fois (deux fichiers struct d'un même texte) : le dernier
    /// gagne. Renvoie le nombre de lignes écrites.
    #[tracing::instrument(name = "db.replace_toc_edges", skip_all, fields(db.system = "postgresql", owners = items.len()))]
    pub async fn replace_toc_edges(&self, items: &[(TocOwner, Vec<TocEdgeRow>)]) -> Result<u64> {
        let mut last: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for (i, (o, _)) in items.iter().enumerate() {
            last.insert(o.owner_uid.as_str(), i);
        }
        let items: Vec<&(TocOwner, Vec<TocEdgeRow>)> = items
            .iter()
            .enumerate()
            .filter(|(i, (o, _))| last[o.owner_uid.as_str()] == *i)
            .map(|(_, item)| item)
            .collect();
        if items.is_empty() {
            return Ok(0);
        }
        let owners: Vec<&str> = items.iter().map(|(o, _)| o.owner_uid.as_str()).collect();
        self.conn
            .execute(
                "DELETE FROM legal_toc_edge WHERE owner_uid = ANY($1::text[])",
                &[&owners],
            )
            .await?;

        let sink = self
            .conn
            .copy_in(
                "COPY legal_toc_edge (owner_uid, text_uid, seq, child_kind, child_uid, \
                 child_cid, child_num_key, label, etat, date_debut, date_fin, niv) \
                 FROM STDIN (FORMAT binary)",
            )
            .await?;
        let writer = BinaryCopyInWriter::new(
            sink,
            &[
                Type::TEXT,
                Type::TEXT,
                Type::INT4,
                Type::TEXT,
                Type::TEXT,
                Type::TEXT,
                Type::TEXT,
                Type::TEXT,
                Type::TEXT,
                Type::DATE,
                Type::DATE,
                Type::INT4,
            ],
        );
        tokio::pin!(writer);
        let mut written: u64 = 0;
        for (owner, rows) in items {
            for (seq, e) in rows.iter().enumerate() {
                let seq = seq as i32;
                writer
                    .as_mut()
                    .write(&[
                        &owner.owner_uid,
                        &owner.text_uid,
                        &seq,
                        &e.child_kind,
                        &e.child_uid,
                        &e.child_cid,
                        &e.child_num_key,
                        &e.label,
                        &e.etat,
                        &e.date_debut,
                        &e.date_fin,
                        &e.niv,
                    ])
                    .await?;
                written += 1;
            }
        }
        writer.finish().await?;
        Ok(written)
    }

    /// L'arbre structurel complet d'un texte **à une date**, aplati en ordre de
    /// lecture (chemin de `seq`). Ancre : `owner_uid = text_uid` (le premier
    /// niveau vient du fichier `texte/struct`). Vide si le texte n'a pas de
    /// structure ingérée (l'appelant replie sur le sommaire à plat).
    /// `depth < 32` : garde-fou contre un cycle dans la donnée externe.
    #[tracing::instrument(name = "db.toc_tree", skip(self), fields(db.system = "postgresql"))]
    pub async fn toc_tree(&self, text_uid: &str, at: NaiveDate) -> Result<Vec<TocTreeRow>> {
        let sql = concat!(
            "WITH RECURSIVE tree AS ( \
                 SELECT e.child_kind, e.child_uid, e.child_cid, e.child_num_key, \
                        e.label, e.etat, 1 AS depth, ARRAY[e.seq] AS path \
                 FROM legal_toc_edge e \
                 WHERE e.owner_uid = $1 AND ",
            window!("e", "$2"),
            "    UNION ALL \
                 SELECT c.child_kind, c.child_uid, c.child_cid, c.child_num_key, \
                        c.label, c.etat, t.depth + 1, t.path || c.seq \
                 FROM tree t \
                 JOIN legal_toc_edge c ON c.owner_uid = t.child_uid \
                 WHERE t.child_kind = 'section' AND t.depth < 32 AND ",
            window!("c", "$2"),
            ") SELECT child_kind, child_uid, child_cid, child_num_key, label, etat, depth \
              FROM tree ORDER BY path",
        );
        let rows = self.conn.query(sql, &[&text_uid, &at]).await?;
        Ok(rows
            .iter()
            .map(|r| TocTreeRow {
                child_kind: r.get("child_kind"),
                child_uid: r.get("child_uid"),
                child_cid: r.get("child_cid"),
                child_num_key: r.get("child_num_key"),
                label: r.get("label"),
                etat: r.get("etat"),
                depth: r.get("depth"),
            })
            .collect())
    }

    /// Vue-lecture d'une section (ADR 0207) : résout le `cid` stable vers sa
    /// version couvrant `at`, puis renvoie `(titre de section, sous-arbre en
    /// ordre de lecture)` — corps des articles joints par `source_uid`.
    /// `None` si le `cid` est inconnu du texte à cette date (404 côté API).
    #[tracing::instrument(name = "db.toc_section_reading", skip(self), fields(db.system = "postgresql"))]
    pub async fn toc_section_reading(
        &self,
        text_uid: &str,
        cid: &str,
        at: NaiveDate,
    ) -> Result<Option<(String, Vec<TocReadingRow>)>> {
        let head_sql = concat!(
            "SELECT e.child_uid, e.label FROM legal_toc_edge e \
             WHERE e.text_uid = $1 AND e.child_cid = $2 AND e.child_kind = 'section' AND ",
            window!("e", "$3"),
            " LIMIT 1",
        );
        let Some(head) = self
            .conn
            .query_opt(head_sql, &[&text_uid, &cid, &at])
            .await?
        else {
            return Ok(None);
        };
        let section_uid: String = head.get("child_uid");
        let section_label: String = head.get("label");

        let items = self.reading_subtree(&section_uid, text_uid, at).await?;
        Ok(Some((section_label, items)))
    }

    /// Vue-lecture d'un texte entier : le sous-arbre ancré à la racine
    /// (`owner_uid = text_uid`), corps des articles joints. Vide si le texte
    /// n'a pas de structure ingérée (l'appelant replie sur la lecture à plat).
    #[tracing::instrument(name = "db.toc_text_reading", skip(self), fields(db.system = "postgresql"))]
    pub async fn toc_text_reading(
        &self,
        text_uid: &str,
        at: NaiveDate,
    ) -> Result<Vec<TocReadingRow>> {
        self.reading_subtree(text_uid, text_uid, at).await
    }

    /// Sous-arbre en ordre de lecture ancré à `anchor_uid` (section ou racine
    /// du texte), corps des articles joints par `source_uid`.
    async fn reading_subtree(
        &self,
        anchor_uid: &str,
        text_uid: &str,
        at: NaiveDate,
    ) -> Result<Vec<TocReadingRow>> {
        let sql = concat!(
            "WITH RECURSIVE tree AS ( \
                 SELECT e.child_kind, e.child_uid, e.child_cid, e.child_num_key, \
                        e.label, e.etat, 1 AS depth, ARRAY[e.seq] AS path \
                 FROM legal_toc_edge e \
                 WHERE e.owner_uid = $1 AND ",
            window!("e", "$3"),
            "    UNION ALL \
                 SELECT c.child_kind, c.child_uid, c.child_cid, c.child_num_key, \
                        c.label, c.etat, t.depth + 1, t.path || c.seq \
                 FROM tree t \
                 JOIN legal_toc_edge c ON c.owner_uid = t.child_uid \
                 WHERE t.child_kind = 'section' AND t.depth < 32 AND ",
            window!("c", "$3"),
            ") SELECT t.child_kind, t.child_cid, t.child_num_key, t.label, t.etat, t.depth, \
                     a.texte, a.nota \
              FROM tree t \
              LEFT JOIN legal_article a \
                     ON t.child_kind = 'article' AND a.source_uid = t.child_uid \
                        AND a.text_uid = $2 \
              ORDER BY t.path",
        );
        let rows = self.conn.query(sql, &[&anchor_uid, &text_uid, &at]).await?;
        Ok(rows
            .iter()
            .map(|r| TocReadingRow {
                child_kind: r.get("child_kind"),
                child_cid: r.get("child_cid"),
                child_num_key: r.get("child_num_key"),
                label: r.get("label"),
                etat: r.get("etat"),
                depth: r.get("depth"),
                texte: r.get("texte"),
                nota: r.get("nota"),
            })
            .collect())
    }

    /// Purge les arêtes des textes qui n'ont plus aucun article en base
    /// (texte supprimé par `.dat`, collapsé…). Passe de fin de backfill —
    /// anti-join pleine table, l'appelant lève le `statement_timeout`.
    #[tracing::instrument(name = "db.prune_orphan_toc_edges", skip(self), fields(db.system = "postgresql"))]
    pub async fn prune_orphan_toc_edges(&self) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "DELETE FROM legal_toc_edge e \
                 WHERE NOT EXISTS ( \
                     SELECT 1 FROM legal_article a WHERE a.text_uid = e.text_uid)",
                &[],
            )
            .await?;
        Ok(n)
    }

    /// Purge autoritaire des arêtes d'un texte curé avant réécriture
    /// (ADR 0186) — le dataset est la source de vérité de sa structure,
    /// y compris quand il n'en déclare plus.
    #[tracing::instrument(name = "db.delete_toc_edges_by_text", skip(self), fields(db.system = "postgresql"))]
    pub async fn delete_toc_edges_by_text(&self, text_uid: &str) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "DELETE FROM legal_toc_edge WHERE text_uid = $1",
                &[&text_uid],
            )
            .await?;
        Ok(n)
    }

    /// Suppression `.dat` (ADR 0207) : purge les arêtes dont le propriétaire
    /// figure dans une liste de suppression. `owners` = ids extraits des
    /// chemins (`LEGISCTA` de version, cid de texte pour `texte/struct`).
    #[tracing::instrument(name = "db.delete_toc_edges_by_owners", skip_all, fields(db.system = "postgresql", owners = owners.len()))]
    pub async fn delete_toc_edges_by_owners(&self, owners: &[String]) -> Result<u64> {
        if owners.is_empty() {
            return Ok(0);
        }
        let n = self
            .conn
            .execute(
                "DELETE FROM legal_toc_edge WHERE owner_uid = ANY($1::text[])",
                &[&owners],
            )
            .await?;
        Ok(n)
    }
}
