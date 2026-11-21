//! Arêtes `legal_toc_edge` (arbre structurel daté des textes DILA) : écriture
//! par **remplacement par propriétaire** (même patron que `legal_link`), lecture
//! par CTE récursive filtrée à une date (sommaire réel, vue-lecture d'une
//! section), purge des orphelins après backfill.

use std::collections::HashMap;

use chrono::NaiveDate;
use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::types::Type;

use super::types::{SectionArticle, SectionArticles, TocEdgeRow, TocOwner, TocTreeRow};
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

/// An article version's `texte` and `nota`, and the key of that version.
#[derive(Default)]
pub struct ArticleBody {
    pub texte: Option<String>,
    pub nota: Option<String>,
    pub num_key: String,
    pub date_debut: NaiveDate,
}

/// Bounds the walk against a cycle in the source data.
const TOC_MAX_DEPTH: i32 = 32;

struct TocEdge {
    child_kind: String,
    child_uid: String,
    child_cid: Option<String>,
    child_num_key: Option<String>,
    label: String,
    etat: String,
    date_debut: Option<NaiveDate>,
    date_fin: Option<NaiveDate>,
}

impl TocEdge {
    fn covers(&self, at: NaiveDate) -> bool {
        self.date_debut.is_none_or(|d| d <= at) && self.date_fin.is_none_or(|f| f > at)
    }
}

/// Every edge of one text, all versions, each owner's children in `seq` order.
pub struct TocEdges {
    root: String,
    edges: Vec<TocEdge>,
    children: HashMap<String, Vec<usize>>,
    /// Every date an edge opens or closes on, sorted.
    breaks: Vec<NaiveDate>,
}

impl TocEdges {
    fn new(root: &str, owned: Vec<(String, i32, TocEdge)>) -> Self {
        let mut children: HashMap<String, Vec<usize>> = HashMap::new();
        let mut breaks = Vec::new();
        let mut edges = Vec::with_capacity(owned.len());
        for (i, (owner, _, edge)) in owned.into_iter().enumerate() {
            breaks.extend(edge.date_debut);
            breaks.extend(edge.date_fin);
            children.entry(owner).or_default().push(i);
            edges.push(edge);
        }
        breaks.sort_unstable();
        breaks.dedup();
        Self {
            root: root.to_string(),
            edges,
            children,
            breaks,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// The first day of the run of days `at` belongs to, over which the tree
    /// does not change.
    pub fn epoch(&self, at: NaiveDate) -> NaiveDate {
        match self.breaks.partition_point(|b| *b <= at) {
            0 => NaiveDate::MIN,
            i => self.breaks[i - 1],
        }
    }

    /// The tree at `at` in reading order, a section before its children; an
    /// article without a key is left out.
    pub fn tree_at(&self, at: NaiveDate) -> Vec<TocTreeRow> {
        let mut out = Vec::new();
        self.walk(&self.root, 1, at, &mut out);
        out
    }

    fn walk(&self, owner: &str, depth: i32, at: NaiveDate, out: &mut Vec<TocTreeRow>) {
        let Some(kids) = self.children.get(owner) else {
            return;
        };
        for e in kids.iter().map(|&i| &self.edges[i]) {
            if !e.covers(at) {
                continue;
            }
            let section = e.child_kind == "section";
            if section || e.child_num_key.as_deref().is_some_and(|k| !k.is_empty()) {
                out.push(TocTreeRow {
                    depth,
                    child_kind: e.child_kind.clone(),
                    child_uid: e.child_uid.clone(),
                    child_cid: e.child_cid.clone(),
                    child_num_key: e.child_num_key.clone(),
                    label: e.label.clone(),
                    etat: e.etat.clone(),
                });
            }
            if section && depth < TOC_MAX_DEPTH {
                self.walk(&e.child_uid, depth + 1, at, out);
            }
        }
    }

    /// Approximate bytes held, for a cache's weigher.
    pub fn weight(&self) -> usize {
        let edges: usize = self
            .edges
            .iter()
            .map(|e| {
                160 + e.child_kind.len()
                    + e.child_uid.len()
                    + e.child_cid.as_ref().map_or(0, String::len)
                    + e.child_num_key.as_ref().map_or(0, String::len)
                    + e.label.len()
                    + e.etat.len()
            })
            .sum();
        let owners: usize = self.children.keys().map(|k| k.len() + 80).sum();
        edges + owners + 8 * self.edges.len() + 4 * self.breaks.len()
    }
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
        // L'ordre de lecture persisté dérive de cette structure : la réécrire
        // salit le texte, le `NULL` étant le seul marqueur d'un rang périmé. **Un
        // seul article par texte** suffit à le porter : le finisseur sélectionne
        // les textes dont *au moins un* article est `NULL`, puis recalcule le
        // texte entier.
        let mut texts: Vec<&str> = items.iter().map(|(o, _)| o.text_uid.as_str()).collect();
        texts.sort_unstable();
        texts.dedup();
        self.conn
            .execute(
                "UPDATE legal_article SET reading_rank = NULL
                 WHERE id IN (
                     SELECT s.id FROM unnest($1::text[]) AS t(uid)
                     CROSS JOIN LATERAL (
                         SELECT a.id FROM legal_article a
                         WHERE a.text_uid = t.uid AND a.reading_rank IS NOT NULL
                         LIMIT 1) s)",
                &[&texts],
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

    pub async fn toc_tree(&self, text_uid: &str, at: NaiveDate) -> Result<Vec<TocTreeRow>> {
        Ok(self.toc_edges(text_uid).await?.tree_at(at))
    }

    /// Every edge of a text's tree, all versions.
    #[tracing::instrument(name = "db.toc_edges", skip(self), fields(db.system = "postgresql"))]
    pub async fn toc_edges(&self, text_uid: &str) -> Result<TocEdges> {
        let rows = self
            .conn
            .query(
                "SELECT owner_uid, seq, child_kind, child_uid, child_cid, child_num_key, \
                        label, etat, date_debut, date_fin \
                 FROM legal_toc_edge WHERE text_uid = $1",
                &[&text_uid],
            )
            .await?;
        let mut owned: Vec<(String, i32, TocEdge)> = rows
            .iter()
            .map(|r| {
                (
                    r.get(0),
                    r.get(1),
                    TocEdge {
                        child_kind: r.get(2),
                        child_uid: r.get(3),
                        child_cid: r.get(4),
                        child_num_key: r.get(5),
                        label: r.get(6),
                        etat: r.get(7),
                        date_debut: r.get(8),
                        date_fin: r.get(9),
                    },
                )
            })
            .collect();
        owned.sort_unstable_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
        Ok(TocEdges::new(text_uid, owned))
    }

    /// The body and note of the article versions `source_uids`, by uid.
    #[tracing::instrument(name = "db.article_bodies", skip(self, source_uids), fields(db.system = "postgresql", n = source_uids.len()))]
    pub async fn article_bodies(
        &self,
        text_uid: &str,
        source_uids: &[&str],
    ) -> Result<HashMap<String, ArticleBody>> {
        let rows = self
            .conn
            .query(
                "SELECT source_uid, texte, nota, num_key, date_debut FROM legal_article \
                 WHERE text_uid = $1 AND source_uid = ANY($2)",
                &[&text_uid, &source_uids],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.get(0),
                    ArticleBody {
                        texte: r.get(1),
                        nota: r.get(2),
                        num_key: r.get(3),
                        date_debut: r.get(4),
                    },
                )
            })
            .collect())
    }

    /// The key of every article the text ever had, all versions.
    #[tracing::instrument(name = "db.text_article_keys", skip(self), fields(db.system = "postgresql"))]
    pub async fn text_article_keys(&self, text_uid: &str) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT num_key FROM legal_article WHERE text_uid = $1",
                &[&text_uid],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// The articles under each section asked, a version uid or a cid, in force at `at`.
    #[tracing::instrument(name = "db.section_articles", skip(self, ids), fields(db.system = "postgresql", n = ids.len()))]
    pub async fn section_articles(
        &self,
        ids: &[String],
        at: NaiveDate,
    ) -> Result<HashMap<String, SectionArticles>> {
        let sql = concat!(
            "WITH RECURSIVE cids AS ( \
                 SELECT i.id, coalesce((SELECT e.child_cid FROM legal_toc_edge e \
                                        WHERE e.child_uid = i.id AND e.child_cid IS NOT NULL \
                                        LIMIT 1), i.id) AS cid \
                 FROM unnest($1::text[]) AS i(id) \
             ), cur AS ( \
                 SELECT DISTINCT ON (c.id) c.id, e.child_uid AS sec, e.text_uid \
                 FROM cids c JOIN legal_toc_edge e \
                   ON e.child_kind = 'section' AND e.child_cid = c.cid \
                 WHERE ",
            window!("e", "$2"),
            "    ORDER BY c.id, e.date_debut DESC NULLS LAST \
             ), tree AS ( \
                 SELECT cur.id, cur.text_uid, e.child_kind, e.child_uid, e.child_num_key, \
                        e.label, ARRAY[e.seq] AS path, 1 AS depth \
                 FROM cur JOIN legal_toc_edge e ON e.owner_uid = cur.sec \
                 WHERE ",
            window!("e", "$2"),
            "    UNION ALL \
                 SELECT t.id, t.text_uid, e.child_kind, e.child_uid, e.child_num_key, \
                        e.label, t.path || e.seq, t.depth + 1 \
                 FROM tree t JOIN legal_toc_edge e ON e.owner_uid = t.child_uid \
                 WHERE t.child_kind = 'section' AND t.depth < 32 AND ",
            window!("e", "$2"),
            ") SELECT id, text_uid, child_uid, label, child_num_key FROM tree \
              WHERE child_kind = 'article' AND child_num_key <> '' \
              ORDER BY id, path",
        );
        let rows = self.conn.query(sql, &[&ids, &at]).await?;
        let mut sections: HashMap<String, SectionArticles> = HashMap::new();
        for r in &rows {
            sections
                .entry(r.get(0))
                .or_insert_with(|| SectionArticles {
                    text_uid: r.get(1),
                    articles: Vec::new(),
                })
                .articles
                .push(SectionArticle {
                    uid: r.get(2),
                    num: r.get(3),
                    num_key: r.get(4),
                });
        }
        Ok(sections)
    }

    /// Every text with a structure.
    #[tracing::instrument(name = "db.toc_text_uids", skip(self), fields(db.system = "postgresql"))]
    pub async fn toc_text_uids(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query("SELECT DISTINCT text_uid FROM legal_toc_edge", &[])
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
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

    /// Purge autoritaire des arêtes d'un texte curé avant réécriture — le
    /// dataset est la source de vérité de sa structure, y compris quand il n'en
    /// déclare plus.
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

    /// Suppression `.dat` : purge les arêtes dont le propriétaire figure dans
    /// une liste de suppression. `owners` = ids extraits des chemins
    /// (`LEGISCTA` de version, cid de texte pour `texte/struct`).
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

#[cfg(test)]
mod tests {
    use super::{TocEdge, TocEdges};
    use chrono::NaiveDate;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn edge(
        owner: &str,
        seq: i32,
        kind: &str,
        uid: &str,
        key: Option<&str>,
        window: (Option<&str>, Option<&str>),
    ) -> (String, i32, TocEdge) {
        (
            owner.to_string(),
            seq,
            TocEdge {
                child_kind: kind.to_string(),
                child_uid: uid.to_string(),
                child_cid: None,
                child_num_key: key.map(str::to_string),
                label: uid.to_string(),
                etat: "VIGUEUR".to_string(),
                date_debut: window.0.map(day),
                date_fin: window.1.map(day),
            },
        )
    }

    #[test]
    fn tree_at_a_date_reads_each_window_and_keeps_reading_order() {
        let always = (None, None);
        let edges = TocEdges::new(
            "T",
            vec![
                edge("T", 0, "section", "S1", None, always),
                edge("T", 1, "section", "S2", None, (Some("2010-01-01"), None)),
                edge(
                    "S1",
                    0,
                    "article",
                    "A1",
                    Some("1"),
                    (None, Some("2010-01-01")),
                ),
                edge(
                    "S1",
                    1,
                    "article",
                    "A2",
                    Some("2"),
                    (Some("2010-01-01"), None),
                ),
                edge("S1", 2, "article", "A0", Some(""), always),
                edge("S2", 0, "article", "A3", Some("3"), always),
            ],
        );
        let uids = |at: &str| -> Vec<(i32, String)> {
            edges
                .tree_at(day(at))
                .into_iter()
                .map(|r| (r.depth, r.child_uid))
                .collect()
        };
        let row = |depth: i32, uid: &str| (depth, uid.to_string());
        assert_eq!(uids("2009-12-31"), vec![row(1, "S1"), row(2, "A1")]);
        assert_eq!(
            uids("2010-01-01"),
            vec![row(1, "S1"), row(2, "A2"), row(1, "S2"), row(2, "A3")]
        );
        assert_eq!(edges.epoch(day("2015-06-01")), day("2010-01-01"));
        assert_eq!(edges.epoch(day("1990-06-01")), NaiveDate::MIN);
    }
}
