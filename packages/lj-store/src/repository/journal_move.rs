//! A text of the norms that leaves for its publication: the candidates, the
//! trace of every row its removal takes, and the move.

use std::collections::HashSet;

use super::DecisionRepository;
use crate::error::Result;

/// A French text with a JO publication.
#[derive(Debug, Clone)]
pub struct JournalMoveCandidate {
    pub text_uid: String,
    pub role: Option<String>,
    pub nature: String,
    pub title: String,
    /// The target of a link or of another text's citation.
    pub linked: bool,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct JournalMoveCounts {
    pub publications: u64,
    pub slugs: u64,
    pub links: u64,
    pub articles: u64,
    pub bodies: u64,
    pub texts: u64,
}

impl std::ops::AddAssign for JournalMoveCounts {
    fn add_assign(&mut self, o: Self) {
        self.publications += o.publications;
        self.slugs += o.slugs;
        self.links += o.links;
        self.articles += o.articles;
        self.bodies += o.bodies;
        self.texts += o.texts;
    }
}

/// The cascades of `legal_text`, and the rows a text owns with no foreign key.
const TRACED: [(&str, &str); 12] = [
    ("legal_text_slug_redirect", "x.text_uid = t.text_uid"),
    (
        "text_legal_citation",
        "t.text_uid IN (x.owner_text_uid, x.ref_text_uid)",
    ),
    ("text_case_citation", "x.owner_text_uid = t.text_uid"),
    (
        "text_filiation",
        "t.text_uid IN (x.child_text_uid, x.parent_text_uid)",
    ),
    ("legal_article_usage", "x.text_uid = t.text_uid"),
    ("assertion", "x.text_uid = t.text_uid"),
    ("assertion_compile_state", "x.text_uid = t.text_uid"),
    ("text_place_coverage", "x.text_uid = t.text_uid"),
    ("legal_link", "x.owner_text_uid = t.text_uid"),
    ("legal_toc_edge", "x.text_uid = t.text_uid"),
    ("article_scope", "x.text_uid = t.text_uid"),
    ("legal_article", "x.text_uid = t.text_uid"),
];

impl DecisionRepository<'_> {
    /// French texts with a slug, a JO publication, no piece, and
    /// a role `individuel`, a title carrying `protected` or matching `formulas`, or nature `RESULTATS`.
    #[tracing::instrument(name = "db.journal_move_candidates", skip_all, fields(db.system = "postgresql"))]
    pub async fn journal_move_candidates(
        &self,
        protected: &str,
        formulas: &str,
    ) -> Result<Vec<JournalMoveCandidate>> {
        Ok(self
            .conn
            .query(
                "SELECT t.text_uid, t.role, t.nature, t.title,
                        EXISTS (SELECT 1 FROM legal_link l WHERE l.target_text_uid = t.text_uid)
                        OR EXISTS (SELECT 1 FROM text_legal_citation c
                                   WHERE c.ref_text_uid = t.text_uid
                                     AND c.owner_text_uid <> t.text_uid)
                 FROM legal_text t
                 WHERE t.jurisdiction = 'FR' AND t.slug IS NOT NULL
                   AND t.role IS DISTINCT FROM 'vehicule'
                   AND (t.role = 'individuel' OR t.nature = 'RESULTATS'
                        OR strpos(t.title, $1) > 0 OR t.title ~* $2)
                   AND EXISTS (SELECT 1 FROM text_publication p
                               WHERE p.text_uid = t.text_uid AND p.journal_code = 'JORF')
                   AND NOT EXISTS (SELECT 1 FROM legal_article a WHERE a.text_uid = t.text_uid)
                 ORDER BY t.text_uid",
                &[&protected, &formulas],
            )
            .await?
            .iter()
            .map(|r| JournalMoveCandidate {
                text_uid: r.get(0),
                role: r.get(1),
                nature: r.get(2),
                title: r.get(3),
                linked: r.get(4),
            })
            .collect())
    }

    /// French texts of role `individuel` with a slug, a JO publication, and
    /// pieces; `consolidated` when an article comes from another source than the JO.
    #[tracing::instrument(name = "db.journal_move_measures", skip_all, fields(db.system = "postgresql"))]
    pub async fn journal_move_measures(&self) -> Result<Vec<(JournalMoveCandidate, bool)>> {
        Ok(self
            .conn
            .query(
                "SELECT t.text_uid, t.role, t.nature, t.title,
                        EXISTS (SELECT 1 FROM legal_link l WHERE l.target_text_uid = t.text_uid)
                        OR EXISTS (SELECT 1 FROM text_legal_citation c
                                   WHERE c.ref_text_uid = t.text_uid
                                     AND c.owner_text_uid <> t.text_uid),
                        EXISTS (SELECT 1 FROM legal_article a
                                WHERE a.text_uid = t.text_uid AND a.source <> 'jorf'
                                  AND a.num_key <> 'body')
                 FROM legal_text t
                 WHERE t.jurisdiction = 'FR' AND t.slug IS NOT NULL AND t.role = 'individuel'
                   AND EXISTS (SELECT 1 FROM text_publication p
                               WHERE p.text_uid = t.text_uid AND p.journal_code = 'JORF')
                   AND EXISTS (SELECT 1 FROM legal_article a WHERE a.text_uid = t.text_uid)
                 ORDER BY t.text_uid",
                &[],
            )
            .await?
            .iter()
            .map(|r| {
                (
                    JournalMoveCandidate {
                        text_uid: r.get(0),
                        role: r.get(1),
                        nature: r.get(2),
                        title: r.get(3),
                        linked: r.get(4),
                    },
                    r.get(5),
                )
            })
            .collect())
    }

    /// French texts of one of `natures` with a JO publication, but a vehicle;
    /// `consolidated` when an article comes from another source than the JO.
    #[tracing::instrument(name = "db.journal_move_notices", skip_all, fields(db.system = "postgresql"))]
    pub async fn journal_move_notices(
        &self,
        natures: &[&str],
    ) -> Result<Vec<(JournalMoveCandidate, bool)>> {
        Ok(self
            .conn
            .query(
                "SELECT t.text_uid, t.role, t.nature, t.title,
                        EXISTS (SELECT 1 FROM legal_link l WHERE l.target_text_uid = t.text_uid)
                        OR EXISTS (SELECT 1 FROM text_legal_citation c
                                   WHERE c.ref_text_uid = t.text_uid
                                     AND c.owner_text_uid <> t.text_uid),
                        EXISTS (SELECT 1 FROM legal_article a
                                WHERE a.text_uid = t.text_uid AND a.source <> 'jorf'
                                  AND a.num_key <> 'body')
                 FROM legal_text t
                 WHERE t.jurisdiction = 'FR' AND t.nature = ANY($1)
                   AND t.role IS DISTINCT FROM 'vehicule'
                   AND EXISTS (SELECT 1 FROM text_publication p
                               WHERE p.text_uid = t.text_uid AND p.journal_code = 'JORF')
                 ORDER BY t.text_uid",
                &[&natures],
            )
            .await?
            .iter()
            .map(|r| {
                (
                    JournalMoveCandidate {
                        text_uid: r.get(0),
                        role: r.get(1),
                        nature: r.get(2),
                        title: r.get(3),
                        linked: r.get(4),
                    },
                    r.get(5),
                )
            })
            .collect())
    }

    /// The texts of one of `natures` with a publication, of no role setting them apart, with
    /// their order; `consolidated` when an article comes from none of the journals' `sources`.
    #[tracing::instrument(name = "db.journal_move_no_norm", skip_all, fields(db.system = "postgresql"))]
    pub async fn journal_move_no_norm(
        &self,
        natures: &[&str],
        sources: &[&str],
    ) -> Result<Vec<(JournalMoveCandidate, String, bool)>> {
        Ok(self
            .conn
            .query(
                "SELECT t.text_uid, t.role, t.nature, t.title,
                        EXISTS (SELECT 1 FROM legal_link l WHERE l.target_text_uid = t.text_uid)
                        OR EXISTS (SELECT 1 FROM text_legal_citation c
                                   WHERE c.ref_text_uid = t.text_uid
                                     AND c.owner_text_uid <> t.text_uid),
                        t.jurisdiction,
                        EXISTS (SELECT 1 FROM legal_article a
                                WHERE a.text_uid = t.text_uid AND a.num_key <> 'body'
                                  AND a.source <> ALL($2))
                 FROM legal_text t
                 WHERE upper(t.nature) = ANY($1)
                   AND coalesce(t.role, '') NOT IN ('individuel', 'vehicule', 'habilitation')
                   AND EXISTS (SELECT 1 FROM text_publication p WHERE p.text_uid = t.text_uid)
                 ORDER BY t.text_uid",
                &[&natures, &sources],
            )
            .await?
            .iter()
            .map(|r| {
                (
                    JournalMoveCandidate {
                        text_uid: r.get(0),
                        role: r.get(1),
                        nature: r.get(2),
                        title: r.get(3),
                        linked: r.get(4),
                    },
                    r.get(5),
                    r.get(6),
                )
            })
            .collect())
    }

    /// For each `(text_uid, court, docket)`, the decision of `court` filed under `docket`, the
    /// one of the text's date first; a ruling the corpus lacks is left out.
    #[tracing::instrument(name = "db.printed_rulings", skip_all, fields(db.system = "postgresql", rulings = rulings.len()))]
    pub async fn printed_rulings(
        &self,
        rulings: &[(String, &str, String)],
    ) -> Result<Vec<(String, i64)>> {
        let uids: Vec<&str> = rulings.iter().map(|(uid, _, _)| uid.as_str()).collect();
        let courts: Vec<&str> = rulings.iter().map(|(_, court, _)| *court).collect();
        let dockets: Vec<&str> = rulings
            .iter()
            .map(|(_, _, docket)| docket.as_str())
            .collect();
        Ok(self
            .conn
            .query(
                "SELECT r.uid, d.id
                 FROM unnest($1::text[], $2::text[], $3::text[]) r(uid, court, docket)
                 JOIN legal_text t ON t.text_uid = r.uid
                 CROSS JOIN LATERAL (
                   SELECT d.id FROM decisions d
                   WHERE d.jurisdiction_code = r.court AND d.docket_numbers @> ARRAY[r.docket]
                     AND d.deleted_at IS NULL
                   ORDER BY d.date_lecture IS NOT DISTINCT FROM t.date_texte DESC,
                            d.date_lecture, d.id
                   LIMIT 1) d
                 ORDER BY r.uid",
                &[&uids, &courts, &dockets],
            )
            .await?
            .iter()
            .map(|r| (r.get(0), r.get(1)))
            .collect())
    }

    /// The texts of `text_uids` that have a publication, whatever the journal.
    #[tracing::instrument(name = "db.journal_move_listed", skip_all, fields(db.system = "postgresql", texts = text_uids.len()))]
    pub async fn journal_move_listed(
        &self,
        text_uids: &[String],
    ) -> Result<Vec<JournalMoveCandidate>> {
        Ok(self
            .conn
            .query(
                "SELECT t.text_uid, t.role, t.nature, t.title,
                        EXISTS (SELECT 1 FROM legal_link l WHERE l.target_text_uid = t.text_uid)
                        OR EXISTS (SELECT 1 FROM text_legal_citation c
                                   WHERE c.ref_text_uid = t.text_uid
                                     AND c.owner_text_uid <> t.text_uid)
                 FROM legal_text t
                 WHERE t.text_uid = ANY($1)
                   AND EXISTS (SELECT 1 FROM text_publication p WHERE p.text_uid = t.text_uid)
                 ORDER BY t.text_uid",
                &[&text_uids],
            )
            .await?
            .iter()
            .map(|r| JournalMoveCandidate {
                text_uid: r.get(0),
                role: r.get(1),
                nature: r.get(2),
                title: r.get(3),
                linked: r.get(4),
            })
            .collect())
    }

    /// One JSON line per text: its `legal_text` row and every row its removal
    /// takes, each restorable by `jsonb_populate_record`.
    #[tracing::instrument(name = "db.journal_move_trace", skip_all, fields(db.system = "postgresql", texts = text_uids.len()))]
    pub async fn journal_move_trace(&self, text_uids: &[String]) -> Result<Vec<String>> {
        let traced: Vec<String> = TRACED
            .iter()
            .map(|(table, on)| {
                format!(
                    "'{table}', (SELECT coalesce(jsonb_agg(to_jsonb(x)), '[]') FROM {table} x WHERE {on})" // sql-splice-ok: tables and joins from the TRACED const
                )
            })
            .collect();
        let sql = format!(
            "SELECT jsonb_build_object('legal_text', to_jsonb(t), {})::text
             FROM legal_text t WHERE t.text_uid = ANY($1) ORDER BY t.text_uid",
            traced.join(", ")
        );
        Ok(self
            .conn
            .query(&sql, &[&text_uids])
            .await?
            .iter()
            .map(|r| r.get(0))
            .collect())
    }

    /// In one transaction: the articles each journal printed, then nature, NOR, date and a body,
    /// onto the publication where it has none; the slugs to `<journal>:<text_uid>`, or to the
    /// decision `rulings` gives the text; the rows; the text.
    #[tracing::instrument(name = "db.move_to_journal", skip_all, fields(db.system = "postgresql", texts = text_uids.len()))]
    pub async fn move_to_journal(
        &self,
        text_uids: &[String],
        rulings: &[(String, i64)],
    ) -> Result<JournalMoveCounts> {
        let (ruled, decisions): (Vec<&str>, Vec<i64>) =
            rulings.iter().map(|(uid, id)| (uid.as_str(), *id)).unzip();
        self.conn.batch_execute("BEGIN").await?;
        let moved: Result<JournalMoveCounts> = async {
            let (journals, sources): (Vec<String>, Vec<&str>) = self
                .conn
                .query(
                    "SELECT DISTINCT journal_code FROM text_publication WHERE text_uid = ANY($1)",
                    &[&text_uids],
                )
                .await?
                .iter()
                .filter_map(|r| {
                    let code: String = r.get(0);
                    lj_norm::applicability::journal_article_source(&code).map(|s| (code, s))
                })
                .unzip();
            self.conn
                .execute(
                    "UPDATE text_publication p
                     SET articles = w.articles || p.articles
                     FROM (SELECT a.text_uid, j.code,
                                  jsonb_object_agg(a.source_uid, jsonb_strip_nulls(jsonb_build_object(
                                    'text', a.texte, 'nota', a.nota, 'num', nullif(a.num, '')))) AS articles
                           FROM unnest($2::text[], $3::text[]) j(code, source)
                           JOIN legal_article a ON a.source = j.source
                           WHERE a.text_uid = ANY($1) AND a.num_key <> 'body'
                           GROUP BY a.text_uid, j.code) w
                     WHERE p.text_uid = w.text_uid AND p.journal_code = w.code",
                    &[&text_uids, &journals, &sources],
                )
                .await?;
            let bodies = self
                .conn
                .execute(
                    "UPDATE text_publication p SET body = a.texte
                     FROM (SELECT DISTINCT ON (text_uid) text_uid, texte FROM legal_article
                           WHERE text_uid = ANY($1) AND num_key = 'body'
                           ORDER BY text_uid, date_debut DESC) a
                     WHERE p.text_uid = a.text_uid AND p.body IS NULL
                       AND a.texte IS NOT NULL AND p.articles = '{}'::jsonb",
                    &[&text_uids],
                )
                .await?;
            let publications = self
                .conn
                .execute(
                    "UPDATE text_publication p
                     SET nature = coalesce(p.nature, t.nature),
                         nor = coalesce(nullif(p.nor, ''), t.nor),
                         date_texte = coalesce(p.date_texte, t.date_texte)
                     FROM legal_text t
                     WHERE t.text_uid = ANY($1) AND p.text_uid = t.text_uid
                       AND (p.nature IS NULL
                            OR (nullif(p.nor, '') IS NULL AND t.nor IS NOT NULL)
                            OR (p.date_texte IS NULL AND t.date_texte IS NOT NULL))",
                    &[&text_uids],
                )
                .await?;
            let slugs = self
                .conn
                .execute(
                    "INSERT INTO notice_slug (slug, notice_uid)
                     SELECT DISTINCT ON (s.slug) s.slug, lower(p.journal_code) || ':' || s.text_uid
                     FROM (SELECT slug, text_uid FROM legal_text
                           WHERE text_uid = ANY($1) AND slug IS NOT NULL
                           UNION
                           SELECT ancien_slug, text_uid FROM legal_text_slug_redirect
                           WHERE text_uid = ANY($1)) s
                     JOIN text_publication p ON p.text_uid = s.text_uid
                     WHERE s.text_uid <> ALL($2)
                     ORDER BY s.slug, p.language
                     ON CONFLICT (slug) DO NOTHING",
                    &[&text_uids, &ruled],
                )
                .await?
                + self
                    .conn
                    .execute(
                        "INSERT INTO norm_slug_decision (slug, decision_id)
                         SELECT s.slug, r.decision_id
                         FROM (SELECT slug, text_uid FROM legal_text
                               WHERE text_uid = ANY($1) AND slug IS NOT NULL
                               UNION
                               SELECT ancien_slug, text_uid FROM legal_text_slug_redirect
                               WHERE text_uid = ANY($1)) s
                         JOIN unnest($1::text[], $2::bigint[]) r(text_uid, decision_id)
                           USING (text_uid)
                         ON CONFLICT (slug) DO NOTHING",
                        &[&ruled, &decisions],
                    )
                    .await?;
            let links = self
                .conn
                .execute(
                    "DELETE FROM legal_link WHERE owner_text_uid = ANY($1)",
                    &[&text_uids],
                )
                .await?;
            let articles = self
                .conn
                .execute(
                    "DELETE FROM legal_article WHERE text_uid = ANY($1)",
                    &[&text_uids],
                )
                .await?;
            for table in ["legal_toc_edge", "article_scope"] {
                self.conn
                    .execute(
                        &format!("DELETE FROM {table} WHERE text_uid = ANY($1)"), // sql-splice-ok: table from a literal list
                        &[&text_uids],
                    )
                    .await?;
            }
            let texts = self
                .conn
                .execute(
                    "DELETE FROM legal_text WHERE text_uid = ANY($1)",
                    &[&text_uids],
                )
                .await?;
            Ok(JournalMoveCounts {
                publications,
                slugs,
                links,
                articles,
                bodies,
                texts,
            })
        }
        .await;
        match moved {
            Ok(counts) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(counts)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    /// The `text_uid`s whose norm left for a notice of `journal`.
    #[tracing::instrument(name = "db.moved_text_uids", skip(self), fields(db.system = "postgresql"))]
    pub async fn moved_text_uids(&self, journal: &str) -> Result<HashSet<String>> {
        let prefix = format!("{journal}:");
        Ok(self
            .conn
            .query(
                "SELECT DISTINCT substr(notice_uid, length($1) + 1) FROM notice_slug
                 WHERE starts_with(notice_uid, $1)",
                &[&prefix],
            )
            .await?
            .iter()
            .map(|r| r.get(0))
            .collect())
    }

    /// The notice a slug of a text that left the norms leads to.
    #[tracing::instrument(name = "db.notice_of_slug", skip(self), fields(db.system = "postgresql"))]
    pub async fn notice_of_slug(&self, slug: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_opt(
                "SELECT notice_uid FROM notice_slug WHERE slug = $1",
                &[&slug],
            )
            .await?
            .map(|r| r.get(0)))
    }

    /// The decision a slug of a court ruling that left the norms leads to: its type, its
    /// court code and its public id.
    #[tracing::instrument(name = "db.decision_of_moved_slug", skip(self), fields(db.system = "postgresql"))]
    pub async fn decision_of_moved_slug(
        &self,
        slug: &str,
    ) -> Result<Option<(String, Option<String>, String)>> {
        Ok(self
            .conn
            .query_opt(
                "SELECT d.jurisdiction_type, d.jurisdiction_code, d.public_id
                 FROM norm_slug_decision n JOIN decisions d ON d.id = n.decision_id
                 WHERE n.slug = $1 AND d.deleted_at IS NULL",
                &[&slug],
            )
            .await?
            .map(|r| (r.get(0), r.get(1), r.get(2))))
    }

    pub async fn analyze_legal_text(&self) -> Result<()> {
        Ok(self.conn.batch_execute("ANALYZE legal_text").await?)
    }
}
