//! Textes & articles de loi (`legal_text` / `legal_article`) : upsert des
//! textes/articles, suppression LEGI par chemins, law-at-date, timeline des
//! versions, résolution de code, décisions citantes et sitemaps.
//!
//! L'identité d'un texte = `text_uid` (globalement unique, `source` hors clé) ;
//! l'identité d'une version d'article = `(text_uid, num_key, date_debut)`.
//! `source`/`source_uid`/`source_url` sont la provenance par version. Les
//! citations des décisions vivent dans `legal_citation` (module `citations`).

use super::norm_hubs::NORM_SCOPE;
use super::support::{authority_rank_sql, legal_article_row_from_row};
use super::types::{
    ArticleBrowseFilters, ArticleCandidate, ArticleDesignation, ArticleFacetMember, ArticleLegHit,
    ArticleNeighborRow, ArticleRankHit, ArticleRrf, ArticleSearchRow, ArticleSearchStats,
    ArticleTitleMode, BodyWrite, CitingDecisionRow, CoCitedArticleRow, EuTwin, FacetCount,
    FacetValueRow, Fraicheur, InstrumentCandidate, InstrumentResolution, JurisdictionRow,
    LawCodeSummaryRow, LawVersionRow, LegalArticleRow, LegalLinkOwner, LegalLinkRow,
    LegalTextCatalogRow, LegalTextRow, NamedText, PlaceFilter, ReferentialRef, SeededJurisdiction,
    SitemapArticle, SlugSourceRow, TextListRow, TocArticleRow, TocReadingRow, TreatyTitle,
};
use super::DecisionRepository;
use crate::error::{Result, StoreError};
use chrono::NaiveDate;
use lj_core::normalizer::normalize_chars;
use lj_norm::jurisdiction_path::jurisdiction_paths;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use tokio_postgres::types::ToSql;

/// The date a text is filed under: its own date, else its publication date,
/// each read only between 1500 and 2099 (DILA stamps 2999-01-01 on some JORF
/// texts).
pub(super) fn text_date(
    date_texte: Option<NaiveDate>,
    date_publi: Option<NaiveDate>,
) -> Option<NaiveDate> {
    use chrono::Datelike;
    [date_texte, date_publi]
        .into_iter()
        .flatten()
        .find(|d| (1500..2100).contains(&d.year()))
}

const STAMP_SELECT: &str = "SELECT id, text_uid, jurisdiction, nature, \
     date_texte, date_publi, text_date, jurisdiction_paths FROM legal_text";

/// Seuil de bascule des pages « décisions citantes » vers la marche par
/// récence, en nombre de décisions citantes (`citing_decision_counts`).
/// Partagé avec les citantes d'une division (`citations.rs`), qui l'applique à
/// la somme des décomptes de ses articles.
pub(super) const CITING_RECENCY_WALK_MIN_COUNT: i64 = 100_000;

/// Candidats sondés pour le rattrapage d'un nom de texte, au-delà des cinq
/// montrés à l'appelant : le rattrapage exige une cible unique — égalité de
/// clé, de numéro de tête ou préfixe — et non une proximité, donc élargir ne
/// peut qu'ajouter la bonne ligne, jamais du bruit.
const RELAX_CANDIDATES: i64 = 25;

const SLUG_LOCK_KEY: i64 = 0x6c6a_5f73_6c75_6773;

/// Ce qu'un passage de recueil vers la clé qualifiée a déplacé, puis écrit.
/// `non_couvertes` non nul = passage annulé, le recueil reste à la clé nue.
#[derive(Debug, Default, Clone, Copy)]
pub struct TransitRecueil {
    pub requalifies: u64,
    pub porteuses: u64,
    pub non_couvertes: u64,
    pub ecrits: u64,
    pub inchanges: u64,
}

impl DecisionRepository<'_> {
    /// Lit le référentiel `facet_value` entier : labels FR, hiérarchie
    /// (`parent_uid`) et ordre d'affichage, dans l'ordre du seed (`facet, sort`).
    #[tracing::instrument(name = "db.load_facet_values", skip(self), fields(db.system = "postgresql"))]
    pub async fn load_facet_values(&self) -> Result<Vec<FacetValueRow>> {
        let rows = self
            .conn
            .query(
                "SELECT uid, facet, label, abbr, parent_uid, sort, hint \
                 FROM facet_value ORDER BY facet, sort",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| FacetValueRow {
                uid: r.get(0),
                facet: r.get(1),
                label: r.get(2),
                abbr: r.get(3),
                parent_uid: r.get(4),
                sort: r.get(5),
                hint: r.get(6),
            })
            .collect())
    }

    /// Lit le référentiel `jurisdiction` entier : une ligne par unité
    /// juridictionnelle (`tj_le_havre`, `ca_paris`…) avec code source, type,
    /// ville et label FR.
    #[tracing::instrument(name = "db.load_jurisdictions", skip(self), fields(db.system = "postgresql"))]
    pub async fn load_jurisdictions(&self) -> Result<Vec<JurisdictionRow>> {
        let rows = self
            .conn
            .query(
                "SELECT code, source_code, jurisdiction_type, city, label, place FROM jurisdiction",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| JurisdictionRow {
                code: r.get(0),
                source_code: r.get(1),
                jurisdiction_type: r.get(2),
                city: r.get(3),
                label: r.get(4),
                place: r.get(5),
            })
            .collect())
    }

    /// Each court's names in other languages, for the courts that have some.
    #[tracing::instrument(name = "db.load_court_names", skip(self), fields(db.system = "postgresql"))]
    pub async fn load_court_names(&self) -> Result<Vec<(String, serde_json::Value)>> {
        let rows = self
            .conn
            .query(
                "SELECT code, names FROM jurisdiction WHERE names IS NOT NULL",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Every court filed under another: a closed one under the court that
    /// succeeded it, a tribunal paritaire under its tribunal judiciaire.
    #[tracing::instrument(name = "db.jurisdiction_successors", skip(self), fields(db.system = "postgresql"))]
    pub async fn jurisdiction_successors(&self) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT code, successor_code FROM jurisdiction WHERE successor_code IS NOT NULL",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Writes courts our sources do not name in a form the referential can
    /// derive: closed ones, and the successors no decision has created yet.
    #[tracing::instrument(name = "db.seed_jurisdictions", skip_all, fields(db.system = "postgresql", n = rows.len()))]
    pub async fn seed_jurisdictions(&self, rows: &[SeededJurisdiction]) -> Result<u64> {
        let mut written = 0;
        let (living, closed): (Vec<_>, Vec<_>) = rows.iter().partition(|r| r.closed_on.is_none());
        for r in living.into_iter().chain(closed) {
            written += self
                .conn
                .execute(
                    "INSERT INTO jurisdiction \
                       (code, source_code, jurisdiction_type, city, label, closed_on, successor_code) \
                     VALUES ($1, $1, $2, $3, $4, $5, $6) \
                     ON CONFLICT (code) DO UPDATE SET \
                       jurisdiction_type = excluded.jurisdiction_type, city = excluded.city, \
                       label = excluded.label, closed_on = excluded.closed_on, \
                       successor_code = excluded.successor_code \
                     WHERE (jurisdiction.jurisdiction_type, jurisdiction.city, jurisdiction.label, \
                            jurisdiction.closed_on, jurisdiction.successor_code) \
                       IS DISTINCT FROM (excluded.jurisdiction_type, excluded.city, excluded.label, \
                            excluded.closed_on, excluded.successor_code)",
                    &[
                        &r.code,
                        &r.jurisdiction_type,
                        &r.city,
                        &r.label,
                        &r.closed_on,
                        &r.successor_code,
                    ],
                )
                .await?;
        }
        Ok(written)
    }

    /// Writes each court's names in the languages other than its label's, where they changed.
    #[tracing::instrument(name = "db.write_jurisdiction_names", skip_all, fields(db.system = "postgresql", n = names.len()))]
    pub async fn write_jurisdiction_names(
        &self,
        names: &[(&str, serde_json::Value)],
    ) -> Result<u64> {
        let mut written = 0;
        for (code, named) in names {
            written += self
                .conn
                .execute(
                    "UPDATE jurisdiction SET names = $2 WHERE code = $1 AND names IS DISTINCT FROM $2",
                    &[code, &tokio_postgres::types::Json(named)],
                )
                .await?;
        }
        Ok(written)
    }

    /// The nodes of the seat axis, as the nightly snapshot counted them. The
    /// filter refuses a path absent from here rather than serving an empty page.
    #[tracing::instrument(name = "db.load_seat_paths", skip(self), fields(db.system = "postgresql"))]
    pub async fn load_seat_paths(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT value FROM corpus_counts WHERE dimension = 'seat'",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Idempotent upsert of a text on its `text_uid`. `title_key`
    /// (`normalize_instrument(title)`) comes from the caller. Never touches `slug`,
    /// set by [`Self::set_text_slugs`]; a body is written by
    /// [`Self::write_text_bodies`].
    #[tracing::instrument(name = "db.upsert_legal_text", skip(self, text), fields(db.system = "postgresql"))]
    pub async fn upsert_legal_text(&self, text: &LegalTextRow) -> Result<()> {
        self.conn
            .execute(
                "
                INSERT INTO legal_text
                  (text_uid, jurisdiction, title, title_key, nature,
                   last_modified, date_texte, date_publi, eli, nor, instrument_key,
                   status, text_date, jurisdiction_paths)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
                ON CONFLICT (text_uid) DO UPDATE SET
                  jurisdiction = EXCLUDED.jurisdiction,
                  title = EXCLUDED.title,
                  title_key = EXCLUDED.title_key,
                  nature = EXCLUDED.nature,
                  last_modified = EXCLUDED.last_modified,
                  date_texte = EXCLUDED.date_texte,
                  date_publi = EXCLUDED.date_publi,
                  eli = EXCLUDED.eli,
                  nor = EXCLUDED.nor,
                  instrument_key = EXCLUDED.instrument_key,
                  status = EXCLUDED.status,
                  text_date = EXCLUDED.text_date,
                  jurisdiction_paths = EXCLUDED.jurisdiction_paths
                ",
                &[
                    &text.text_uid,
                    &text.jurisdiction,
                    &text.title,
                    &text.title_key,
                    &text.nature,
                    &text.last_modified,
                    &text.date_texte,
                    &text.date_publi,
                    &text.eli,
                    &text.nor,
                    &text.instrument_key,
                    &text.status,
                    &text_date(text.date_texte, text.date_publi),
                    &jurisdiction_paths(&text.text_uid, &text.jurisdiction, &text.nature),
                ],
            )
            .await?;
        Ok(())
    }

    /// The texts of that nature and date, with their titles and instrument keys.
    #[tracing::instrument(name = "db.texts_of_nature_on", skip(self), fields(db.system = "postgresql"))]
    pub async fn texts_of_nature_on(
        &self,
        nature: &str,
        date: NaiveDate,
    ) -> Result<Vec<(String, String, Option<String>)>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, title, instrument_key FROM legal_text \
                 WHERE nature = $1 AND date_texte = $2",
                &[&nature, &date],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// Hands the slug of `from_uid` to `into_uid` when the latter has none.
    #[tracing::instrument(name = "db.adopt_slug", skip(self), fields(db.system = "postgresql"))]
    pub async fn adopt_slug(&self, from_uid: &str, into_uid: &str) -> Result<()> {
        self.conn.batch_execute("BEGIN").await?;
        let res = async {
            let slug: Option<String> = self
                .conn
                .query_opt(
                    "SELECT f.slug FROM legal_text f \
                     JOIN legal_text t ON t.text_uid = $2 AND t.slug IS NULL \
                     WHERE f.text_uid = $1 AND f.slug IS NOT NULL FOR UPDATE",
                    &[&from_uid, &into_uid],
                )
                .await?
                .map(|r| r.get(0));
            if let Some(slug) = slug {
                for (uid, slug) in [(from_uid, None), (into_uid, Some(&slug))] {
                    self.conn
                        .execute(
                            "UPDATE legal_text SET slug = $2 WHERE text_uid = $1",
                            &[&uid, &slug],
                        )
                        .await?;
                }
            }
            Ok(())
        }
        .await;
        match res {
            Ok(()) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    /// Pièce de tête des recueils ayant transité vers la clé qualifiée. Nulle =
    /// le recueil porte encore ses numéros nus.
    #[tracing::instrument(name = "db.base_pieces", skip(self), fields(db.system = "postgresql"))]
    pub async fn base_pieces(&self) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, base_piece_uid FROM legal_text \
                 WHERE base_piece_uid IS NOT NULL",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Recueils d'un fond qui portent encore leurs numéros nus, par uid. Le
    /// curseur part de `legal_text` et non des articles : un recueil dont aucun
    /// article n'est entré est justement celui qui a le plus à recevoir.
    #[tracing::instrument(name = "db.recueils_a_requalifier", skip(self), fields(db.system = "postgresql"))]
    pub async fn recueils_a_requalifier(
        &self,
        uid_prefix: &str,
        limit: i64,
    ) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid FROM legal_text \
                 WHERE text_uid LIKE $1 AND base_piece_uid IS NULL \
                 ORDER BY text_uid LIMIT $2",
                &[&uid_prefix, &limit],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Fait passer un recueil à la clé qualifiée : les articles en base prennent
    /// le préfixe de la pièce qui les numérote, puis ceux que l'ancienne clé
    /// refusait entrent. Une transaction par recueil, `base_piece_uid` posée
    /// dedans : il est entier dans l'ancien monde ou entier dans le nouveau, et
    /// une coupure le rend au premier. Une ligne en base que `carte` ne couvre
    /// pas annule le passage (`non_couvertes`) : elle resterait nue sous un
    /// recueil marqué, et l'incrément suivant écrirait sa jumelle qualifiée.
    #[tracing::instrument(name = "db.transiter_recueil", skip(self, carte, articles), fields(db.system = "postgresql"))]
    pub async fn transiter_recueil(
        &self,
        text_uid: &str,
        base_piece_uid: &str,
        carte: &[(String, String, Option<String>)],
        articles: &[(LegalArticleRow, Vec<LegalLinkRow>)],
    ) -> Result<TransitRecueil> {
        self.conn.batch_execute("BEGIN").await?;
        let transit = self
            .transiter_recueil_inner(text_uid, base_piece_uid, carte, articles)
            .await;
        match transit {
            Ok(counts) if counts.non_couvertes == 0 => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(counts)
            }
            Ok(counts) => {
                self.conn.batch_execute("ROLLBACK").await?;
                Ok(counts)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    async fn transiter_recueil_inner(
        &self,
        text_uid: &str,
        base_piece_uid: &str,
        carte: &[(String, String, Option<String>)],
        articles: &[(LegalArticleRow, Vec<LegalLinkRow>)],
    ) -> Result<TransitRecueil> {
        let (requalifies, porteuses, non_couvertes) = self
            .requalifier_recueil(text_uid, base_piece_uid, carte)
            .await?;
        let mut counts = TransitRecueil {
            requalifies,
            porteuses,
            non_couvertes,
            ecrits: 0,
            inchanges: 0,
        };
        if non_couvertes > 0 {
            return Ok(counts);
        }
        let mut liens = Vec::new();
        for (art, edges) in articles {
            if self
                .upsert_legal_article(art, Fraicheur::Autoritaire)
                .await?
            {
                counts.ecrits += 1;
                liens.push((
                    LegalLinkOwner {
                        text_uid: art.text_uid.clone(),
                        num_key: art.num_key.clone(),
                        date_debut: art.date_debut,
                    },
                    edges.clone(),
                ));
            } else {
                counts.inchanges += 1;
            }
        }
        self.replace_legal_links(&liens).await?;
        Ok(counts)
    }

    async fn requalifier_recueil(
        &self,
        text_uid: &str,
        base_piece_uid: &str,
        carte: &[(String, String, Option<String>)],
    ) -> Result<(u64, u64, u64)> {
        let uids: Vec<&str> = carte.iter().map(|c| c.0.as_str()).collect();
        let prefixes: Vec<&str> = carte.iter().map(|c| c.1.as_str()).collect();
        let chemins: Vec<Option<&str>> = carte.iter().map(|c| c.2.as_deref()).collect();
        let row = self
            .conn
            .query_one(
                "
                WITH cible AS (
                  SELECT a.id, a.num_key AS ancienne, a.date_debut, m.title_path,
                         m.prefixe || a.num_key AS nouvelle
                    FROM legal_article a
                    JOIN unnest($3::text[], $4::text[], $5::text[])
                         AS m(source_uid, prefixe, title_path)
                      ON m.source_uid = a.source_uid
                   WHERE a.text_uid = $1
                     AND (m.prefixe <> '' OR a.title_path IS DISTINCT FROM m.title_path)
                ),
                bougees AS (SELECT * FROM cible WHERE nouvelle <> ancienne),
                orphelines AS (
                  SELECT count(*) AS n FROM legal_article a
                   WHERE a.text_uid = $1
                     AND NOT EXISTS (
                       SELECT 1 FROM unnest($3::text[]) AS u(source_uid)
                        WHERE u.source_uid = a.source_uid)
                ),
                art AS (
                  UPDATE legal_article a
                     SET num_key = c.nouvelle, title_path = c.title_path,
                         reading_rank = NULL
                    FROM cible c WHERE a.id = c.id
                  RETURNING 1
                ),
                liens AS (
                  UPDATE legal_link t SET owner_num_key = c.nouvelle FROM bougees c
                   WHERE t.owner_text_uid = $1 AND t.owner_num_key = c.ancienne
                     AND t.owner_date_debut = c.date_debut
                  RETURNING 1
                ),
                cit_txt AS (
                  UPDATE text_legal_citation t SET owner_num_key = c.nouvelle FROM bougees c
                   WHERE t.owner_text_uid = $1 AND t.owner_num_key = c.ancienne
                     AND t.owner_date_debut = c.date_debut
                  RETURNING 1
                ),
                cit_dec AS (
                  UPDATE text_case_citation t SET owner_num_key = c.nouvelle FROM bougees c
                   WHERE t.owner_text_uid = $1 AND t.owner_num_key = c.ancienne
                     AND t.owner_date_debut = c.date_debut
                  RETURNING 1
                ),
                portee AS (
                  UPDATE article_scope t SET num_key = c.nouvelle FROM bougees c
                   WHERE t.text_uid = $1 AND t.num_key = c.ancienne
                     AND t.date_debut = c.date_debut
                  RETURNING 1
                ),
                usages AS (
                  UPDATE legal_article_usage t SET num_key = c.nouvelle
                    FROM (SELECT DISTINCT ancienne, nouvelle FROM bougees) c
                   WHERE t.text_uid = $1 AND t.num_key = c.ancienne
                  RETURNING 1
                ),
                tete AS (
                  UPDATE legal_text SET base_piece_uid = $2 WHERE text_uid = $1
                  RETURNING 1
                )
                SELECT (SELECT count(*) FROM art)::bigint AS articles,
                       ((SELECT count(*) FROM liens) + (SELECT count(*) FROM cit_txt)
                        + (SELECT count(*) FROM cit_dec) + (SELECT count(*) FROM portee)
                        + (SELECT count(*) FROM usages))::bigint AS porteuses,
                       (SELECT n FROM orphelines)::bigint AS non_couvertes
                ",
                &[&text_uid, &base_piece_uid, &uids, &prefixes, &chemins],
            )
            .await?;
        let (articles, porteuses, orphelines): (i64, i64, i64) = (
            row.get("articles"),
            row.get("porteuses"),
            row.get("non_couvertes"),
        );
        Ok((articles as u64, porteuses as u64, orphelines as u64))
    }

    /// Insère un texte seulement s'il est absent (`ON CONFLICT DO NOTHING`) : un
    /// JORFTEXT déjà porté n'est jamais écrasé par la fiche d'origine du fond JO.
    /// Renvoie `true` si la ligne a été créée.
    pub async fn insert_legal_text_if_absent(&self, text: &LegalTextRow) -> Result<bool> {
        let n = self
            .conn
            .execute(
                "
                INSERT INTO legal_text
                  (text_uid, jurisdiction, title, title_key, nature,
                   last_modified, date_texte, date_publi, eli, nor, instrument_key,
                   status, text_date, jurisdiction_paths)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
                ON CONFLICT (text_uid) DO NOTHING
                ",
                &[
                    &text.text_uid,
                    &text.jurisdiction,
                    &text.title,
                    &text.title_key,
                    &text.nature,
                    &text.last_modified,
                    &text.date_texte,
                    &text.date_publi,
                    &text.eli,
                    &text.nor,
                    &text.instrument_key,
                    &text.status,
                    &text_date(text.date_texte, text.date_publi),
                    &jurisdiction_paths(&text.text_uid, &text.jurisdiction, &text.nature),
                ],
            )
            .await?;
        Ok(n == 1)
    }

    /// Writes each text's body as its `body` piece, an unnumbered row of
    /// `legal_article` dated by its text. `IfAbsent`
    /// writes only on a text with no row in `legal_article`. A protected act keeps
    /// no body. Returns the texts written.
    #[tracing::instrument(name = "db.write_text_bodies", skip(self, bodies), fields(db.system = "postgresql", source, n = bodies.len()))]
    pub async fn write_text_bodies(
        &self,
        source: &str,
        bodies: &[(String, String)],
        mode: BodyWrite,
    ) -> Result<Vec<String>> {
        if bodies.is_empty() {
            return Ok(Vec::new());
        }
        let uids: Vec<&str> = bodies.iter().map(|(u, _)| u.as_str()).collect();
        let texts: Vec<String> = bodies.iter().map(|(_, b)| normalize_chars(b)).collect();
        let checksums: Vec<i64> = texts
            .iter()
            .map(|b| xxhash_rust::xxh3::xxh3_64(b.as_bytes()) as i64)
            .collect();
        let undated: Vec<String> = self
            .conn
            .query(
                "SELECT text_uid FROM legal_text WHERE text_uid = ANY($1) AND text_date IS NULL",
                &[&uids],
            )
            .await?
            .iter()
            .map(|r| r.get(0))
            .collect();
        if !undated.is_empty() {
            return Err(StoreError::Invalid(format!(
                "a body piece is dated by its text, and these have no date: {undated:?}"
            )));
        }
        let protected = format!("%{}%", lj_vocab::PROTECTED_ACCESS_MARKER);
        let rows = self
            .conn
            .query(
                "
                WITH t AS (
                  SELECT t.text_uid, t.text_date, coalesce(t.status, '') AS status,
                         v.texte, v.checksum
                  FROM unnest($1::text[], $2::text[], $3::bigint[]) AS v(text_uid, texte, checksum)
                  JOIN legal_text t USING (text_uid)
                  WHERE t.title NOT LIKE $5
                    AND (NOT $6 OR NOT EXISTS
                         (SELECT 1 FROM legal_article a WHERE a.text_uid = t.text_uid))
                ),
                stale AS (
                  DELETE FROM legal_article a USING t
                  WHERE a.text_uid = t.text_uid AND a.num_key = 'body'
                    AND a.date_debut <> t.text_date
                )
                INSERT INTO legal_article
                  (source, source_uid, text_uid, num, num_key, status, date_debut,
                   texte, content_checksum, position, translation, source_asof)
                SELECT $4, t.text_uid, t.text_uid, '', 'body', t.status, t.text_date,
                       t.texte, t.checksum, 0, 'officiel', CURRENT_DATE
                FROM t
                ON CONFLICT (text_uid, num_key, date_debut) DO UPDATE SET
                  source = EXCLUDED.source,
                  status = EXCLUDED.status,
                  texte = EXCLUDED.texte,
                  content_checksum = EXCLUDED.content_checksum,
                  source_asof = EXCLUDED.source_asof,
                  reading_rank = NULL
                RETURNING text_uid
                ",
                &[
                    &uids,
                    &texts,
                    &checksums,
                    &source,
                    &protected,
                    &(mode == BodyWrite::IfAbsent),
                ],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// The texts of a nature with no article but a `body` piece under 300
    /// characters.
    pub async fn empty_legal_text_uids(&self, nature: &str) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid FROM legal_text lt
                 WHERE nature = $1
                   AND NOT EXISTS (
                     SELECT 1 FROM legal_article la WHERE la.text_uid = lt.text_uid
                       AND NOT (la.num_key = 'body' AND length(la.texte) < 300)
                   )
                 ORDER BY text_uid",
                &[&nature],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// International treaties holding a body, each with the one JORF text that
    /// published it, whose stock articles carry the treaty.
    pub async fn intl_treaty_vehicles(&self) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT t.text_uid, min(l.target_text_uid) FROM legal_text t
                 JOIN legal_link l ON l.owner_text_uid = t.text_uid
                   AND l.verb = 'publie' AND l.target_text_uid LIKE 'JORFTEXT%'
                 WHERE t.text_uid LIKE 'INTL/%'
                   AND EXISTS (SELECT 1 FROM legal_article b
                        WHERE b.text_uid = t.text_uid AND b.num_key = 'body')
                   AND NOT EXISTS (SELECT 1 FROM legal_article a
                        WHERE a.text_uid = t.text_uid AND a.num <> '')
                 GROUP BY t.text_uid
                 HAVING count(DISTINCT l.target_text_uid) = 1
                 ORDER BY t.text_uid",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Texts with a slug, and that slug, sorted by `text_uid`.
    pub async fn texts_with_slug(&self) -> Result<Vec<(SlugSourceRow, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, title, jurisdiction, date_texte::text, nor, slug, date_publi::text \
                 FROM legal_text WHERE slug IS NOT NULL ORDER BY text_uid",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    SlugSourceRow {
                        text_uid: r.get(0),
                        title: r.get(1),
                        jurisdiction: r.get(2),
                        date_texte: r.get(3),
                        date_publi: r.get(6),
                        nor: r.get(4),
                    },
                    r.get(5),
                )
            })
            .collect())
    }

    /// Drops the redirect of `slug` to `text_uid`, the slug free again; the rows dropped.
    pub async fn drop_text_slug_redirect(&self, text_uid: &str, slug: &str) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "DELETE FROM legal_text_slug_redirect WHERE text_uid = $1 AND ancien_slug = $2",
                &[&text_uid, &slug],
            )
            .await?)
    }

    /// Every redirect, `((jurisdiction, old slug), text_uid)`.
    pub async fn text_slug_redirects(&self) -> Result<Vec<((String, String), String)>> {
        let rows = self
            .conn
            .query(
                "SELECT jurisdiction, ancien_slug, text_uid FROM legal_text_slug_redirect",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| ((r.get(0), r.get(1)), r.get(2)))
            .collect())
    }

    /// Opens a slug pass: a transaction holding the lock every pass waits for, so two finishers never mint one free slug twice.
    pub async fn begin_slug_pass(&self) -> Result<()> {
        self.conn.batch_execute("BEGIN").await?;
        self.conn
            .batch_execute("SET LOCAL statement_timeout = 0")
            .await?;
        self.conn
            .execute("SELECT pg_advisory_xact_lock($1)", &[&SLUG_LOCK_KEY])
            .await?;
        Ok(())
    }

    pub async fn end_slug_pass(&self, commit: bool) -> Result<()> {
        self.conn
            .batch_execute(if commit { "COMMIT" } else { "ROLLBACK" })
            .await?;
        Ok(())
    }

    /// Textes sans slug, `(text_uid, title)` triés par `text_uid`.
    pub async fn texts_without_slug(&self) -> Result<Vec<SlugSourceRow>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, title, jurisdiction, date_texte::text, nor, date_publi::text \
                 FROM legal_text WHERE slug IS NULL ORDER BY text_uid",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| SlugSourceRow {
                text_uid: r.get(0),
                title: r.get(1),
                jurisdiction: r.get(2),
                date_texte: r.get(3),
                date_publi: r.get(5),
                nor: r.get(4),
            })
            .collect())
    }

    /// The `(jurisdiction, slug)` pairs already taken: the pair, uniqueness
    /// bearing on it now that one name lives under several jurisdictions. A
    /// bequeathed slug is taken too, a live slug winning over its redirect.
    pub async fn existing_text_slugs(&self) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT jurisdiction, slug FROM legal_text WHERE slug IS NOT NULL \
                 UNION SELECT t.jurisdiction, r.ancien_slug FROM legal_text_slug_redirect r \
                 JOIN legal_text t USING (text_uid)",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// The texts of a foreign jurisdiction and their slug, for the pass that
    /// strips the country suffix.
    pub async fn foreign_text_slugs(&self) -> Result<Vec<(String, String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, jurisdiction, slug FROM legal_text \
                 WHERE slug IS NOT NULL AND jurisdiction NOT IN ('FR', 'EU', 'INTL') \
                 ORDER BY text_uid",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// The international pile, titles included: what the Union enacts is read
    /// from the title, and sorted outside SQL, where the rule can be tested.
    pub async fn international_texts(&self) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, title FROM legal_text \
                 WHERE jurisdiction = 'INTL' ORDER BY text_uid",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    pub async fn set_text_jurisdiction(&self, text_uid: &str, jurisdiction: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE legal_text SET jurisdiction = $2 WHERE text_uid = $1",
                &[&text_uid, &jurisdiction],
            )
            .await?;
        self.stamp_legal_texts(&[text_uid.to_string()]).await?;
        Ok(())
    }

    /// Renames a text and bequeaths its address: the old slug becomes a
    /// redirect and the two denormalised copies follow. One transaction per
    /// text, so the pass can resume.
    pub async fn rename_text_slug(&self, text_uid: &str, ancien: &str, neuf: &str) -> Result<()> {
        self.conn.batch_execute("BEGIN").await?;
        let renamed: Result<()> = async {
            self.conn
                .execute(
                    "INSERT INTO legal_text_slug_redirect (ancien_slug, text_uid, jurisdiction) \
                     SELECT $1, text_uid, jurisdiction FROM legal_text WHERE text_uid = $2 \
                     ON CONFLICT (ancien_slug, jurisdiction) \
                     DO UPDATE SET text_uid = EXCLUDED.text_uid",
                    &[&ancien, &text_uid],
                )
                .await?;
            for sql in [
                "UPDATE legal_text SET slug = $2 WHERE text_uid = $1",
                "UPDATE legal_article SET slug = $2 WHERE text_uid = $1",
            ] {
                self.conn.execute(sql, &[&text_uid, &neuf]).await?;
            }
            // The commentary table holds the slug alone, so the old one is its key.
            self.conn
                .execute(
                    "UPDATE article_commentaire SET text_slug = $2 WHERE text_slug = $1",
                    &[&ancien, &neuf],
                )
                .await?;
            Ok(())
        }
        .await;
        match renamed {
            Ok(()) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    /// The column's only writer, on `slug NULL` alone: a slug once set never
    /// changes.
    pub async fn set_text_slugs(&self, slugs: &[(String, String)]) -> Result<u64> {
        let uids: Vec<&str> = slugs.iter().map(|(u, _)| u.as_str()).collect();
        let vals: Vec<&str> = slugs.iter().map(|(_, s)| s.as_str()).collect();
        let rows = self
            .conn
            .query(
                "
                UPDATE legal_text t
                SET slug = v.slug
                FROM unnest($1::text[], $2::text[]) AS v(text_uid, slug)
                WHERE t.text_uid = v.text_uid AND t.slug IS NULL
                RETURNING t.text_uid
                ",
                &[&uids, &vals],
            )
            .await?;
        Ok(rows.len() as u64)
    }

    /// Pose le flag `num_prefix_agnostic` d'un texte : la résolution d'article
    /// matchera sur le cœur numérique préfixe-strippé pour ce texte (codes
    /// territoriaux PF/NC cités avec un préfixe d'instrument incohérent).
    pub async fn set_legal_text_num_prefix_agnostic(
        &self,
        text_uid: &str,
        flag: bool,
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE legal_text SET num_prefix_agnostic = $2 WHERE text_uid = $1",
                &[&text_uid, &flag],
            )
            .await?;
        Ok(())
    }

    /// Upsert idempotent d'une version d'article sur son identité
    /// `(text_uid, num_key, date_debut)`. Skip si le `content_checksum` (xxh3-64
    /// du bloc source brut) est inchangé. Le checksum `u64` est stocké en `BIGINT`
    /// via cast bit-à-bit `i64::from_ne_bytes`. `date_debut` `None` → sentinelle
    /// '0001-01-01' (borne ouverte ; la PK interdit le NULL).
    ///
    /// Le `DO UPDATE` exige `source_uid` égal : un article distinct qui plie sur la
    /// même PK n'écrase pas l'occupant, il est remonté en WARN et perdu.
    ///
    /// L'écriture situe la version entre ses voisines : elle purge la sentinelle
    /// de même `source_uid`, ferme la version qui précède, et prend pour fin la
    /// veille de sa suivante si celle-ci est déjà écrite. Sans quoi deux textes
    /// se disent applicables le même jour et la lecture n'en sert qu'un. Seules
    /// les voisines immédiates sont vues ; `reconcile` rattrape la chaîne.
    ///
    /// A DILA version id moved to another start date by an authoritative delivery
    /// leaves no row nor edge at its old date, and the ends that date capped
    /// reopen; a replay never writes a version the base holds at another date.
    ///
    /// Renvoie `true` si la ligne a été insérée, modifiée ou dédoublonnée.
    #[tracing::instrument(name = "db.upsert_legal_article", skip(self, art), fields(db.system = "postgresql"))]
    pub async fn upsert_legal_article(
        &self,
        art: &LegalArticleRow,
        fraicheur: Fraicheur,
    ) -> Result<bool> {
        let autoritaire = matches!(fraicheur, Fraicheur::Autoritaire);
        let checksum = i64::from_ne_bytes(art.content_checksum.to_ne_bytes());
        let texte = art.texte.as_deref().map(normalize_chars);
        let nota = art.nota.as_deref().map(normalize_chars);
        let texte_original = art.texte_original.as_deref().map(normalize_chars);
        let row = self
            .conn
            .query_one(
                "
                WITH purge AS (
                  -- Une passe antérieure a écrit cet article sans date de début, donc
                  -- sur la sentinelle ; celle-ci la connaît. Même `source_uid` ⇒ même
                  -- version mal datée, pas une version distincte : sans cette purge la
                  -- PK (text_uid, num_key, date_debut) en fait deux lignes ouvertes,
                  -- toutes deux en vigueur.
                  -- A DILA version id names one version, which has one start date:
                  -- when an authoritative delivery moves it (entry into force
                  -- postponed, unknown date `2222-02-22` fixed later), the row at
                  -- the old date is that same version, stale.
                  DELETE FROM legal_article
                  WHERE $7::date IS NOT NULL
                    AND text_uid = $1 AND num_key = $3 AND source_uid = $13
                    AND date_debut <> $7
                    AND (date_debut = DATE '0001-01-01'
                         OR ($20 AND $13 ~ '^(LEGI|KALI|JORF)ARTI'))
                  RETURNING date_debut
                ),
                liens_purges AS (
                  -- The caller writes the delivered edges under the date that stays.
                  DELETE FROM legal_link
                  WHERE owner_text_uid = $1 AND owner_num_key = $3
                    AND owner_date_debut IN (SELECT date_debut FROM purge)
                ),
                ecartee AS (
                  -- A replay predates the base: the date a newer delivery gave this
                  -- version holds, and the replayed one is not written.
                  SELECT NOT $20 AND $13 ~ '^(LEGI|KALI|JORF)ARTI' AND EXISTS (
                           SELECT 1 FROM legal_article h
                            WHERE h.text_uid = $1 AND h.num_key = $3 AND h.source_uid = $13
                              AND h.date_debut <> COALESCE($7, DATE '0001-01-01')
                              AND h.date_debut <> DATE '0001-01-01') AS v
                ),
                ferme_devanciere AS (
                  -- Un article n'a qu'une version applicable à la fois. La version
                  -- qui précède immédiatement celle-ci se ferme la veille — la
                  -- laisser ouverte ferait dire à deux textes différents qu'ils
                  -- s'appliquent le même jour. Seulement l'immédiate : une plus
                  -- ancienne encore ouverte se ferme sur SA suivante, pas sur
                  -- celle-ci, et c'est le filet hebdo qui rattrape la chaîne.
                  -- Une mort-née, jamais applicable, ne ferme rien ni ne se ferme.
                  UPDATE legal_article p
                     SET date_fin = COALESCE($7, DATE '0001-01-01') - 1
                   WHERE $6 <> 'MODIFIE_MORT_NE'
                     AND NOT (SELECT v FROM ecartee)
                     AND (p.text_uid, p.num_key, p.date_debut) = (
                           SELECT q.text_uid, q.num_key, q.date_debut
                             FROM legal_article q
                            WHERE q.text_uid = $1 AND q.num_key = $3
                              AND q.status <> 'MODIFIE_MORT_NE'
                              AND q.date_debut < COALESCE($7, DATE '0001-01-01')
                              -- A row this statement deletes cannot be updated too:
                              -- the update would win and the row would stay.
                              AND q.date_debut NOT IN (SELECT date_debut FROM purge)
                            ORDER BY q.date_debut DESC LIMIT 1)
                     AND (p.date_fin IS NULL
                          OR p.date_fin >= COALESCE($7, DATE '0001-01-01'))
                  RETURNING 1
                ),
                ins AS (
                  INSERT INTO legal_article
                    (text_uid, num, num_key, position, title_path, status, date_debut,
                     date_fin, texte, nota, content_checksum, source, source_uid, source_url,
                     texte_original, lang_original, translation, source_asof, source_upstream_url)
                  -- La source donne la date de fin quand elle la connaît ; sinon
                  -- c'est la veille de la version suivante, si celle-ci est déjà là.
                  -- Les versions n'arrivent pas dans l'ordre, donc les deux sens se
                  -- traitent : ici la suivante, plus haut la devancière.
                  -- `LEAST`, not `COALESCE`: LEGI and KALI close inclusively,
                  -- end = start of the next one. The source's bound is pulled back.
                  SELECT $1, $2, $3, $4, $5, $6, COALESCE($7, DATE '0001-01-01'),
                         -- An end before the start leaves the row reachable at no
                         -- date; only a stillborn may carry such a window.
                         LEAST(CASE WHEN $6 <> 'MODIFIE_MORT_NE'
                                     AND $8 < COALESCE($7, DATE '0001-01-01')
                                    THEN NULL ELSE $8 END,
                               (SELECT min(n.date_debut) - 1
                                  FROM legal_article n
                                 WHERE n.text_uid = $1 AND n.num_key = $3
                                   AND n.status <> 'MODIFIE_MORT_NE'
                                   AND n.date_debut > COALESCE($7, DATE '0001-01-01')
                                   AND n.date_debut NOT IN (SELECT date_debut FROM purge))),
                         $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19
                  WHERE NOT (SELECT v FROM ecartee)
                  ON CONFLICT (text_uid, num_key, date_debut) DO UPDATE SET
                    num = EXCLUDED.num,
                    position = EXCLUDED.position,
                    title_path = EXCLUDED.title_path,
                    status = EXCLUDED.status,
                    date_fin = EXCLUDED.date_fin,
                    texte = EXCLUDED.texte,
                    nota = EXCLUDED.nota,
                    content_checksum = EXCLUDED.content_checksum,
                    source = EXCLUDED.source,
                    source_uid = EXCLUDED.source_uid,
                    source_url = EXCLUDED.source_url,
                    texte_original = EXCLUDED.texte_original,
                    lang_original = EXCLUDED.lang_original,
                    translation = EXCLUDED.translation,
                    source_asof = EXCLUDED.source_asof,
                    source_upstream_url = EXCLUDED.source_upstream_url,
                    -- Le chemin d'écriture n'attribue jamais un rang de lecture,
                    -- il l'invalide : `NULL` marque le texte à recalculer,
                    -- `refresh_reading_ranks` s'en charge en fin de passe. Une
                    -- insertion arrive à `NULL` par défaut.
                    reading_rank = CASE
                      WHEN legal_article.title_path IS DISTINCT FROM EXCLUDED.title_path
                        OR legal_article.position IS DISTINCT FROM EXCLUDED.position
                      THEN NULL ELSE legal_article.reading_rank END
                  -- Créneau disputé : l'état départage, puis l'autorité de
                  -- source. Une applicable remplace une occupante mort-née ;
                  -- à état égal, la consolidée remplace la parution.
                  WHERE ($20 AND legal_article.source_uid = EXCLUDED.source_uid
                         AND legal_article.content_checksum IS DISTINCT FROM EXCLUDED.content_checksum)
                     OR (legal_article.status = 'MODIFIE_MORT_NE'
                         AND EXCLUDED.status <> 'MODIFIE_MORT_NE')
                     OR (legal_article.source = 'jorf'
                         AND EXCLUDED.source = 'legifrance'
                         AND EXCLUDED.status <> 'MODIFIE_MORT_NE')
                     -- Versions share a start date and the slot holds one: the one
                     -- whose window runs furthest is the only one that ever applied.
                     OR (legal_article.source = EXCLUDED.source
                         AND EXCLUDED.status <> 'MODIFIE_MORT_NE'
                         AND COALESCE(EXCLUDED.date_fin, DATE '9999-12-31')
                             > COALESCE(legal_article.date_fin, DATE '9999-12-31'))
                  RETURNING 1
                ),
                autre_cle AS (
                  -- A JO article has one key in its text: holding this one, its
                  -- rows under another are stale. A treaty record cuts one into many.
                  DELETE FROM legal_article
                  WHERE $12 = 'jorf' AND source = 'jorf' AND $1 NOT LIKE 'INTL/%'
                    AND text_uid = $1 AND source_uid = $13 AND num_key <> $3
                    AND (EXISTS (SELECT 1 FROM ins)
                         OR EXISTS (SELECT 1 FROM legal_article h
                                    WHERE h.text_uid = $1 AND h.num_key = $3
                                      AND h.source_uid = $13))
                )
                SELECT ((SELECT count(*) FROM ins) + (SELECT count(*) FROM purge))::int AS written,
                       ARRAY(SELECT date_debut FROM purge
                              WHERE date_debut <> DATE '0001-01-01') AS redatees,
                       (SELECT a.source_uid FROM legal_article a
                        WHERE a.text_uid = $1 AND a.num_key = $3
                          AND a.date_debut = COALESCE($7, DATE '0001-01-01')) AS holder,
                       (SELECT a.status FROM legal_article a
                        WHERE a.text_uid = $1 AND a.num_key = $3
                          AND a.date_debut = COALESCE($7, DATE '0001-01-01')) AS holder_status,
                       (SELECT a.source FROM legal_article a
                        WHERE a.text_uid = $1 AND a.num_key = $3
                          AND a.date_debut = COALESCE($7, DATE '0001-01-01')) AS holder_source
                ",
                &[
                    &art.text_uid,
                    &art.num,
                    &art.num_key,
                    &art.position,
                    &art.title_path,
                    &art.status,
                    &art.date_debut,
                    &art.date_fin,
                    &texte,
                    &nota,
                    &checksum,
                    &art.source,
                    &art.source_uid,
                    &art.source_url,
                    &texte_original,
                    &art.lang_original,
                    &art.translation,
                    &art.source_asof,
                    &art.source_upstream_url,
                    &autoritaire,
                ],
            )
            .await?;
        let written: i32 = row.get("written");
        let redatees: Vec<NaiveDate> = row.get("redatees");
        if !redatees.is_empty() {
            self.rouvrir_fins_redatees(art, &redatees).await?;
        }
        let holder: Option<String> = row.get("holder");
        let holder_status: Option<String> = row.get("holder_status");
        let holder_source: Option<String> = row.get("holder_source");
        if written == 0 {
            if let Some(holder) = holder.filter(|h| *h != art.source_uid) {
                // L'entrant qui perd par la règle du créneau disputé perd
                // légitimement : rien à arbitrer, on ne réveille personne.
                let entrant_perd = (art.status == "MODIFIE_MORT_NE"
                    && holder_status.as_deref() != Some("MODIFIE_MORT_NE"))
                    || (art.source == "jorf" && holder_source.as_deref() == Some("legifrance"))
                    // La source ferme cette version le jour où elle l'ouvre :
                    // applicable aucun jour, elle ne dispute rien.
                    || (art.date_fin.is_some() && art.date_fin <= art.date_debut);
                if entrant_perd {
                    return Ok(false);
                }
                tracing::warn!(
                    text_uid = %art.text_uid,
                    num = %art.num,
                    num_key = %art.num_key,
                    date_debut = ?art.date_debut,
                    source = %art.source,
                    entrant = %art.source_uid,
                    occupant = %holder,
                    "ARTICLE IDENTITY CLASH: a distinct chronicle article folds onto the same \
                     PK (text_uid, num_key, date_debut), incoming row NOT written, occupant \
                     kept. To review: a number genuinely duplicated in the text, or an \
                     identity key to refine."
                );
            }
        }
        Ok(written > 0)
    }

    /// Reopens the ends the stale dates of a moved version capped: each row of the
    /// article closed the day before one of `redatees` ends the day before its
    /// real next one, the moved version itself no later than its source end.
    async fn rouvrir_fins_redatees(
        &self,
        art: &LegalArticleRow,
        redatees: &[NaiveDate],
    ) -> Result<()> {
        let fin_source = art
            .date_fin
            .filter(|fin| art.date_debut.is_none_or(|debut| *fin >= debut));
        self.conn
            .execute(
                "
                UPDATE legal_article p
                   SET date_fin = LEAST(
                         CASE WHEN p.source_uid = $4 THEN $5::date END,
                         (SELECT min(n.date_debut) FROM legal_article n
                           WHERE n.text_uid = p.text_uid AND n.num_key = p.num_key
                             AND n.status <> 'MODIFIE_MORT_NE'
                             AND n.date_debut > p.date_debut) - 1)
                 WHERE p.text_uid = $1 AND p.num_key = $2
                   AND p.status <> 'MODIFIE_MORT_NE'
                   AND p.date_fin + 1 = ANY($3::date[])
                ",
                &[
                    &art.text_uid,
                    &art.num_key,
                    &redatees,
                    &art.source_uid,
                    &fin_source,
                ],
            )
            .await?;
        Ok(())
    }

    /// Rafraîchit la fraîcheur « as-of » d'une source live re-synchronisée
    /// quotidiennement (legifrance/kali) : une ligne par source dans
    /// `ingest_freshness`.
    #[tracing::instrument(name = "db.upsert_ingest_freshness", skip(self), fields(db.system = "postgresql"))]
    pub async fn upsert_ingest_freshness(&self, source: &str, asof: NaiveDate) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO ingest_freshness (source, asof) VALUES ($1, $2)
                 ON CONFLICT (source) DO UPDATE SET asof = EXCLUDED.asof",
                &[&source, &asof],
            )
            .await?;
        Ok(())
    }

    /// Fraîcheur « as-of » d'une source live (legifrance/kali) depuis
    /// `ingest_freshness`. `None` si la source n'y figure pas — la fraîcheur vit
    /// alors sur `legal_article.source_asof`.
    #[tracing::instrument(name = "db.get_ingest_freshness", skip(self), fields(db.system = "postgresql"))]
    pub async fn get_ingest_freshness(&self, source: &str) -> Result<Option<NaiveDate>> {
        let row = self
            .conn
            .query_opt(
                "SELECT asof FROM ingest_freshness WHERE source = $1",
                &[&source],
            )
            .await?;
        Ok(row.map(|r| r.get(0)))
    }

    /// Date de la dernière ligne ingérée pour chaque source de décisions, hors
    /// tombstones.
    #[tracing::instrument(name = "db.derniere_ingestion_par_source", skip(self), fields(db.system = "postgresql"))]
    pub async fn derniere_ingestion_par_source(&self) -> Result<Vec<(String, NaiveDate)>> {
        let rows = self
            .conn
            .query(
                "SELECT source, max(ingested_at)::date FROM decision_sources \
                  WHERE deleted_at IS NULL GROUP BY source",
                &[],
            )
            .await?;
        Ok(rows.into_iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Paires distinctes `(source, source_url)` des lignes dont `source` est une
    /// catégorie/méthode à reclasser (`treaty`, `eu-law`, `official-fr`,
    /// `traduction-automatique`) et qui portent une URL, d'où le libellé de
    /// diffuseur se dérive (`diffuseur_label_from_url`). Lecture seule.
    #[tracing::instrument(name = "db.distinct_category_source_urls", skip(self), fields(db.system = "postgresql"))]
    pub async fn distinct_category_source_urls(
        &self,
        sources: &[&str],
    ) -> Result<Vec<(String, String)>> {
        let sources: Vec<String> = sources.iter().map(|s| s.to_string()).collect();
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT source, source_url FROM legal_article \
                 WHERE source = ANY($1) AND COALESCE(source_url, '') <> ''",
                &[&sources],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Reclasse `source` pour les lignes d'un couple `(ancien source, URL)` donné
    /// vers le `nouveau` libellé de diffuseur. Renvoie le nombre de lignes.
    #[tracing::instrument(name = "db.relabel_source_by_url", skip(self), fields(db.system = "postgresql"))]
    pub async fn relabel_source_by_url(&self, old: &str, url: &str, new: &str) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "UPDATE legal_article SET source = $3 WHERE source = $1 AND source_url = $2",
                &[&old, &url, &new],
            )
            .await?;
        Ok(n)
    }

    /// Reclasse les traités du bulk JORF (`source='treaty'`, sans URL,
    /// `source_uid` natif `JORFARTI…`) en `source='jorf'` — la nature « traité »
    /// reste portée par `jurisdiction='INTL'`. Exclut les chaînes curées
    /// (avenants, `source_uid` propre au dataset). Renvoie le nombre de lignes.
    #[tracing::instrument(name = "db.relabel_treaty_jorf_bulk", skip(self), fields(db.system = "postgresql"))]
    pub async fn relabel_treaty_jorf_bulk(&self) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "UPDATE legal_article SET source = 'jorf' \
                 WHERE source = 'treaty' AND COALESCE(source_url, '') = '' \
                   AND source_uid LIKE 'JORFARTI%'",
                &[],
            )
            .await?;
        Ok(n)
    }

    /// Canonicalise un libellé de diffuseur (`legal_article.source`) : `from` →
    /// `to`, pour les variantes d'hôte d'un même diffuseur (ex. `jafbase.fr` →
    /// `jafbase`). Renvoie le nombre de lignes modifiées.
    #[tracing::instrument(name = "db.canonicalize_source_label", skip(self), fields(db.system = "postgresql"))]
    pub async fn canonicalize_source_label(&self, from: &str, to: &str) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "UPDATE legal_article SET source = $2 WHERE source = $1",
                &[&from, &to],
            )
            .await?;
        Ok(n)
    }

    /// Reliquat post-reclassement : lignes encore sous un `source`
    /// catégorie/méthode (URL vide, hors bulk JORF). `(source, nombre)`.
    /// Lecture seule.
    #[tracing::instrument(name = "db.count_category_source_leftovers", skip(self), fields(db.system = "postgresql"))]
    pub async fn count_category_source_leftovers(
        &self,
        sources: &[&str],
    ) -> Result<Vec<(String, i64)>> {
        let sources: Vec<String> = sources.iter().map(|s| s.to_string()).collect();
        let rows = self
            .conn
            .query(
                "SELECT source, count(*) FROM legal_article \
                 WHERE source = ANY($1) GROUP BY source ORDER BY source",
                &[&sources],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// `text_uid` des véhicules de publication : les cibles d'une arête `publie`
    /// entrante possédée par une fiche instrument. Lecture seule.
    #[tracing::instrument(name = "db.published_vehicle_uids", skip(self), fields(db.system = "postgresql"))]
    pub async fn published_vehicle_uids(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT target_text_uid FROM legal_link
                 WHERE verb = 'publie' AND direction = 'incoming'
                   AND target_text_uid IS NOT NULL",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Suppression d'articles d'un fond DILA par chemins issus des
    /// `liste_suppression_<fond>*.dat`. Le stem du chemin `.dat`
    /// (`…/LEGIARTI000….xml` / `…/KALIARTI000….xml`) est le `source_uid` à purger
    /// pour le `source` donné (`'legifrance'` LEGI/JORF, `'kali'` KALI). Renvoie
    /// le nombre de lignes supprimées.
    #[tracing::instrument(name = "db.delete_legal_articles_by_paths", skip(self, paths), fields(db.system = "postgresql", source, paths = paths.len()))]
    pub async fn delete_legal_articles_by_paths(
        &self,
        source: &str,
        paths: &[String],
    ) -> Result<u64> {
        if paths.is_empty() {
            return Ok(0);
        }
        let source_uids: Vec<String> = paths
            .iter()
            .filter_map(|p| {
                let stem = p.rsplit('/').next().unwrap_or(p);
                let id = stem.strip_suffix(".xml").unwrap_or(stem);
                if id.is_empty() {
                    None
                } else {
                    Some(id.to_string())
                }
            })
            .collect();
        if source_uids.is_empty() {
            return Ok(0);
        }
        // Purge liée des arêtes du graphe AVANT les articles : la clé owner de
        // `legal_link` se résout via la ligne d'article encore présente. Sauf `jorf` :
        // une publication n'écrit aucun lien, et son triplet est celui de la version
        // LEGI du jour de parution.
        if source != "jorf" {
            self.conn
                .execute(
                    "DELETE FROM legal_link ll USING legal_article a \
                     WHERE a.source = $1 AND a.source_uid = ANY($2) \
                       AND ll.owner_text_uid = a.text_uid AND ll.owner_num_key = a.num_key \
                       AND ll.owner_date_debut = a.date_debut",
                    &[&source, &source_uids],
                )
                .await?;
        }
        let n = self
            .conn
            .execute(
                "DELETE FROM legal_article \
                 WHERE source = $1 AND source_uid = ANY($2)",
                &[&source, &source_uids],
            )
            .await?;
        Ok(n)
    }

    /// The LEGI versions whose text is short and says `traitement`, with that
    /// text: the caller tells which ones stand in for an article not yet consolidated.
    #[tracing::instrument(name = "db.legi_placeholder_candidates", skip(self), fields(db.system = "postgresql"))]
    pub async fn legi_placeholder_candidates(&self) -> Result<Vec<(i64, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT id, texte FROM legal_article
                 WHERE texte @@@ 'traitement' AND source = 'legifrance' AND length(texte) < 40",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Deletes the article versions `ids` with their edges and citations, and reopens
    /// the JO row each one closed. Returns the versions deleted and the rows reopened.
    #[tracing::instrument(name = "db.drop_legal_article_versions", skip(self, ids), fields(db.system = "postgresql", ids = ids.len()))]
    pub async fn drop_legal_article_versions(&self, ids: &[i64]) -> Result<(i64, i64)> {
        let row = self
            .conn
            .query_one(
                "WITH gone AS (
                   DELETE FROM legal_article WHERE id = ANY($1)
                   RETURNING text_uid, num_key, date_debut),
                 links AS (
                   DELETE FROM legal_link l USING gone g
                   WHERE l.owner_text_uid = g.text_uid AND l.owner_num_key = g.num_key
                     AND l.owner_date_debut = g.date_debut),
                 legal_cites AS (
                   DELETE FROM text_legal_citation c USING gone g
                   WHERE c.owner_text_uid = g.text_uid AND c.owner_num_key = g.num_key
                     AND c.owner_date_debut = g.date_debut),
                 case_cites AS (
                   DELETE FROM text_case_citation c USING gone g
                   WHERE c.owner_text_uid = g.text_uid AND c.owner_num_key = g.num_key
                     AND c.owner_date_debut = g.date_debut),
                 reopened AS (
                   UPDATE legal_article p
                      SET date_fin = (SELECT min(n.date_debut) - 1 FROM legal_article n
                                       WHERE n.text_uid = p.text_uid AND n.num_key = p.num_key
                                         AND n.status <> 'MODIFIE_MORT_NE'
                                         AND n.date_debut > p.date_debut
                                         AND n.id <> ALL($1))
                     FROM gone g
                    WHERE p.source = 'jorf' AND p.id <> ALL($1)
                      AND p.text_uid = g.text_uid AND p.num_key = g.num_key
                      AND p.date_fin = g.date_debut - 1
                   RETURNING 1)
                 SELECT (SELECT count(*) FROM gone), (SELECT count(*) FROM reopened)",
                &[&ids],
            )
            .await?;
        Ok((row.get(0), row.get(1)))
    }

    /// Pose l'état de diffusion d'un lot de textes. Ne touche que la `nature`
    /// donnée. Renvoie le nombre de lignes modifiées ; un uid absent est ignoré.
    #[tracing::instrument(name = "db.set_legal_texts_status", skip(self, text_uids), fields(db.system = "postgresql", nature, status, uids = text_uids.len()))]
    pub async fn set_legal_texts_status(
        &self,
        nature: &str,
        text_uids: &[String],
        status: &str,
    ) -> Result<u64> {
        if text_uids.is_empty() {
            return Ok(0);
        }
        Ok(self
            .conn
            .execute(
                "UPDATE legal_text SET status = $3 \
                 WHERE nature = $1 AND text_uid = ANY($2)",
                &[&nature, &text_uids, &status],
            )
            .await?)
    }

    /// The `EU/REG` and `EU/DIR` texts with no numbered article, and their status.
    #[tracing::instrument(name = "db.eu_texts_without_articles", skip(self), fields(db.system = "postgresql"))]
    pub async fn eu_texts_without_articles(&self) -> Result<Vec<(String, Option<String>)>> {
        let rows = self
            .conn
            .query(
                "SELECT t.text_uid, t.status FROM legal_text t \
                 WHERE (t.text_uid LIKE 'EU/REG/%' OR t.text_uid LIKE 'EU/DIR/%') \
                   AND NOT EXISTS (SELECT 1 FROM legal_article a \
                                   WHERE a.text_uid = t.text_uid AND a.num <> '')",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// The `EU/REG` and `EU/DIR` texts whose `body` piece was written on or after `since`.
    pub async fn eu_bodies_written_since(&self, since: NaiveDate) -> Result<HashSet<String>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid FROM legal_article \
                 WHERE num_key = 'body' AND source = 'eu-law' AND source_asof >= $1 \
                   AND (text_uid LIKE 'EU/REG/%' OR text_uid LIKE 'EU/DIR/%')",
                &[&since],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// The `EU/REG` and `EU/DIR` texts with no piece, newest first.
    #[tracing::instrument(name = "db.eu_texts_without_body", skip(self), fields(db.system = "postgresql"))]
    pub async fn eu_texts_without_body(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT t.text_uid FROM legal_text t \
                 WHERE (t.text_uid LIKE 'EU/REG/%' OR t.text_uid LIKE 'EU/DIR/%') \
                   AND NOT EXISTS (SELECT 1 FROM legal_article a WHERE a.text_uid = t.text_uid) \
                 ORDER BY t.text_date DESC NULLS LAST",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    #[tracing::instrument(name = "db.set_texts_status", skip(self, text_uids), fields(db.system = "postgresql", status, uids = text_uids.len()))]
    pub async fn set_texts_status(
        &self,
        text_uids: &[String],
        status: Option<&str>,
    ) -> Result<u64> {
        const LOT: usize = 200;
        let mut changed = 0;
        for lot in text_uids.chunks(LOT) {
            changed += self
                .conn
                .execute(
                    "UPDATE legal_text SET status = $2 \
                     WHERE text_uid = ANY($1) AND status IS DISTINCT FROM $2",
                    &[&lot, &status],
                )
                .await?;
        }
        Ok(changed)
    }

    /// The text each NOR names, a row of another diffuser before a bulletin's.
    #[tracing::instrument(name = "db.legal_text_uids_by_nor", skip(self, nors), fields(db.system = "postgresql", nors = nors.len()))]
    pub async fn legal_text_uids_by_nor(
        &self,
        nors: &[String],
    ) -> Result<std::collections::HashMap<String, String>> {
        let rows = self
            .conn
            .query(
                "SELECT nor, text_uid FROM legal_text WHERE nor = ANY($1) \
                 ORDER BY (text_uid LIKE 'bo\\_%') DESC",
                &[&nors],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Turns `ABROGE` every text of `nature` outside `kept`, and returns how many
    /// it turned.
    #[tracing::instrument(name = "db.retire_legal_texts_except", skip(self, kept), fields(db.system = "postgresql", nature, kept = kept.len()))]
    pub async fn retire_legal_texts_except(&self, nature: &str, kept: &[String]) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "UPDATE legal_text SET status = 'ABROGE' \
                 WHERE nature = $1 AND text_uid <> ALL($2) \
                   AND status IS DISTINCT FROM 'ABROGE'",
                &[&nature, &kept],
            )
            .await?)
    }

    /// Ferme les articles d'un lot de textes à la date où le texte porteur cesse
    /// d'être en vigueur. Ne remonte jamais une fermeture déjà posée plus tôt :
    /// une version remplacée en cours de vie garde la sienne. Une date future
    /// se pose telle quelle — la fenêtre de validité la fera tomber le jour dit.
    /// Jamais avant le début de la version : la ligne ne serait plus joignable à
    /// aucune date, aucune ne satisfaisant `date_debut <= D` et `date_fin >= D`.
    #[tracing::instrument(name = "db.close_legal_articles", skip(self, closures), fields(db.system = "postgresql", textes = closures.len()))]
    pub async fn close_legal_articles(&self, closures: &[(String, NaiveDate)]) -> Result<u64> {
        // In batches: the BM25 index charges every row, and a batch stays under
        // the `statement_timeout`. Later passes write nothing.
        const LOT: usize = 200;
        let mut total = 0;
        for lot in closures.chunks(LOT) {
            let (uids, fins): (Vec<String>, Vec<NaiveDate>) = lot.iter().cloned().unzip();
            total += self
                .conn
                .execute(
                    "UPDATE legal_article a SET date_fin = c.fin \
                       FROM unnest($1::text[], $2::date[]) AS c(uid, fin) \
                      WHERE a.text_uid = c.uid \
                        AND c.fin >= a.date_debut \
                        AND (a.date_fin IS NULL OR a.date_fin > c.fin)",
                    &[&uids, &fins],
                )
                .await?;
        }
        Ok(total)
    }

    /// Every `EU/…` fiche of the catalogue, body-carrying or not: the namespace
    /// an act is minted into, read whole to keep the minting from opening a
    /// second identity for an act already there.
    pub async fn eu_text_uids(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid FROM legal_text WHERE text_uid LIKE 'EU/%'",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// DILA fiches of European nature whose title carries the act number, with
    /// no piece: the JO announces the act, its content lives at
    /// EUR-Lex. `(text_uid, title, nature)`, oldest first.
    pub async fn jorf_eu_shells(&self) -> Result<Vec<(String, String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT t.text_uid, t.title, t.nature FROM legal_text t \
                  WHERE t.text_uid LIKE 'JORFTEXT%' \
                    AND t.nature IN ('DIRECTIVE_EURO', 'REGLEMENTEUROPEEN', 'REGLEMENT_EURO') \
                    AND t.title ~ '^(Directive|Règlement)[^0-9]{0,40}[0-9]{2,4}/[0-9]{1,4}' \
                    AND NOT EXISTS (SELECT 1 FROM legal_article a WHERE a.text_uid = t.text_uid) \
                  ORDER BY t.date_texte NULLS LAST, t.text_uid",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// Pairs of `EU/` fiches under swapped number and year, an act held twice.
    /// Both sides come back whole, the caller deciding which one the CELEX names.
    pub async fn eu_uid_twins(&self) -> Result<Vec<(EuTwin, EuTwin)>> {
        let rows = self
            .conn
            .query(
                "WITH e AS (
                     SELECT text_uid,
                            split_part(text_uid, '/', 2) AS kind,
                            split_part(split_part(text_uid, '/', 3), '-', 1) AS num,
                            split_part(split_part(text_uid, '/', 3), '-', 2) AS an
                       FROM legal_text WHERE text_uid LIKE 'EU/%'
                 ), a AS (
                     SELECT text_uid,
                            regexp_replace(
                                min(source_url) FILTER (WHERE source_url LIKE '%/celex/%'),
                                '^.*/celex/', '') AS celex,
                            count(*) AS articles
                       FROM legal_article WHERE text_uid LIKE 'EU/%' GROUP BY text_uid
                 )
                 SELECT x.text_uid, ax.celex, coalesce(ax.articles, 0),
                        y.text_uid, ay.celex, coalesce(ay.articles, 0)
                   FROM e x
                   JOIN e y ON y.kind = x.kind AND y.num = x.an AND y.an = x.num
                           AND x.text_uid < y.text_uid
                   LEFT JOIN a ax ON ax.text_uid = x.text_uid
                   LEFT JOIN a ay ON ay.text_uid = y.text_uid
                  ORDER BY x.text_uid",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    EuTwin {
                        text_uid: r.get(0),
                        celex: r.get(1),
                        articles: r.get(2),
                    },
                    EuTwin {
                        text_uid: r.get(3),
                        celex: r.get(4),
                        articles: r.get(5),
                    },
                )
            })
            .collect())
    }

    /// EU fund in the DB, one row per text holding `eu-law`/`eur-lex` articles:
    /// `(text_uid, celex, versions, title, nature, jurisdiction, date_texte)`,
    /// the CELEX read off `legal_article.source_url`. Two figures say where the
    /// sync stopped and neither does alone: the CELEX of the most recent
    /// version, and how many version rows the act holds. Before its series was
    /// loaded, an act's single row already carried the latest CELEX.
    #[allow(clippy::type_complexity)]
    pub async fn eu_law_registry(
        &self,
    ) -> Result<
        Vec<(
            String,
            String,
            i64,
            String,
            String,
            String,
            Option<NaiveDate>,
        )>,
    > {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT ON (t.text_uid)
                        t.text_uid, regexp_replace(a.source_url, '^.*/celex/', ''),
                        count(*) OVER (PARTITION BY t.text_uid),
                        t.title, t.nature, t.jurisdiction, t.date_texte
                 FROM legal_article a
                 JOIN legal_text t USING (text_uid)
                 WHERE a.source IN ('eu-law', 'eur-lex') AND a.source_url LIKE '%/celex/%'
                 ORDER BY t.text_uid, a.date_debut DESC",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.get(0),
                    r.get(1),
                    r.get(2),
                    r.get(3),
                    r.get(4),
                    r.get(5),
                    r.get(6),
                )
            })
            .collect())
    }

    /// Page keyset de toutes les versions d'articles à corps, ordonnée par la PK
    /// `(text_uid, num_key, date_debut)` : les versions d'un même texte arrivent
    /// groupées. `after` = dernière clé de la page précédente
    /// (`("", "", 0001-01-01)` pour la première).
    pub async fn legal_article_versions_page(
        &self,
        after: (&str, &str, NaiveDate),
        limit: i64,
    ) -> Result<Vec<(String, String, NaiveDate, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, num_key, date_debut, texte \
                 FROM legal_article \
                 WHERE (text_uid, num_key, date_debut) > ($1, $2, $3) \
                   AND texte IS NOT NULL \
                 ORDER BY text_uid, num_key, date_debut \
                 LIMIT $4",
                &[&after.0, &after.1, &after.2, &limit],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
            .collect())
    }

    /// Corps de la version la plus récente de chaque `(text_uid, num_key)`
    /// demandé, avec son `date_debut`. Une clé sans version à corps est absente du
    /// résultat.
    pub async fn legal_article_latest_bodies(
        &self,
        keys: &[(String, String)],
    ) -> Result<Vec<(String, String, String, NaiveDate)>> {
        let uids: Vec<&str> = keys.iter().map(|(u, _)| u.as_str()).collect();
        let nums: Vec<&str> = keys.iter().map(|(_, n)| n.as_str()).collect();
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT ON (a.text_uid, a.num_key) \
                     a.text_uid, a.num_key, a.texte, a.date_debut \
                 FROM legal_article a \
                 JOIN unnest($1::text[], $2::text[]) AS k(uid, num) \
                   ON k.uid = a.text_uid AND k.num = a.num_key \
                 WHERE a.texte IS NOT NULL \
                 ORDER BY a.text_uid, a.num_key, a.date_debut DESC",
                &[&uids, &nums],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
            .collect())
    }

    /// Purge des articles d'un texte hors snapshot courant : supprime les lignes
    /// du `source`/`text_uid` dont la version n'est pas `keep_date` ou dont le
    /// `num_key` a disparu du document. Les arêtes `legal_link` des lignes purgées
    /// partent d'abord.
    #[tracing::instrument(name = "db.delete_legal_articles_versions_except", skip(self, keep_num_keys), fields(db.system = "postgresql", source, text_uid))]
    pub async fn delete_legal_articles_versions_except(
        &self,
        source: &str,
        text_uid: &str,
        keep_date: chrono::NaiveDate,
        keep_num_keys: &[String],
    ) -> Result<u64> {
        self.conn
            .execute(
                "DELETE FROM legal_link ll USING legal_article a \
                 WHERE a.source = $1 AND a.text_uid = $2 \
                   AND (a.date_debut <> $3 OR a.num_key <> ALL($4)) \
                   AND ll.owner_text_uid = a.text_uid AND ll.owner_num_key = a.num_key \
                   AND ll.owner_date_debut = a.date_debut",
                &[&source, &text_uid, &keep_date, &keep_num_keys],
            )
            .await?;
        let n = self
            .conn
            .execute(
                "DELETE FROM legal_article \
                 WHERE source = $1 AND text_uid = $2 \
                   AND (date_debut <> $3 OR num_key <> ALL($4))",
                &[&source, &text_uid, &keep_date, &keep_num_keys],
            )
            .await?;
        Ok(n)
    }

    /// Purges the rows of a text a load replaces: those of its own `sources`, and every
    /// version still open. A closed version another source wrote, a curated reading
    /// merged into the text, stays. Returns the number of rows deleted.
    #[tracing::instrument(name = "db.delete_replaced_legal_articles", skip(self), fields(db.system = "postgresql"))]
    pub async fn delete_replaced_legal_articles(
        &self,
        text_uid: &str,
        sources: &[&str],
    ) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "DELETE FROM legal_article \
                 WHERE text_uid = $1 AND (source = ANY($2) OR date_fin IS NULL)",
                &[&text_uid, &sources],
            )
            .await?;
        Ok(n)
    }

    /// Collapse des doublons diffuseur : supprime les coquilles `LEGITEXT` vides
    /// (0 article) dont un autre texte porte le corps sous le même `title_key`.
    /// Ne touche jamais un texte cité (`ref_text_uid`) ni un texte porteur
    /// d'articles. Renvoie le nombre de coquilles collapsées.
    #[tracing::instrument(name = "db.collapse_empty_legitext_doublons", skip(self), fields(db.system = "postgresql"))]
    pub async fn collapse_empty_legitext_doublons(&self) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "
                DELETE FROM legal_text t
                WHERE t.text_uid LIKE 'LEGITEXT%'
                  AND t.jurisdiction = 'FR'
                  AND NOT EXISTS (
                        SELECT 1 FROM legal_article a WHERE a.text_uid = t.text_uid)
                  AND EXISTS (
                        SELECT 1 FROM legal_text b
                        JOIN legal_article ba ON ba.text_uid = b.text_uid
                        WHERE b.text_uid <> t.text_uid
                          AND lower(b.title_key) = lower(t.title_key))
                  AND NOT EXISTS (
                        SELECT 1 FROM legal_citation c
                        WHERE public.lj_cit_terms(c.spans) @> ARRAY[t.text_uid])
                ",
                &[],
            )
            .await?;
        Ok(n)
    }

    /// Classifies the roles of a batch of texts: (individuel, habilitation, vehicule, modificatif).
    #[tracing::instrument(name = "db.classify_text_roles_scoped", skip_all, fields(db.system = "postgresql", uids = uids.len()))]
    pub async fn classify_text_roles_scoped(
        &self,
        uids: &[String],
    ) -> Result<(u64, u64, u64, u64)> {
        if uids.is_empty() {
            return Ok((0, 0, 0, 0));
        }
        self.run_text_role_sqls(Some(uids)).await
    }

    /// Rôles des textes publiés, backfill pleine-table ; le régime établi passe
    /// par [`Self::classify_text_roles_scoped`]. Recalcule tout. Renvoie
    /// (individuel, habilitation, vehicule, modificatif).
    #[tracing::instrument(name = "db.backfill_text_roles", skip(self), fields(db.system = "postgresql"))]
    pub async fn backfill_text_roles(&self) -> Result<(u64, u64, u64, u64)> {
        self.run_text_role_sqls(None).await
    }

    /// Runs the six statements of [`text_role_sqls`] (reset + 5 passes), scoped to
    /// `uids` if given, in one transaction: a role reset then given back nets out
    /// at the commit, where the articles take their copies. Returns
    /// (individuel, habilitation, vehicule + vehicule_publie, modificatif).
    async fn run_text_role_sqls(&self, uids: Option<&[String]>) -> Result<(u64, u64, u64, u64)> {
        self.conn.batch_execute("BEGIN").await?;
        let counted: Result<[u64; 6]> = async {
            let mut counts = [0u64; 6];
            for (i, sql) in text_role_sqls(uids.is_some()).iter().enumerate() {
                counts[i] = match uids {
                    Some(uids) => self.conn.execute(sql.as_str(), &[&uids]).await?,
                    None => self.conn.execute(sql.as_str(), &[]).await?,
                };
            }
            Ok(counts)
        }
        .await;
        match counted {
            Ok(c) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok((c[1], c[2], c[3] + c[4], c[5]))
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    /// Aligne `legal_link.verb` sur le repli verbe/nom courant de
    /// `lj_extract::legi::lien_verb`. Renvoie le nombre de lignes alignées.
    #[tracing::instrument(name = "db.normalize_link_verbs", skip(self), fields(db.system = "postgresql"))]
    pub async fn normalize_link_verbs(&self) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "
                UPDATE legal_link SET verb = m.verb
                FROM (VALUES
                        ('RATIFIE', 'ratifie'), ('RATIFICATION', 'ratifie'),
                        ('DENONCE', 'denonce'), ('DENONCIATION', 'denonce'),
                        ('ANNULE', 'annule'), ('ANNULATION', 'annule'),
                        ('DISJOINT', 'disjoint'), ('DISJONCTION', 'disjoint'),
                        ('ETEND', 'etend'), ('EXTENSION', 'etend'),
                        ('RECTIFIE', 'rectifie'), ('TRANSPOSITION', 'transpose')
                     ) AS m (typelien, verb)
                WHERE legal_link.typelien = m.typelien
                  AND legal_link.verb <> m.verb
                ",
                &[],
            )
            .await?;
        Ok(n)
    }

    /// Article servi à une date. `date` absente ⇒ la version dans la fenêtre de
    /// validité du jour, sinon la dernière. Sinon, la version dont l'intervalle
    /// `[date_debut, date_fin]` couvre `date` (la plus récente en cas de
    /// chevauchement) ; `date_debut` est NOT NULL (sentinelle '0001-01-01' = borne
    /// ouverte basse). À date égale la version consolidée prime sur la publication
    /// au JO (`(source='jorf') ASC`). `None` si aucune version ne s'applique.
    /// Résolution vers l'identité `(text_uid, num_key)`, la version choisie ici par
    /// la date.
    #[tracing::instrument(name = "db.law_article_at_date", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_article_at_date(
        &self,
        text_uid: &str,
        num_key: &str,
        date: Option<NaiveDate>,
    ) -> Result<Option<LegalArticleRow>> {
        let row = match date {
            Some(d) => {
                self.conn
                    .query_opt(
                        "
                        SELECT text_uid, num, num_key, position, title_path, status,
                               date_debut, date_fin, texte, nota, content_checksum,
                               source, source_uid, source_url,
                               texte_original, lang_original, translation,
                               source_asof, source_upstream_url
                        FROM legal_article
                        WHERE text_uid = $1 AND num_key = $2 AND num <> ''
                          AND date_debut <= $3
                          AND (date_fin IS NULL OR date_fin >= $3)
                        ORDER BY date_debut DESC, (source = 'jorf') ASC
                        LIMIT 1
                        ",
                        &[&text_uid, &num_key, &d],
                    )
                    .await?
            }
            None => {
                let valid_a = article_valid_now("a");
                let norm_a = article_is_norm("a");
                self.conn
                    .query_opt(
                        &format!(
                            "
                        SELECT text_uid, num, num_key, position, title_path, status,
                               date_debut, date_fin, texte, nota, content_checksum,
                               source, source_uid, source_url,
                               texte_original, lang_original, translation,
                               source_asof, source_upstream_url
                        FROM legal_article a
                        WHERE text_uid = $1 AND num_key = $2 AND num <> ''
                        ORDER BY ({valid_a}) DESC, ({norm_a}) DESC,
                                 (source = 'jorf') ASC, date_debut DESC NULLS LAST
                        LIMIT 1
                        "
                        ),
                        &[&text_uid, &num_key],
                    )
                    .await?
            }
        };
        Ok(row.map(legal_article_row_from_row))
    }

    /// Dernière version commencée au plus tard à `date` : le texte tel qu'il a
    /// cessé de s'appliquer, quand `date` est postérieure à l'abrogation. Ses
    /// bornes disent seules l'écart, l'appelant doit donc les servir avec.
    #[tracing::instrument(name = "db.law_article_last_version_before", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_article_last_version_before(
        &self,
        text_uid: &str,
        num_key: &str,
        date: NaiveDate,
    ) -> Result<Option<LegalArticleRow>> {
        let row = self
            .conn
            .query_opt(
                "
                SELECT text_uid, num, num_key, position, title_path, status,
                       date_debut, date_fin, texte, nota, content_checksum,
                       source, source_uid, source_url,
                       texte_original, lang_original, translation,
                       source_asof, source_upstream_url
                FROM legal_article
                WHERE text_uid = $1 AND num_key = $2 AND num <> '' AND date_debut <= $3
                ORDER BY date_debut DESC, (source = 'jorf') ASC
                LIMIT 1
                ",
                &[&text_uid, &num_key, &date],
            )
            .await?;
        Ok(row.map(legal_article_row_from_row))
    }

    /// Lecture « à la suite » : les articles du texte `text_uid`, version en
    /// vigueur à `date` (ou `VIGUEUR` si `date` absente), triés par `position`
    /// (ordre de lecture réel, ≠ tri lexical). `position` NULL trié en dernier ;
    /// fallback `num_key`. Champs bruts.
    #[tracing::instrument(name = "db.law_text_articles", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_text_articles(
        &self,
        text_uid: &str,
        date: Option<NaiveDate>,
    ) -> Result<Vec<LegalArticleRow>> {
        let rows = match date {
            Some(d) => {
                self.conn
                    .query(
                        "
                        SELECT text_uid, num, num_key, position, title_path, status,
                               date_debut, date_fin, texte, nota, content_checksum,
                               source, source_uid, source_url,
                               texte_original, lang_original, translation,
                               source_asof, source_upstream_url
                        FROM (
                          SELECT DISTINCT ON (num_key) *
                          FROM legal_article
                          WHERE text_uid = $1 AND date_debut <= $2
                            AND (date_fin IS NULL OR date_fin >= $2)
                          ORDER BY num_key, date_debut DESC, (source = 'jorf') ASC
                        ) v
                        ORDER BY position NULLS LAST, num_key
                        ",
                        &[&text_uid, &d],
                    )
                    .await?
            }
            None => {
                let valid_a = article_valid_now("a");
                let norm_a = article_is_norm("a");
                self.conn
                    .query(
                        &format!(
                            "
                        SELECT text_uid, num, num_key, position, title_path, status,
                               date_debut, date_fin, texte, nota, content_checksum,
                               source, source_uid, source_url,
                               texte_original, lang_original, translation,
                               source_asof, source_upstream_url
                        FROM (
                          -- Fenêtre du jour préférée, sinon dernière version ;
                          -- vraie version avant moignon, consolidée avant
                          -- publication.
                          SELECT DISTINCT ON (num_key) *
                          FROM legal_article a
                          WHERE text_uid = $1
                          ORDER BY num_key, ({valid_a}) DESC, ({norm_a}) DESC,
                                   (source = 'jorf') ASC, date_debut DESC NULLS LAST
                        ) v
                        ORDER BY position NULLS LAST, num_key
                        "
                        ),
                        &[&text_uid],
                    )
                    .await?
            }
        };
        Ok(rows.into_iter().map(legal_article_row_from_row).collect())
    }

    /// Timeline des versions d'un article, triée par `date_debut` croissante.
    /// `source_uid` = identifiant natif de la version (LEGIARTI…). Dates rendues
    /// en ISO `String` (`::text`).
    #[tracing::instrument(name = "db.law_article_versions", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_article_versions(
        &self,
        text_uid: &str,
        num_key: &str,
    ) -> Result<Vec<LawVersionRow>> {
        let rows = self
            .conn
            .query(
                "
                SELECT source_uid, status, date_debut::text, date_fin::text
                FROM legal_article
                WHERE text_uid = $1 AND num_key = $2 AND num <> ''
                ORDER BY date_debut
                ",
                &[&text_uid, &num_key],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| LawVersionRow {
                source_uid: r.get(0),
                status: r.get(1),
                date_debut: r.get(2),
                date_fin: r.get(3),
            })
            .collect())
    }

    /// A text's summary: its `legal_text` metadata, the count of its numbered
    /// articles over every version, and its body: its `body` piece beside
    /// numbered articles, else its unnumbered pieces in published order. `None`
    /// for an unknown text.
    #[tracing::instrument(name = "db.law_code_summary", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_code_summary(&self, text_uid: &str) -> Result<Option<LawCodeSummaryRow>> {
        let row = self
            .conn
            .query_opt(
                "
                SELECT t.text_uid, t.slug, t.title, t.nature, t.last_modified::text,
                       -- Every version: an abrogated text shows its articles,
                       -- not an empty summary.
                       (SELECT count(DISTINCT a.num_key)
                        FROM legal_article a
                        WHERE a.text_uid = t.text_uid AND a.num <> '') AS article_count,
                       t.upcoming_versions,
                       CASE WHEN EXISTS (SELECT 1 FROM legal_article n
                                         WHERE n.text_uid = t.text_uid
                                           AND n.num <> '')
                       THEN (SELECT a.texte FROM legal_article a
                             WHERE a.text_uid = t.text_uid AND a.num_key = 'body'
                             ORDER BY a.date_debut DESC LIMIT 1)
                       ELSE (SELECT string_agg(a.texte, E'\n\n'
                                               ORDER BY a.position NULLS LAST, a.num_key)
                             FROM legal_article a
                             WHERE a.text_uid = t.text_uid AND a.num = '')
                       END AS body,
                       t.status, t.nor, t.date_texte::text, t.jurisdiction
                FROM legal_text t
                WHERE t.text_uid = $1
                ",
                &[&text_uid],
            )
            .await?;
        Ok(row.map(|r| LawCodeSummaryRow {
            text_uid: r.get(0),
            slug: r.get(1),
            title: r.get(2),
            nature: r.get(3),
            last_modified: r.get(4),
            article_count: r.get(5),
            upcoming_versions: r.get(6),
            body: r.get(7),
            status: r.get(8),
            nor: r.get(9),
            date_texte: r.get(10),
            jurisdiction: r.get(11),
        }))
    }

    /// Pose les dates de versions futures d'un texte. Écrit toujours, y compris
    /// vide (un incrément qui vide `VERSIONS_A_VENIR` vide la colonne).
    pub async fn set_legal_text_upcoming_versions(
        &self,
        text_uid: &str,
        dates: &[NaiveDate],
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE legal_text SET upcoming_versions = $2 WHERE text_uid = $1",
                &[&text_uid, &dates],
            )
            .await?;
        Ok(())
    }

    /// Résout un slug de code en identité de texte — `text_uid`, **slug
    /// canonique** et titre humain.
    ///
    /// `slug` = la chaîne d'URL `/norm/{juris}/{slug}`. Un slug retiré par un renommage
    /// rend le texte visé et son slug d'aujourd'hui, l'appelant lisant l'écart
    /// pour poser un 301. `None` si le slug n'est ni vivant ni redirigé → 404.
    ///
    /// Slug et titre voyagent avec l'uid : la page article a besoin des trois
    /// (en-tête « Article N du <titre> », URL canonique), et c'est la même ligne.
    #[tracing::instrument(name = "db.referential_ref", skip(self), fields(db.system = "postgresql"))]
    pub async fn referential_ref(
        &self,
        slug: &str,
        jurisdiction: Option<&str>,
    ) -> Result<Option<ReferentialRef>> {
        // Two indexed branches rather than an `OR` over a join: the planner
        // cannot index `t.slug = $1 OR t.text_uid = r.text_uid` and falls back to
        // a seq scan, on the hot path of every `/texte` page. `rang` makes the
        // live slug win when a string is both live and redirected.
        //
        // With no jurisdiction, France wins: `code-civil` lives under several of
        // them since the country suffix left the slugs, and the URLs indexed
        // under the shape that carried none all name a French text.
        let row = self
            .conn
            .query_opt(
                "SELECT text_uid, slug, title, jurisdiction FROM ( \
                   SELECT t.text_uid, t.slug, t.title, t.jurisdiction, 0 AS rang \
                     FROM legal_text t WHERE t.slug = $1 \
                       AND ($2::text IS NULL OR t.jurisdiction = $2) \
                   UNION ALL \
                   SELECT t.text_uid, t.slug, t.title, t.jurisdiction, 1 \
                     FROM legal_text_slug_redirect r \
                     JOIN legal_text t ON t.text_uid = r.text_uid \
                    WHERE r.ancien_slug = $1 \
                      AND ($2::text IS NULL OR t.jurisdiction = $2) \
                 ) c ORDER BY rang, (jurisdiction <> 'FR') LIMIT 1",
                &[&slug, &jurisdiction],
            )
            .await?;
        Ok(row.map(|r| (r.get(0), r.get(1), r.get(2), r.get(3))))
    }

    /// Resolves an instrument value (slug, free name or acronym) to its catalogue
    /// identity: **the only entry point** of the `/texte` alphabet, shared by the
    /// search filters, the `code` parameter and the bench that measures it.
    /// Four steps:
    ///
    /// 1. the slug as is, the normal case;
    /// 2. a BOFiP id (`BOI-RSA-CHAMP-20-40-10-20`), the document's `text_uid`;
    /// 3. the usual name from the curated alias dataset (« cgi », « loi du 6
    ///    juillet 1989 »): when several texts answer to the same name, usage
    ///    decides, and no string derives it;
    /// 4. **fallback** on the « did you mean » candidates
    ///    ([`lj_norm::instrument_match::pick`]), which demands a single target.
    ///
    /// A caller who writes the real name of the text is right; answering
    /// « unknown » sends them to correct what is correct.
    ///
    /// The slug returned is **today's**, never the one received: a caller who
    /// keeps the old name of a renamed text still resolves, and the URLs built
    /// behind it do not go through a redirect.
    #[tracing::instrument(name = "db.resolve_instrument", skip(self), fields(db.system = "postgresql"))]
    pub async fn resolve_instrument(
        &self,
        value: &str,
        jurisdiction: Option<&str>,
    ) -> Result<InstrumentResolution> {
        let canonique = |r: Option<(String, String, Option<String>, String)>| {
            r.map(
                |(text_uid, slug, _, jurisdiction)| InstrumentResolution::Direct {
                    slug,
                    text_uid,
                    jurisdiction,
                },
            )
        };
        if let Some(hit) = canonique(
            self.referential_ref(&lj_fold::slug(value, '-'), jurisdiction)
                .await?,
        ) {
            return Ok(hit);
        }
        if let Some(text_uid) = lj_norm::text_identity::bofip_uid(value) {
            if let Some((slug, jurisdiction)) = self.text_ref_by_uid(&text_uid).await? {
                return Ok(InstrumentResolution::Direct {
                    slug,
                    text_uid,
                    jurisdiction,
                });
            }
        }
        if let Some(alias) = lj_norm::aliases::canonical_slug(value) {
            if let Some(hit) = canonique(self.referential_ref(alias, jurisdiction).await?) {
                return Ok(hit);
            }
        }
        if let Some(text_uid) = lj_norm::aliases::canonical_text_uid(value) {
            if let Some((slug, jurisdiction)) = self.text_ref_by_uid(text_uid).await? {
                return Ok(InstrumentResolution::Direct {
                    slug,
                    text_uid: text_uid.to_string(),
                    jurisdiction,
                });
            }
        }
        let candidates = self
            .suggest_instruments(value, RELAX_CANDIDATES)
            .await
            .unwrap_or_default();
        let paires: Vec<(&str, &str)> = candidates
            .iter()
            .map(|c| (c.slug.as_str(), c.title.as_str()))
            .collect();
        match lj_norm::instrument_match::pick(value, &paires) {
            Some((i, regle)) => Ok(InstrumentResolution::Rattrape {
                slug: candidates[i].slug.clone(),
                text_uid: candidates[i].text_uid.clone(),
                jurisdiction: candidates[i].jurisdiction.clone(),
                regle,
            }),
            None => Ok(InstrumentResolution::Inconnu { candidates }),
        }
    }

    /// Codes de juridiction portant un tiret. Doit rendre vide : le rattrapage
    /// MCP qui remplace le tiret par le souligné tient sa légitimité de là — si
    /// l'alphabet mélange les deux, un code tiré absent du référentiel n'est
    /// plus une faute de frappe et le rattrapage vise à côté.
    #[tracing::instrument(name = "db.jurisdictions_au_code_tire", skip(self), fields(db.system = "postgresql"))]
    pub async fn jurisdictions_au_code_tire(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query("SELECT code FROM jurisdiction WHERE code LIKE '%-%'", &[])
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Parmi les slugs donnés, ceux qu'aucun texte ne porte **aujourd'hui**. Un
    /// slug seulement redirigé est fautif : il résout encore, donc rien ne casse
    /// et rien ne se voit, jusqu'au jour où la redirection est nettoyée. Sert à
    /// confronter au catalogue une liste écrite à la main.
    #[tracing::instrument(name = "db.slugs_sans_texte_vivant", skip(self, slugs), fields(db.system = "postgresql"))]
    pub async fn slugs_sans_texte_vivant(&self, slugs: &[String]) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT s FROM unnest($1::text[]) AS s \
                  WHERE NOT EXISTS (SELECT 1 FROM legal_text t WHERE t.slug = s)",
                &[&slugs],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Sentinelles doublonnées : la ligne restée sur '0001-01-01' alors qu'une
    /// ligne datée porte le même `(text_uid, num_key, source_uid)` — la même
    /// version, écrite deux fois.
    const SENTINELLES_DOUBLONS: &'static str = "SELECT a.ctid FROM legal_article a \
         WHERE a.date_debut = DATE '0001-01-01' \
           AND EXISTS (SELECT 1 FROM legal_article b \
                        WHERE b.text_uid = a.text_uid AND b.num_key = a.num_key \
                          AND b.source_uid = a.source_uid \
                          AND b.date_debut <> DATE '0001-01-01')";

    /// DILA versions held at more than one start date of the same article: a
    /// version has one start, so every row but one is a date the source has since
    /// moved. Driven by the `source_uid` index rather than a window over the table.
    const VERSIONS_REDATEES: &'static str =
        "SELECT a.source_uid, a.text_uid, a.num_key, a.date_debut \
         FROM legal_article a \
         WHERE a.source_uid IN (SELECT source_uid FROM legal_article \
                                 WHERE source_uid ~ '^(LEGI|KALI|JORF)ARTI' \
                                 GROUP BY source_uid HAVING count(*) > 1) \
           AND a.date_debut <> DATE '0001-01-01' \
           AND EXISTS (SELECT 1 FROM legal_article b \
                        WHERE b.source_uid = a.source_uid AND b.text_uid = a.text_uid \
                          AND b.num_key = a.num_key AND b.date_debut <> DATE '0001-01-01' \
                          AND b.date_debut <> a.date_debut)";

    /// Versions qu'une version postérieure du même article n'a pas fermées : leur
    /// fenêtre de validité déborde sur celle de leur successeur, donc deux textes
    /// se disent applicables le même jour. Les mort-nées sont hors du jeu, ni
    /// fermeuses ni à fermer : jamais applicables, leur fenêtre inversée est la
    /// donnée de la source.
    const VERSIONS_NON_FERMEES: &'static str = "SELECT v.ctid, v.suivante \
         FROM (SELECT ctid, date_fin, \
                      lead(date_debut) OVER (PARTITION BY text_uid, num_key \
                                             ORDER BY date_debut) AS suivante \
                 FROM legal_article \
                WHERE status <> 'MODIFIE_MORT_NE') v \
         WHERE v.suivante IS NOT NULL \
           AND (v.date_fin IS NULL OR v.date_fin >= v.suivante)";

    /// Counts the misplaced article versions, writing nothing: `(doubled
    /// sentinels, DILA versions held at several start dates, versions no next one
    /// closes)`. Read by the invariant check; the repair is
    /// [`Self::situer_versions_articles`], and `redate-dila-versions` for the
    /// redated versions, which only the archive settles.
    #[tracing::instrument(name = "db.versions_mal_situees", skip(self), fields(db.system = "postgresql"))]
    ///
    /// One query per count: joined into one `SELECT` of scalar subqueries they
    /// exceed the pool's `statement_timeout`, where each holds within it.
    pub async fn versions_mal_situees(&self) -> Result<(u64, u64, u64)> {
        let compter = |q: &str| {
            let sql = format!("SELECT count(*) FROM ({q}) x");
            async move {
                let row = self.conn.query_one(&sql, &[]).await?;
                Ok::<u64, tokio_postgres::Error>(row.get::<_, i64>(0) as u64)
            }
        };
        let sentinelles = compter(Self::SENTINELLES_DOUBLONS).await?;
        let redatees = self
            .conn
            .query_one(
                &format!(
                    "SELECT count(DISTINCT source_uid) FROM ({}) x",
                    Self::VERSIONS_REDATEES
                ),
                &[],
            )
            .await?
            .get::<_, i64>(0) as u64;
        let non_fermees = compter(Self::VERSIONS_NON_FERMEES).await?;
        Ok((sentinelles, redatees, non_fermees))
    }

    /// The rows of the DILA versions held at more than one start date, for the
    /// ids starting with `prefix` (`LEGIARTI`): `(source_uid, text_uid, num_key,
    /// date_debut)`.
    #[tracing::instrument(name = "db.versions_redatees", skip(self), fields(db.system = "postgresql"))]
    pub async fn versions_redatees(
        &self,
        prefix: &str,
    ) -> Result<Vec<(String, String, String, NaiveDate)>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT * FROM ({}) x WHERE starts_with(x.source_uid, $1)",
                    Self::VERSIONS_REDATEES
                ),
                &[&prefix],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
            .collect())
    }

    /// Counts versions whose end precedes their start, without writing: no date
    /// satisfies both `date_debut <= D` and `date_fin >= D`, so the row leaves
    /// every dated read. Stillborns are out of scope, their inverted window
    /// being how the source says "never applicable", which puts the expected
    /// count at zero.
    #[tracing::instrument(name = "db.versions_intervalle_inverse", skip(self), fields(db.system = "postgresql"))]
    pub async fn versions_intervalle_inverse(&self) -> Result<u64> {
        let row = self
            .conn
            .query_one(
                "SELECT count(*) FROM legal_article \
                  WHERE date_fin IS NOT NULL AND date_fin < date_debut \
                    AND status <> 'MODIFIE_MORT_NE'",
                &[],
            )
            .await?;
        Ok(row.get::<_, i64>(0) as u64)
    }

    /// Situe chaque version d'article entre ses voisines, pleine table : purge la
    /// sentinelle doublonnée, rouvre les fins posées contre une mort-née ou
    /// inversées, puis ferme toute version la veille de sa vraie suivante.
    /// Filet de dérive, pas régime : l'upsert le fait déjà à l'écriture, donc un
    /// compteur non nul dit qu'un écrivain a fui.
    ///
    /// In batches: the table carries a BM25 index, and the catch-up of the
    /// historical stock touches much of it; one transaction of that size would
    /// hold the lock and bloat the index faster than autovacuum can follow.
    #[tracing::instrument(name = "db.situer_versions_articles", skip(self), fields(db.system = "postgresql"))]
    pub async fn situer_versions_articles(&self) -> Result<(u64, u64)> {
        const LOT: i64 = 50_000;
        let mut purgees = 0;
        loop {
            let n = self
                .conn
                .execute(
                    &format!(
                        "DELETE FROM legal_article a USING ({} LIMIT {LOT}) c \
                          WHERE a.ctid = c.ctid",
                        Self::SENTINELLES_DOUBLONS
                    ),
                    &[],
                )
                .await?;
            purgees += n;
            if n == 0 {
                break;
            }
        }
        let mut reopened_stillborn = 0;
        loop {
            let n = self
                .conn
                .execute(
                    &format!(
                        "UPDATE legal_article a SET date_fin = NULL \
                           FROM (SELECT a2.ctid FROM legal_article a2 \
                                   JOIN legal_article m \
                                     ON m.text_uid = a2.text_uid AND m.num_key = a2.num_key \
                                  WHERE m.status = 'MODIFIE_MORT_NE' \
                                    AND a2.status <> 'MODIFIE_MORT_NE' \
                                    AND a2.date_fin = m.date_debut - 1 \
                                  LIMIT {LOT}) f \
                          WHERE a.ctid = f.ctid"
                    ),
                    &[],
                )
                .await?;
            reopened_stillborn += n;
            if n == 0 {
                break;
            }
        }
        let mut reopened_inverted = 0;
        loop {
            let n = self
                .conn
                .execute(
                    &format!(
                        "UPDATE legal_article a SET date_fin = NULL \
                           FROM (SELECT ctid FROM legal_article \
                                  WHERE date_fin IS NOT NULL AND date_fin < date_debut \
                                    AND status <> 'MODIFIE_MORT_NE' \
                                  LIMIT {LOT}) f \
                          WHERE a.ctid = f.ctid"
                    ),
                    &[],
                )
                .await?;
            reopened_inverted += n;
            if n == 0 {
                break;
            }
        }
        if reopened_stillborn + reopened_inverted > 0 {
            tracing::info!(
                reopened_stillborn,
                reopened_inverted,
                "ends reopened before reclosing on the real next one"
            );
        }
        let mut fermees = 0;
        loop {
            let n = self
                .conn
                .execute(
                    &format!(
                        "UPDATE legal_article a SET date_fin = f.suivante - 1 \
                           FROM ({} LIMIT {LOT}) f WHERE a.ctid = f.ctid",
                        Self::VERSIONS_NON_FERMEES
                    ),
                    &[],
                )
                .await?;
            fermees += n;
            if n == 0 {
                break;
            }
        }
        Ok((purgees, fermees))
    }

    /// Of the given uids, those naming no text with something to read: a uid does
    /// not redirect like a slug, it designates or it does not.
    #[tracing::instrument(name = "db.uids_sans_texte", skip(self, uids), fields(db.system = "postgresql"))]
    pub async fn uids_sans_texte(&self, uids: &[String]) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT u FROM unnest($1::text[]) AS u \
                      WHERE NOT EXISTS (SELECT 1 FROM legal_text t \
                                        WHERE t.text_uid = u AND {TEXT_READABLE_SQL})"
                ),
                &[&uids],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// The distinct `(live slug, article key)` a Légifrance article id
    /// designates, the key empty for an unnumbered piece, which reads as its
    /// text. The versions of one article share their id; past one pair the
    /// caller refuses rather than pick, so three are enough.
    #[tracing::instrument(name = "db.article_refs_by_source_uid", skip(self), fields(db.system = "postgresql"))]
    pub async fn article_refs_by_source_uid(
        &self,
        source_uid: &str,
    ) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT t.slug, CASE WHEN a.num = '' THEN '' ELSE a.num_key END \
                   FROM legal_article a \
                   JOIN legal_text t ON t.text_uid = a.text_uid \
                  WHERE a.source_uid = $1 LIMIT 3",
                &[&source_uid],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// `(slug, jurisdiction)` of a Légifrance text id (`LEGITEXT…`, `JORFTEXT…`).
    #[tracing::instrument(name = "db.text_ref_by_uid", skip(self), fields(db.system = "postgresql"))]
    pub async fn text_ref_by_uid(&self, text_uid: &str) -> Result<Option<(String, String)>> {
        let row = self
            .conn
            .query_opt(
                "SELECT slug, jurisdiction FROM legal_text WHERE text_uid = $1",
                &[&text_uid],
            )
            .await?;
        Ok(row.map(|r| (r.get(0), r.get(1))))
    }

    /// `(slug, jurisdiction)` of the texts whose table of contents holds the
    /// section `cid` (`LEGISCTA…`). Bounded at three: past one, only the
    /// ambiguity matters.
    #[tracing::instrument(name = "db.section_texts_by_cid", skip(self), fields(db.system = "postgresql"))]
    pub async fn section_texts_by_cid(&self, cid: &str) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT t.slug, t.jurisdiction FROM legal_toc_edge e \
                   JOIN legal_text t ON t.text_uid = e.text_uid \
                  WHERE e.child_cid = $1 LIMIT 3",
                &[&cid],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Suggestions pour une valeur d'instrument inconnue : candidats par BM25 sur
    /// le titre, puis classement par similarité trigramme tolérante aux fautes
    /// (pg_trgm), plein-champ sur le slug et le titre.
    ///
    /// Two floors: `similarity()` scales with the length of both strings, so a
    /// short common name against a long title falls under it whatever it
    /// designates, « loi du 6 juillet 1989 » scoring lower on the 89-462 than an
    /// unrelated law of 1943. `strict_word_similarity` measures the best anchoring
    /// of the need *inside* the title and catches those rows; it saturates on
    /// unrelated texts, so it opens the floor without ever carrying the ranking.
    ///
    /// BM25 carries the selection because `similarity()` is not indexable. The
    /// hyphen becomes a space: the title tokenizer keeps hyphens, so a whole slug
    /// would match no term. Bounded to 200 candidates, past which the trigram
    /// ranking no longer changes the first five.
    ///
    /// Le numéro hyphéné reste un token indivisible du titre : `loi 71 1130` ne
    /// matche pas `71-1130`, qui n'entrait donc dans aucun des 200 candidats.
    /// D'où la disjonction sur le groupe numérique, et le départage sur sa
    /// position dans le slug, qui place le texte avant ceux qui le modifient
    /// (7 contre 54). Sans groupe numérique, les deux sont inertes.
    ///
    /// L'uid voyage avec le slug : l'appelant qui rattrape une valeur sur un
    /// candidat en a besoin comme filtre, et c'est la même ligne.
    #[tracing::instrument(name = "db.suggest_instruments", skip(self), fields(db.system = "postgresql"))]
    pub async fn suggest_instruments(
        &self,
        needle: &str,
        limit: i64,
    ) -> Result<Vec<InstrumentCandidate>> {
        let rows = self
            .conn
            .query(
                "WITH cand AS ( \
                   SELECT id, slug, title, text_uid, jurisdiction FROM legal_text \
                    WHERE id @@@ paradedb.match('title', replace($1, '-', ' ')) \
                       OR ($1 ~ '[0-9]+-[0-9]+' \
                           AND id @@@ paradedb.match('title', substring($1 from '[0-9]+-[0-9]+'))) \
                    ORDER BY paradedb.score(id) DESC LIMIT 200) \
                 SELECT slug, title, text_uid, jurisdiction FROM cand \
                  WHERE slug IS NOT NULL \
                    AND (GREATEST(similarity($1, slug), similarity($1, title)) > 0.4 \
                     OR strict_word_similarity($1, title) > 0.4) \
                  ORDER BY coalesce(nullif(position(substring($1 from '[0-9]+-[0-9]+') in slug), 0), 9999), \
                  GREATEST(similarity($1, slug), similarity($1, title)) DESC, \
                  length(title), title LIMIT $2",
                &[&needle, &limit],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| InstrumentCandidate {
                slug: r.get(0),
                title: r.get(1),
                text_uid: r.get(2),
                jurisdiction: r.get(3),
            })
            .collect())
    }

    /// The texts of the catalogue carrying `key`, best named by `needle` first.
    /// The key selects, the whole needle ranks: the same number lives in ten
    /// codes and only the code's name tells them apart.
    ///
    /// `num` is written glued (« L761-2 ») or detached (« L. 761-2 ») depending
    /// on the text, one token or two in the index, so both spellings enter as
    /// alternatives; `num_key` outside the scan then holds the exactness, which
    /// inside it costs a heap recheck.
    ///
    /// A number alone leaves the score nothing to rank on but title length, and
    /// « 700 » then answers with the Senegalese, Malagasy and Central African
    /// codes of civil procedure before the French one. `legal_article_usage`
    /// names the identities case law really cites, and they pass first.
    #[tracing::instrument(name = "db.suggest_articles", skip(self), fields(db.system = "postgresql"))]
    /// Trigram on `num_key` alone, unlike its BM25 neighbour: an article key is
    /// too short to yield a useful token, and the candidates are already bounded
    /// to one text by the `(text_uid, num_key)` index.
    #[tracing::instrument(name = "db.article_keys_near", skip(self), fields(db.system = "postgresql"))]
    pub async fn article_keys_near(
        &self,
        text_uid: &str,
        key: &str,
        limit: i64,
    ) -> Result<(Vec<(String, String)>, i64)> {
        let rows = self
            .conn
            .query(
                "SELECT num_key, num, count(*) OVER () AS total \
                   FROM legal_article \
                  WHERE text_uid = $1 AND date_fin IS NULL AND num <> '' \
                  ORDER BY similarity(num_key, $2) DESC, num_key \
                  LIMIT $3",
                &[&text_uid, &key, &limit],
            )
            .await?;
        let total = rows.first().map(|r| r.get::<_, i64>("total")).unwrap_or(0);
        Ok((rows.iter().map(|r| (r.get(0), r.get(1))).collect(), total))
    }

    pub async fn suggest_articles(
        &self,
        key: &str,
        needle: &str,
        limit: i64,
    ) -> Result<Vec<ArticleCandidate>> {
        let detache = lj_norm::article_key::spaced_part(key).unwrap_or_else(|| key.to_string());
        let rows = self
            .conn
            .query(
                "WITH cand AS ( \
                   SELECT a.text_uid, a.num_key, a.num, paradedb.score(a.id) AS s \
                     FROM legal_article a \
                    WHERE a.id @@@ paradedb.boolean( \
                            must => ARRAY[paradedb.boolean(should => ARRAY[ \
                              paradedb.match('num', $1), \
                              paradedb.match('num', $2, conjunction_mode => true)], \
                              minimum_should_match => 1)], \
                            should => ARRAY[paradedb.match('search_title', $3)]) \
                    ORDER BY paradedb.score(a.id) DESC LIMIT 400), \
                 best AS ( \
                   SELECT DISTINCT ON (text_uid) text_uid, num_key, num, s \
                     FROM cand WHERE num_key = $1 ORDER BY text_uid, s DESC) \
                 SELECT b.text_uid, b.num_key, b.num, t.title, t.slug \
                   FROM best b JOIN legal_text t ON t.text_uid = b.text_uid \
                   LEFT JOIN legal_article_usage u \
                     ON u.text_uid = b.text_uid AND u.num_key = b.num_key \
                  ORDER BY (u.text_uid IS NULL), b.s DESC, length(t.title), t.title \
                  LIMIT $4",
                &[&key, &detache, &needle, &limit],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| ArticleCandidate {
                text_uid: r.get(0),
                num_key: r.get(1),
                num: r.get(2),
                title: r.get(3),
                slug: r.get(4),
            })
            .collect())
    }

    /// Décisions citant un article : backlinks depuis les blobs
    /// `legal_citation.spans`. Paginée sous cap de fenêtre (`offset + limit ≤ 100`,
    /// tenu par la validation d'entrée). Champs bruts.
    ///
    /// Deux plans selon le volume de citantes (`citing_decision_counts`) : bitmap
    /// GIN et tri portée puis date pour un article courant, marche descendante de
    /// `idx_decisions_date_lecture` pour un article ultra-cité
    /// (≥ [`CITING_RECENCY_WALK_MIN_COUNT`]), qui rend la récence seule.
    ///
    /// Bornée à la fenêtre de validité `[date_debut, date_fin)` de la version
    /// servie. `date_fin` `None` = version en vigueur.
    #[tracing::instrument(name = "db.law_decisions_citing", skip(self), fields(db.system = "postgresql"))]
    #[allow(clippy::too_many_arguments)]
    pub async fn law_decisions_citing(
        &self,
        ref_text_uid: &str,
        num_key: &str,
        date_debut: NaiveDate,
        date_fin: Option<NaiveDate>,
        limit: i64,
        offset: i64,
        lang: &str,
    ) -> Result<Vec<CitingDecisionRow>> {
        let citing_count: i64 = self
            .conn
            .query_opt(
                "SELECT decision_count FROM citing_decision_counts \
                 WHERE cited_term = $1 || '|' || $2",
                &[&ref_text_uid, &num_key],
            )
            .await?
            .map(|r| r.get(0))
            .unwrap_or(0);
        let params: [&(dyn ToSql + Sync); 7] = [
            &ref_text_uid,
            &num_key,
            &limit,
            &offset,
            &date_debut,
            &date_fin,
            &lang,
        ];
        let rows = if citing_count >= CITING_RECENCY_WALK_MIN_COUNT {
            // L'EXISTS sur le blob n'est pas indexable : `d.date_lecture >= $5` exclut les
            // NULL et le scan arrière de l'index date sert l'ORDER BY sans tri.
            self.conn
                .query(
                    &format!(
                        "
                    SELECT d.id, d.public_id, d.jurisdiction_type,
                           j.label, d.date_lecture::text, d.docket_numbers,
                           d.publication_codes, {summary}, d.jurisdiction_code, {bounds} AS summary_bounds
                    FROM decisions d
                    JOIN legal_citation lc ON lc.decision_id = d.id
                    LEFT JOIN jurisdiction j ON j.code = d.jurisdiction_code
                    WHERE d.date_lecture >= $5
                      AND ($6::date IS NULL OR d.date_lecture < $6)
                      AND EXISTS (
                          SELECT 1 FROM jsonb_array_elements(lc.spans) AS el
                          WHERE el->>2 = $1 AND el->>3 = $2)
                    ORDER BY d.date_lecture DESC, d.id
                    LIMIT $3 OFFSET $4
                    ",
                        summary = super::pick::summary("d", "$7"),
                        bounds = super::pick::summary_bounds("d", "$7")
                    ),
                    &params,
                )
                .await?
        } else {
            // Autorité d'abord, puis date décroissante. Les paliers sont des
            // constantes du code, inlinées, pas des données.
            let rank_expr = authority_rank_sql("d.publication_codes");
            self.conn
                .query(
                    &format!(
                        "
                        SELECT d.id, d.public_id, d.jurisdiction_type,
                               j.label, d.date_lecture::text, d.docket_numbers,
                               d.publication_codes, {summary},
                               {rank_expr} AS authority_rank,
                               d.jurisdiction_code, {bounds} AS summary_bounds
                        FROM legal_citation lc
                        JOIN decisions d ON d.id = lc.decision_id
                        LEFT JOIN jurisdiction j ON j.code = d.jurisdiction_code
                        WHERE public.lj_cit_terms(lc.spans) @> ARRAY[$1 || '|' || $2]
                          -- fenêtre de validité [date_debut, date_fin) de la version
                          -- servie ; date_fin NULL = en vigueur (borne haute ouverte).
                          AND d.date_lecture >= $5
                          AND ($6::date IS NULL OR d.date_lecture < $6)
                        -- Texte ISO 'YYYY-MM-DD' → tri == chronologique. Une ligne
                        -- blob par décision : pas de doublons à dédupliquer.
                        ORDER BY authority_rank, d.date_lecture::text DESC NULLS LAST, d.id
                        LIMIT $3 OFFSET $4
                        ",
                        summary = super::pick::summary("d", "$7"),
                        bounds = super::pick::summary_bounds("d", "$7")
                    ),
                    &params,
                )
                .await?
        };
        Ok(rows
            .iter()
            .map(|r| {
                let (summary_quote, summary_body) = super::pick::bounds(r.get("summary_bounds"));
                CitingDecisionRow {
                    id: r.get(0),
                    public_id: r.get(1),
                    jurisdiction_type: r.get(2),
                    jurisdiction_name: r.get(3),
                    jurisdiction_code: r.get("jurisdiction_code"),
                    date_lecture: r.get(4),
                    docket_numbers: r.get(5),
                    publication_codes: r.get(6),
                    summary: r.get(7),
                    summary_quote,
                    summary_body,
                    cited_num_keys: Vec::new(),
                }
            })
            .collect())
    }

    /// Fil d'Ariane TOC d'une version d'article : les divisions enclosantes, de la
    /// racine à la section directe (`label`, `child_cid`). Marche remontante depuis
    /// l'arête de la version servie ; en cas d'arêtes multiples au même niveau, une
    /// seule chaîne est retenue. Vide si la version n'est pas dans la TOC.
    #[tracing::instrument(name = "db.article_toc_breadcrumb", skip(self), fields(db.system = "postgresql"))]
    pub async fn article_toc_breadcrumb(
        &self,
        text_uid: &str,
        article_uid: &str,
    ) -> Result<Vec<(String, Option<String>)>> {
        let rows = self
            .conn
            .query(
                "
                WITH RECURSIVE anchor AS (
                    SELECT e.owner_uid FROM legal_toc_edge e
                    WHERE e.text_uid = $1 AND e.child_kind = 'article'
                      AND e.child_uid = $2
                    ORDER BY e.seq LIMIT 1
                ), up AS (
                    SELECT e.owner_uid, e.label, e.child_cid, 0 AS d
                    FROM legal_toc_edge e
                    JOIN anchor a ON e.child_uid = a.owner_uid
                    UNION ALL
                    SELECT e.owner_uid, e.label, e.child_cid, up.d + 1
                    FROM legal_toc_edge e
                    JOIN up ON e.child_uid = up.owner_uid
                    WHERE up.d < 12
                )
                SELECT DISTINCT ON (d) label, child_cid FROM up
                ORDER BY d DESC
                ",
                &[&text_uid, &article_uid],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Articles co-cités avec `(ref_text_uid, num_key)` dans les décisions :
    /// échantillon des décisions citant l'article (500), puis agrégat de leurs
    /// autres citations pondéré tf-idf saturé — `n/(n+20) × ln(N / df)`, `df` depuis
    /// `citing_decision_counts` et `N` de l'estimation planner de `decisions`.
    /// Seuil ≥ 3 co-occurrences ; le compte affiché reste brut.
    #[tracing::instrument(name = "db.law_co_cited_articles", skip(self), fields(db.system = "postgresql"))]
    pub async fn law_co_cited_articles(
        &self,
        ref_text_uid: &str,
        num_key: &str,
        limit: i64,
    ) -> Result<Vec<CoCitedArticleRow>> {
        let rows = self
            .conn
            .query(
                "
                WITH citing AS (
                    SELECT decision_id FROM legal_citation
                    WHERE public.lj_cit_terms(spans) @> ARRAY[$1 || '|' || $2]
                    LIMIT 500
                ), co AS (
                    SELECT el->>2 AS ref_text_uid, el->>3 AS ref_num_key,
                           count(DISTINCT lc.decision_id) AS n
                    FROM legal_citation lc
                    JOIN citing c ON c.decision_id = lc.decision_id
                    CROSS JOIN LATERAL jsonb_array_elements(lc.spans) AS el
                    WHERE el->>3 IS NOT NULL
                      AND (el->>2 <> $1 OR el->>3 <> $2)
                    GROUP BY 1, 2
                    HAVING count(DISTINCT lc.decision_id) >= 3
                )
                SELECT co.ref_num_key, co.n, t.slug, t.title, t.jurisdiction
                FROM co
                JOIN legal_text t ON t.text_uid = co.ref_text_uid
                -- df absent (terme jamais recompté — article tout neuf) :
                -- repli df = n_co, le plus conservateur des connus.
                LEFT JOIN citing_decision_counts s
                       ON s.cited_term = co.ref_text_uid || '|' || co.ref_num_key
                ORDER BY (co.n::float8 / (co.n + 20)) * ln(
                             (SELECT greatest(reltuples, 1)::float8 FROM pg_class
                              WHERE oid = 'decisions'::regclass)
                             / greatest(coalesce(s.decision_count, co.n), 1)::float8
                         ) DESC,
                         co.ref_num_key
                LIMIT $3
                ",
                &[&ref_text_uid, &num_key, &limit],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| CoCitedArticleRow {
                num_key: r.get(0),
                count: r.get(1),
                text_slug: r.get(2),
                text_title: r.get(3),
                text_jurisdiction: r.get(4),
            })
            .collect())
    }

    /// Iterates `(slug, num, lastmod)` over the articles in force (sitemaps
    /// `/norm/{juris}/{slug}/{num}`). One row per URL, not per version: the
    /// `DISTINCT ON` keeps the version `law_article_at_date` serves. `lastmod` =
    /// `COALESCE(t.last_modified, a.date_debut)`, nullable and unfiltered: the DILA
    /// sentinels come up as is, `plausible_lastmod` on the `lj-ingest` side decides
    /// to publish or omit. Order `(slug, num)`; no SQL pagination.
    #[tracing::instrument(name = "db.iter_referential_for_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_referential_for_sitemap(&self) -> Result<Vec<SitemapArticle>> {
        let (valid_a, norm_a, pick_a) = (
            article_valid_now("a"),
            article_is_norm("a"),
            article_pick_order("a"),
        );
        let rows = self
            .conn
            .query(
                &format!(
                    "
                SELECT DISTINCT ON (t.slug, a.num_key)
                       t.slug, a.num_key, t.jurisdiction,
                       COALESCE(t.last_modified, a.date_debut)::date AS lastmod
                FROM legal_article a
                JOIN legal_text t ON t.text_uid = a.text_uid
                WHERE {valid_a} AND {norm_a} AND a.num <> '' AND t.slug IS NOT NULL
                ORDER BY t.slug, a.num_key, {pick_a}
                "
                ),
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
            .collect())
    }

    /// `(slug, jurisdiction, lastmod)` of the `/norm/{juris}/{slug}` pages, with
    /// no filter on nature or role: an article URL's parent page is declared
    /// with it. A text with no piece in force is left out, its page holding
    /// only its title and metadata. `lastmod` is the raw `t.last_modified`,
    /// sentinels included; ordered by slug, `build_sitemaps` pages in memory.
    #[tracing::instrument(name = "db.iter_texts_for_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_texts_for_sitemap(&self) -> Result<Vec<(String, String, Option<NaiveDate>)>> {
        let (valid_a, norm_a) = (article_valid_now("a"), article_is_norm("a"));
        let rows = self
            .conn
            .query(
                &format!(
                    "
                SELECT t.slug, t.jurisdiction, t.last_modified::date AS lastmod
                FROM legal_text t
                WHERE t.slug IS NOT NULL
                  AND EXISTS (SELECT 1 FROM legal_article a
                              WHERE a.text_uid = t.text_uid
                                AND {valid_a} AND {norm_a})
                ORDER BY t.slug
                "
                ),
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// Rewrites the `legal_text` copies `legal_article` carries, for the articles
    /// where they differ. The triggers keep them at every write; this realigns
    /// what two concurrent writers of a text and its articles left behind.
    #[tracing::instrument(name = "db.refresh_article_denorm", skip(self, textes), fields(db.system = "postgresql", portee = textes.map_or(-1, |t| t.len() as i64)))]
    pub async fn refresh_article_denorm(&self, textes: Option<&[String]>) -> Result<u64> {
        if textes.is_some_and(|t| t.is_empty()) {
            return Ok(0);
        }
        self.conn.batch_execute("BEGIN").await?;
        let updated: Result<u64> = async {
            self.conn
                .batch_execute("SET LOCAL statement_timeout = 0")
                .await?;
            let portee = match textes {
                Some(_) => " AND a.text_uid = ANY($1)",
                None => "",
            };
            let sql = format!(
                "UPDATE legal_article a SET \
                   code_title = t.title, \
                   jurisdiction = t.jurisdiction, \
                   nature = upper(t.nature), \
                   slug = t.slug, \
                   searchable = {ARTICLE_SEARCHABLE_SQL}, \
                   text_date = t.text_date, \
                   jurisdiction_paths = t.jurisdiction_paths \
                 FROM legal_text t \
                 WHERE a.text_uid = t.text_uid{portee} \
                   AND (a.code_title IS DISTINCT FROM t.title \
                     OR a.jurisdiction IS DISTINCT FROM t.jurisdiction \
                     OR a.nature IS DISTINCT FROM upper(t.nature) \
                     OR a.slug IS DISTINCT FROM t.slug \
                     OR a.searchable IS DISTINCT FROM {ARTICLE_SEARCHABLE_SQL} \
                     OR a.text_date IS DISTINCT FROM t.text_date \
                     OR a.jurisdiction_paths IS DISTINCT FROM t.jurisdiction_paths)"
            );
            let n = match textes {
                Some(t) => self.conn.execute(&sql, &[&t]).await?,
                None => self.conn.execute(&sql, &[]).await?,
            };
            Ok(n)
        }
        .await;
        match updated {
            Ok(n) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(n)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    /// Counts the articles whose `legal_text` copies disagree with their parent.
    /// The scoped refresh cannot see a drift no pass touched, so this is the probe
    /// that says the scope has a hole; it must always answer zero.
    pub async fn denorm_derive(&self) -> Result<i64> {
        let row = self
            .conn
            .query_one(
                &format!(
                    "SELECT count(*) FROM legal_article a JOIN legal_text t USING (text_uid) \
                  WHERE a.code_title IS DISTINCT FROM t.title \
                     OR a.jurisdiction IS DISTINCT FROM t.jurisdiction \
                     OR a.nature IS DISTINCT FROM upper(t.nature) \
                     OR a.slug IS DISTINCT FROM t.slug \
                     OR a.searchable IS DISTINCT FROM {ARTICLE_SEARCHABLE_SQL} \
                     OR a.text_date IS DISTINCT FROM t.text_date \
                     OR a.jurisdiction_paths IS DISTINCT FROM t.jurisdiction_paths"
                ),
                &[],
            )
            .await?;
        Ok(row.get(0))
    }

    /// Restamps the date and the jurisdiction paths of the named texts from their
    /// nature and dates, for the writers that change those in SQL. Returns the
    /// texts rewritten.
    pub async fn stamp_legal_texts(&self, text_uids: &[String]) -> Result<u64> {
        let rows = self
            .conn
            .query(
                &format!("{STAMP_SELECT} WHERE text_uid = ANY($1)"),
                &[&text_uids],
            )
            .await?;
        Ok(self.write_stamps(&rows).await?.len() as u64)
    }

    /// Restamps up to `limit` texts past `after_id`, in id order. Returns the last
    /// id read, `None` past the end, the uids read and how many were rewritten.
    pub async fn stamp_legal_texts_after(
        &self,
        after_id: i64,
        limit: i64,
    ) -> Result<(Option<i64>, Vec<String>, usize)> {
        let rows = self
            .conn
            .query(
                &format!("{STAMP_SELECT} WHERE id > $1 ORDER BY id LIMIT $2"),
                &[&after_id, &limit],
            )
            .await?;
        let last = rows.last().map(|r| r.get::<_, i64>("id"));
        let read = rows.iter().map(|r| r.get("text_uid")).collect();
        Ok((last, read, self.write_stamps(&rows).await?.len()))
    }

    /// Gives an instrument with no date of its own the earliest publication date of
    /// the decrees publishing it, among `vehicles` or the whole stock. Returns the
    /// instruments dated, restamped.
    pub async fn date_instruments_from_vehicles(
        &self,
        vehicles: Option<&[String]>,
    ) -> Result<Vec<String>> {
        let scope = match vehicles {
            Some(_) => "l.target_text_uid = ANY($1) AND",
            None => "",
        };
        let sql = format!(
            "UPDATE legal_text i SET date_publi = v.date_publi \
               FROM (SELECT l.owner_text_uid AS uid, min(t.date_publi) AS date_publi \
                       FROM legal_link l JOIN legal_text t ON t.text_uid = l.target_text_uid \
                      WHERE {scope} l.verb = 'publie' AND l.direction = 'incoming' \
                        AND t.date_publi >= DATE '1500-01-01' AND t.date_publi < DATE '2100-01-01' \
                      GROUP BY 1) v \
              WHERE i.text_uid = v.uid AND i.date_texte IS NULL AND i.date_publi IS NULL \
              RETURNING i.text_uid"
        );
        let rows = match vehicles {
            Some(v) => self.conn.query(&sql, &[&v]).await?,
            None => self.conn.query(&sql, &[]).await?,
        };
        let uids: Vec<String> = rows.iter().map(|r| r.get(0)).collect();
        self.stamp_legal_texts(&uids).await?;
        Ok(uids)
    }

    /// Gives a JORF text with no publication date the date of the JO issue that
    /// carried it. Returns the texts dated, restamped.
    pub async fn date_texts_from_publications(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "UPDATE legal_text t SET date_publi = p.date_publi \
                   FROM text_publication p \
                  WHERE p.text_uid = t.text_uid AND p.status = 'authentic' \
                    AND t.date_publi IS NULL \
                    AND p.date_publi >= DATE '1500-01-01' AND p.date_publi < DATE '2100-01-01' \
                  RETURNING t.text_uid",
                &[],
            )
            .await?;
        let uids: Vec<String> = rows.iter().map(|r| r.get(0)).collect();
        self.stamp_legal_texts(&uids).await?;
        Ok(uids)
    }

    /// Every INTL text, with the keywords of the decrees that publish it.
    pub async fn treaty_titles(&self) -> Result<Vec<TreatyTitle>> {
        let rows = self
            .conn
            .query(
                "SELECT t.text_uid, coalesce(t.title, ''),
                        coalesce((SELECT array_agg(DISTINCT k ORDER BY k)
                                    FROM legal_link l
                                    JOIN text_publication p ON p.text_uid = l.target_text_uid
                                    CROSS JOIN unnest(p.keywords) k
                                   WHERE l.owner_text_uid = t.text_uid AND l.verb = 'publie'
                                     AND l.direction = 'incoming'), '{}'),
                        t.text_date
                   FROM legal_text t
                  WHERE t.jurisdiction = 'INTL'",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| TreatyTitle {
                text_uid: r.get(0),
                title: r.get(1),
                keywords: r.get(2),
                date: r.get(3),
            })
            .collect())
    }

    /// Writes the parties of each text, `None` when unknown, to the rows whose
    /// list changes. Returns how many it wrote.
    pub async fn write_treaty_parties(&self, rows: &[(String, Option<Vec<&str>>)]) -> Result<u64> {
        let (uids, parties): (Vec<&str>, Vec<Option<String>>) = rows
            .iter()
            .map(|(uid, p)| (uid.as_str(), p.as_ref().map(|p| p.join(","))))
            .unzip();
        Ok(self
            .conn
            .execute(
                "UPDATE legal_text t SET parties = string_to_array(v.parties, ',')
                   FROM unnest($1::text[], $2::text[]) AS v(text_uid, parties)
                  WHERE t.text_uid = v.text_uid
                    AND t.parties IS DISTINCT FROM string_to_array(v.parties, ',')",
                &[&uids, &parties],
            )
            .await?)
    }

    /// Writes the date of texts that carry none. Returns the texts dated,
    /// restamped.
    pub async fn fill_text_dates(&self, dates: &[(String, NaiveDate)]) -> Result<Vec<String>> {
        let (uids, days): (Vec<&str>, Vec<NaiveDate>) =
            dates.iter().map(|(u, d)| (u.as_str(), *d)).unzip();
        let rows = self
            .conn
            .query(
                "UPDATE legal_text t SET date_texte = v.date_texte \
                   FROM unnest($1::text[], $2::date[]) AS v(text_uid, date_texte) \
                  WHERE t.text_uid = v.text_uid AND t.date_texte IS NULL \
                  RETURNING t.text_uid",
                &[&uids, &days],
            )
            .await?;
        let dated: Vec<String> = rows.iter().map(|r| r.get(0)).collect();
        self.stamp_legal_texts(&dated).await?;
        Ok(dated)
    }

    async fn write_stamps(&self, rows: &[tokio_postgres::Row]) -> Result<Vec<String>> {
        let (mut uids, mut dates, mut paths) = (Vec::new(), Vec::new(), Vec::new());
        for r in rows {
            let date = text_date(r.get("date_texte"), r.get("date_publi"));
            let p = jurisdiction_paths(r.get("text_uid"), r.get("jurisdiction"), r.get("nature"));
            let stored: Option<Vec<String>> = r.get("jurisdiction_paths");
            if r.get::<_, Option<NaiveDate>>("text_date") != date || stored.as_ref() != Some(&p) {
                uids.push(r.get::<_, String>("text_uid"));
                dates.push(date);
                paths.push(p.join(","));
            }
        }
        if !uids.is_empty() {
            self.conn
                .execute(
                    "UPDATE legal_text t SET text_date = v.text_date, \
                       jurisdiction_paths = string_to_array(v.paths, ',') \
                     FROM unnest($1::text[], $2::date[], $3::text[]) AS v(text_uid, text_date, paths) \
                     WHERE t.text_uid = v.text_uid",
                    &[&uids, &dates, &paths],
                )
                .await?;
        }
        Ok(uids)
    }

    /// Recalcule l'ordre de lecture des textes sales : ceux dont au moins un
    /// article a `reading_rank IS NULL`. Seul écrivain d'un rang ; le chemin
    /// d'écriture ne fait qu'invalider. Boucle par lots jusqu'à épuisement. Renvoie
    /// (textes traités, lignes écrites).
    #[tracing::instrument(name = "db.refresh_reading_ranks", skip(self), fields(db.system = "postgresql"))]
    pub async fn refresh_reading_ranks(&self) -> Result<(u64, u64)> {
        /// Lot de textes par aller-retour.
        const BATCH: i64 = 500;
        let (mut texts, mut rows) = (0u64, 0u64);
        loop {
            self.conn.batch_execute("BEGIN").await?;
            let batch: Result<(u64, u64)> = async {
                self.conn
                    .batch_execute("SET LOCAL statement_timeout = 0")
                    .await?;
                self.refresh_reading_ranks_batch(BATCH).await
            }
            .await;
            let (batch_texts, batch_rows) = match batch {
                Ok(v) => {
                    self.conn.batch_execute("COMMIT").await?;
                    v
                }
                Err(e) => {
                    let _ = self.conn.batch_execute("ROLLBACK").await;
                    return Err(e);
                }
            };
            if batch_texts == 0 {
                return Ok((texts, rows));
            }
            texts += batch_texts;
            rows += batch_rows;
            tracing::debug!(texts, rows, "reading ranks");
        }
    }

    /// Un lot du finisseur : les `limit` premiers textes sales, recalculés et
    /// réécrits. Renvoie (textes, lignes écrites) ; `(0, 0)` quand plus rien n'est
    /// sale.
    async fn refresh_reading_ranks_batch(&self, limit: i64) -> Result<(u64, u64)> {
        let uids: Vec<String> = self
            .conn
            .query(
                "SELECT DISTINCT text_uid FROM legal_article
                     WHERE reading_rank IS NULL LIMIT $1",
                &[&limit],
            )
            .await?
            .iter()
            .map(|r| r.get(0))
            .collect();
        if uids.is_empty() {
            return Ok((0, 0));
        }
        // Version canonique de chaque article : le rang est une propriété de
        // `(text_uid, num_key)`, partagée par ses versions.
        let (valid_a, norm_a) = (article_valid_now("a"), article_is_norm("a"));
        let inputs = self
            .conn
            .query(
                &format!(
                    "SELECT DISTINCT ON (text_uid, num_key)
                            text_uid, num_key, title_path, position, num = '' AS piece
                     FROM legal_article a
                     WHERE text_uid = ANY($1)
                     ORDER BY text_uid, num_key, ({valid_a}) DESC, ({norm_a}) DESC,
                              (source = 'jorf') ASC, date_debut DESC NULLS LAST"
                ),
                &[&uids],
            )
            .await?;
        let mut per_text: std::collections::HashMap<String, Vec<ReadingOrderInput>> =
            std::collections::HashMap::new();
        for r in &inputs {
            per_text
                .entry(r.get(0))
                .or_default()
                .push(ReadingOrderInput {
                    num_key: r.get(1),
                    title_path: r.get(2),
                    position: r.get(3),
                    piece: r.get(4),
                });
        }
        // Ordre structurel réel quand le texte a un arbre ingéré : la séquence DILA
        // est l'autorité. Anchor `owner_uid`, `text_uid` porté le long de la récursion
        // pour traiter tout le lot en une requête.
        let structural = self
            .conn
            .query(
                "WITH RECURSIVE tree AS (
                   SELECT e.owner_uid AS root, e.child_kind, e.child_uid,
                          e.child_num_key, ARRAY[e.seq] AS path, 1 AS depth
                   FROM legal_toc_edge e
                   WHERE e.owner_uid = ANY($1)
                     AND e.date_debut <= current_date
                     AND (e.date_fin IS NULL OR e.date_fin >= current_date)
                   UNION ALL
                   SELECT t.root, c.child_kind, c.child_uid, c.child_num_key,
                          t.path || c.seq, t.depth + 1
                   FROM tree t JOIN legal_toc_edge c ON c.owner_uid = t.child_uid
                   WHERE t.child_kind = 'section' AND t.depth < 32
                     AND c.date_debut <= current_date
                     AND (c.date_fin IS NULL OR c.date_fin >= current_date)
                 )
                 SELECT * FROM (
                   SELECT t.root,
                          coalesce(t.child_num_key,
                                   (SELECT a.num_key FROM legal_article a
                                    WHERE a.source_uid = t.child_uid AND a.text_uid = t.root
                                    LIMIT 1)) AS num_key,
                          row_number() OVER (PARTITION BY t.root ORDER BY t.path) AS rn
                   FROM tree t WHERE t.child_kind = 'article'
                 ) o WHERE num_key IS NOT NULL",
                &[&uids],
            )
            .await?;
        let mut structural_order: std::collections::HashMap<
            String,
            std::collections::HashMap<String, i64>,
        > = std::collections::HashMap::new();
        for r in &structural {
            structural_order
                .entry(r.get(0))
                .or_default()
                .insert(r.get(1), r.get(2));
        }

        let (mut u, mut k, mut ranks) = (Vec::new(), Vec::new(), Vec::new());
        for (text_uid, entries) in per_text {
            let tree = structural_order.get(&text_uid);
            let (mut pieces, mut entries): (Vec<_>, Vec<_>) = entries
                .into_iter()
                .partition(|e| e.piece && !tree.is_some_and(|o| o.contains_key(&e.num_key)));
            pieces.sort_by_key(|e| (e.position.is_none(), e.position));
            // Repli d'abord : il ordonne les articles absents de l'arbre du jour
            // (abrogés, transférés, versions futures), qui se rangent derrière.
            lj_norm::article_order::sort_text_reading_order(
                &mut entries,
                |e| &e.num_key,
                |e| e.title_path.as_deref(),
                |e| e.position,
            );
            if let Some(order) = tree {
                entries.sort_by_key(|e| order.get(&e.num_key).copied().unwrap_or(i64::MAX));
            }
            let printed = tree.is_none() && entries.iter().all(|e| e.position.is_some());
            let entries =
                lj_norm::article_order::place_pieces(entries, pieces, printed, |e| e.position);
            for (rank, e) in entries.into_iter().enumerate() {
                u.push(text_uid.clone());
                k.push(e.num_key);
                ranks.push(rank as i32);
            }
        }
        let written = self
            .conn
            .execute(
                "UPDATE legal_article a SET reading_rank = m.rank
                     FROM unnest($1::text[], $2::text[], $3::int[])
                          AS m(text_uid, num_key, rank)
                     WHERE a.text_uid = m.text_uid AND a.num_key = m.num_key
                       AND a.reading_rank IS DISTINCT FROM m.rank",
                &[&u, &k, &ranks],
            )
            .await?;
        Ok((uids.len() as u64, written))
    }

    /// The `text_uid` of a named text, among the visible texts with an article in
    /// force; for a shared slug or ambiguous patterns, the one with the most wins.
    pub async fn resolve_named_text(
        &self,
        target: lj_norm::aliases::NamedTextTarget,
    ) -> Result<Option<String>> {
        use lj_norm::aliases::NamedTextTarget as T;
        let (valid_a, norm_a) = (article_valid_now("a"), article_is_norm("a"));
        let sql = |cond: &str| {
            format!(
                "
                SELECT t.text_uid
                FROM legal_text t
                WHERE {cond}
                  AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                  AND {TEXT_ROLE_VISIBLE_SQL}
                  AND EXISTS (SELECT 1 FROM legal_article a
                              WHERE a.text_uid = t.text_uid
                                AND {valid_a} AND {norm_a})
                ORDER BY (SELECT count(*) FROM legal_article a
                          WHERE a.text_uid = t.text_uid
                            AND {valid_a} AND {norm_a}) DESC
                LIMIT 1
                "
            )
        };
        let rows = match &target {
            T::Uid(uid) => self.conn.query(&sql("t.text_uid = $1"), &[uid]).await?,
            T::Slug(slug) => self.conn.query(&sql("t.slug = $1"), &[slug]).await?,
            T::SlugPatterns(pats) => {
                self.conn
                    .query(&sql("t.slug LIKE ANY($1)"), &[pats])
                    .await?
            }
        };
        Ok(rows.first().map(|r| r.get(0)))
    }

    /// Catalogue `(text_uid, titre)` des codes navigables pour la jambe texte nommé
    /// de [`Self::search_articles`] : nature navigable, rôle visible, ≥ 1 article
    /// `VIGUEUR`, droit applicable en France, titre en « Code … ».
    pub async fn named_code_catalog(&self) -> Result<Vec<(String, String)>> {
        let (valid_a, norm_a) = (article_valid_now("a"), article_is_norm("a"));
        // The title match takes the rows from the index: the ILIKE alone read all of
        // legal_text, 2.4 s warm and 17 s on a cold cache.
        let rows = self
            .conn
            .query(
                &format!(
                    "
                    SELECT t.text_uid, t.title
                    FROM legal_text t
                    WHERE t.id @@@ paradedb.match('title', 'code')
                      AND t.title ILIKE 'code %'
                      AND t.jurisdiction IN ('FR', 'EU', 'INTL')
                      AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                      AND {NAVIGABLE_TEXT_NATURES_SQL} AND {TEXT_ROLE_VISIBLE_SQL}
                      AND EXISTS (SELECT 1 FROM legal_article a
                                  WHERE a.text_uid = t.text_uid
                                    AND {valid_a} AND {norm_a})
                    "
                ),
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// The two lookups behind a [`NamedText`], run for this query alone. The
    /// served path holds them in a cache instead: both scan `legal_text` whole.
    pub async fn resolve_query_named_text(&self, query: &str) -> Result<Option<NamedText>> {
        let catalog = match lj_norm::aliases::query_names_code(query) {
            true => self.named_code_catalog().await?,
            false => Vec::new(),
        };
        let Some((target, stripped)) = lj_norm::aliases::strip_query_named_text(query, &catalog)
        else {
            return Ok(None);
        };
        Ok(self
            .resolve_named_text(target)
            .await?
            .map(|text_uid| NamedText { text_uid, stripped }))
    }

    /// The articles the query designates: its keys inside the text it names, or
    /// every text carrying them when it names none. `consumes` when the keys are
    /// the whole query, the name aside; an empty list under a name is a refusal.
    #[tracing::instrument(name = "db.designated_articles", skip(self, named, filters), fields(db.system = "postgresql"))]
    pub async fn designated_articles(
        &self,
        query: &str,
        named: Option<&NamedText>,
        filters: ArticleBrowseFilters<'_>,
    ) -> Result<Option<ArticleDesignation>> {
        use lj_norm::article_key::{bare_citation, designated_articles, query_article_keys};
        let Some(NamedText { text_uid, stripped }) = named else {
            let (keys, consumes) = match bare_citation(query) {
                Some(keys) => (keys, true),
                None => match query_article_keys(query) {
                    keys if keys.is_empty() => return Ok(None),
                    keys => (keys, false),
                },
            };
            let articles = self.key_carriers(query, &keys, filters).await?;
            return Ok(Some(ArticleDesignation { articles, consumes }));
        };
        let (keys, consumes) = match designated_articles(stripped) {
            Some(keys) => (keys, true),
            None => match query_article_keys(query) {
                keys if keys.is_empty() => return Ok(None),
                keys => (keys, false),
            },
        };
        let mut articles = Vec::with_capacity(keys.len());
        for key in &keys {
            articles.extend(
                self.designated_article_row(text_uid, key, filters.at)
                    .await?,
            );
        }
        Ok(Some(ArticleDesignation { articles, consumes }))
    }

    /// Every text carrying one of `keys`, one row per text and key, the version
    /// valid at the date. What the query spends beyond its keys scores the title,
    /// so « L251-1 bail à construction » leads on the Code de la construction and
    /// not on the first code in the alphabet; with nothing left to spend, the
    /// texts the fonds cites lead. That citation is a boolean: the graph is a blob
    /// per decision, so a count per article is not readable here.
    async fn key_carriers(
        &self,
        query: &str,
        keys: &[String],
        filters: ArticleBrowseFilters<'_>,
    ) -> Result<Vec<ArticleSearchRow>> {
        let scope = searched_scope(
            query,
            filters.home,
            filters.jurisdictions,
            filters.valid_from,
            filters.at,
        );
        let mut params = ArticleSearchParams::empty();
        let Some(num_query) = article_num_query_for(&mut params, keys) else {
            return Ok(Vec::new());
        };
        let rest = lj_norm::article_key::strip_query_article_keys(query).1;
        let (num_query, score) = match rest.trim() {
            "" => (num_query, "0::real".to_string()),
            rest => {
                let ph = params.push(Box::new(rest.to_string()));
                (
                    format!(
                        "paradedb.boolean(must => ARRAY[{num_query}], \
                         should => ARRAY[paradedb.match('search_title', ${ph})])"
                    ),
                    "paradedb.score(a.id)".to_string(),
                )
            }
        };
        let (art_filters, _, dropped) =
            params.push_filters(filters.text_uid, filters.source, filters.category);
        let num_query = dropped.scan(&num_query);
        let (art_scope, _) = params.push_scope(&scope);
        let art_bounds = params.push_bounds(&scope);
        let keys_ph = params.push(Box::new(keys.to_vec()));
        let date_sql = sql_date_expr(filters.at);
        let (valid_a, pick_a) = (article_valid_at("a", &date_sql), article_pick_order("a"));
        let norm_a = article_is_norm("a");
        let rows = self
            .conn
            .query(
                &format!(
                    "
                    SELECT d.*
                    FROM (
                      SELECT DISTINCT ON (a.text_uid, a.num_key)
                             a.text_uid, a.slug, a.code_title, a.num, a.num_key,
                             a.title_path, coalesce(a.status, '') AS status, a.source,
                             a.date_debut, a.date_fin, a.text_date, a.texte,
                             a.jurisdiction, {score} AS score,
                             EXISTS (SELECT 1 FROM legal_article_usage u
                                     WHERE u.text_uid = a.text_uid
                                       AND u.num_key = a.num_key) AS cited
                      FROM legal_article a
                      WHERE a.id @@@ {num_query}
                        AND {valid_a}
                        AND {norm_a}
                        AND a.searchable{art_filters}{art_scope}{art_bounds}
                      ORDER BY a.text_uid, a.num_key, {pick_a}
                    ) d
                    WHERE d.num_key = ANY(${keys_ph})
                    ORDER BY d.score DESC, d.cited DESC, d.code_title, d.num_key
                    LIMIT {REFERENCE_CARRIERS_LIMIT}
                    "
                ),
                &params.refs(),
            )
            .await?;
        Ok(rows.iter().map(article_search_row).collect())
    }

    /// The articles a query cites, read off the formed title alone: one
    /// conjunctive BM25 pass, no name resolution and no catalogue of names. The
    /// catalogue scans `legal_text` whole for want of an index its predicate can
    /// use.
    ///
    /// Conjunctive, so the match itself proves every token of the query landed in
    /// the title; [`lj_norm::aliases::title_names_text`] then says whether they
    /// landed on the name of the text or on its leaf division, which names
    /// nothing. Several rows when several texts answer to the name, one when an
    /// alias names it: « constitution article 34 » is not the 1789 law's. Empty is
    /// the refusal an exact resolution would also have served.
    #[tracing::instrument(name = "db.cited_articles", skip(self), fields(db.system = "postgresql"))]
    pub async fn cited_articles(
        &self,
        query: &str,
        home: &[&str],
        at: Option<NaiveDate>,
    ) -> Result<Vec<ArticleSearchRow>> {
        use lj_norm::aliases::NamedTextTarget as T;
        let scope = lj_norm::jurisdictions::query_scope(query, home);
        let query = scope.stripped.as_deref().unwrap_or(query);
        let Some((key, rest)) = lj_norm::article_key::cited_reference(query) else {
            return Ok(Vec::new());
        };
        let act = lj_norm::text_identity::act_number(&rest);
        let alias = lj_norm::aliases::cited_alias(query);
        let Some(conj) = lj_norm::aliases::conj_title_query(query) else {
            return Ok(Vec::new());
        };
        let mut params = ArticleSearchParams::empty();
        let conj_ph = params.push(Box::new(conj));
        let (art_scope, _) = params.push_scope(&Searched::corpora(scope.clone()));
        let date_sql = sql_date_expr(at);
        let (valid_a, pick_a) = (article_valid_at("a", &date_sql), article_pick_order("a"));
        let norm_a = article_is_norm("a");
        let rows = self
            .conn
            .query(
                &format!(
                    "
                    SELECT DISTINCT ON (a.text_uid, a.num_key)
                           a.text_uid, a.slug, a.code_title, a.num, a.num_key,
                           a.title_path, coalesce(a.status, '') AS status, a.source,
                           a.date_debut, a.date_fin, a.text_date, a.texte,
                           a.jurisdiction, j.score
                    FROM (
                      SELECT a.id, paradedb.score(a.id) AS score
                      FROM legal_article a
                      WHERE a.id @@@ paradedb.match('search_title', ${conj_ph}, \
                                                    conjunction_mode => true)
                        AND {valid_a}
                        AND {norm_a}
                        AND a.searchable{art_scope}
                      ORDER BY paradedb.score(a.id) DESC
                      LIMIT {CITED_ARTICLE_CANDIDATES}
                    ) j
                    JOIN legal_article a ON a.id = j.id
                    ORDER BY a.text_uid, a.num_key, j.score DESC, {pick_a}
                    "
                ),
                &params.refs(),
            )
            .await?;
        let mut candidates: Vec<(f32, ArticleSearchRow)> = rows
            .iter()
            .map(|r| (r.get::<_, f32>("score"), article_search_row(r)))
            .collect();
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
        Ok(candidates
            .into_iter()
            .map(|(_, row)| row)
            .filter(|row| {
                row.num_key == key
                    && lj_norm::aliases::title_names_text(&rest, &row.code_title)
                    && act.as_ref().is_none_or(|act| {
                        lj_norm::text_identity::head_act_number(&row.code_title)
                            .is_some_and(|own| own.number == act.number)
                    })
                    && alias.as_ref().is_none_or(|alias| match alias {
                        T::Uid(uid) => row.text_uid == *uid,
                        T::Slug(slug) => row.slug.as_ref() == Some(slug),
                        T::SlugPatterns(_) => unreachable!("an alias anchors on a slug or a uid"),
                    })
            })
            .collect())
    }

    /// The texts `identity` designates, whole: each with something to read, the
    /// consolidated text before its publication, three at most. A usual name is
    /// read in `jurisdictions`, where 27 codes share the slug `code-civil`.
    #[tracing::instrument(name = "db.texts_by_identity", skip(self), fields(db.system = "postgresql"))]
    pub async fn texts_by_identity(
        &self,
        identity: &lj_norm::text_identity::TextIdentity,
        jurisdictions: &[String],
    ) -> Result<Vec<ArticleSearchRow>> {
        use lj_norm::text_identity::{head_act_number, TextIdentity as I};
        let sql = |cond: &str| {
            format!(
                "
                SELECT t.text_uid, t.slug, t.title AS code_title, '' AS num,
                       '' AS num_key, NULL::text AS title_path,
                       coalesce(t.status, '') AS status, '' AS source,
                       NULL::date AS date_debut, NULL::date AS date_fin, t.text_date,
                       NULL::text AS texte, t.jurisdiction
                FROM legal_text t
                WHERE {cond}
                  AND t.slug IS NOT NULL
                  AND {TEXT_READABLE_SQL}
                ORDER BY t.text_uid LIKE 'LEGITEXT%' DESC, t.text_uid
                LIMIT {ACT_NUMBER_TITLES}
                "
            )
        };
        let rows = match identity {
            I::Uid(uid) => self.conn.query(&sql("t.text_uid = $1"), &[uid]).await?,
            I::Slug(slug) => {
                self.conn
                    .query(
                        &sql("t.slug = $1 AND t.jurisdiction = ANY($2)"),
                        &[slug, &jurisdictions],
                    )
                    .await?
            }
            I::Nor(nor) => self.conn.query(&sql("t.nor = $1"), &[nor]).await?,
            I::Eli(eli) => self.conn.query(&sql("t.eli = $1"), &[eli]).await?,
            I::Act(act) => {
                self.conn
                    .query(&sql("t.id @@@ paradedb.term('title', $1)"), &[&act.number])
                    .await?
            }
        };
        let mut texts: Vec<ArticleSearchRow> = rows.iter().map(article_search_row).collect();
        if let I::Act(act) = identity {
            texts.retain(|t| head_act_number(&t.code_title).as_ref() == Some(act));
        }
        texts.truncate(IDENTIFIED_TEXTS);
        Ok(texts)
    }

    /// Same version choice as [`Self::law_article_at_date`], in the search row shape.
    async fn designated_article_row(
        &self,
        text_uid: &str,
        num_key: &str,
        at: Option<NaiveDate>,
    ) -> Result<Option<ArticleSearchRow>> {
        const COLS: &str = "text_uid, slug, code_title, num, num_key, title_path, \
                            coalesce(status, '') AS status, source, date_debut, date_fin, \
                            text_date, texte, jurisdiction";
        let row = match at {
            Some(d) => {
                self.conn
                    .query_opt(
                        &format!(
                            "SELECT {COLS} FROM legal_article
                             WHERE text_uid = $1 AND num_key = $2
                               AND date_debut <= $3
                               AND (date_fin IS NULL OR date_fin >= $3)
                             ORDER BY date_debut DESC, (source = 'jorf') ASC
                             LIMIT 1"
                        ),
                        &[&text_uid, &num_key, &d],
                    )
                    .await?
            }
            None => {
                let (valid_a, norm_a) = (article_valid_now("a"), article_is_norm("a"));
                self.conn
                    .query_opt(
                        &format!(
                            "SELECT {COLS} FROM legal_article a
                             WHERE text_uid = $1 AND num_key = $2
                             ORDER BY ({valid_a}) DESC, ({norm_a}) DESC,
                                      (source = 'jorf') ASC, date_debut DESC NULLS LAST
                             LIMIT 1"
                        ),
                        &[&text_uid, &num_key],
                    )
                    .await?
            }
        };
        Ok(row.as_ref().map(article_search_row))
    }

    /// Recherche plein-texte d'articles (`/recherche-textes`), titre-primaire. Par
    /// jambe, le prédicat combine titre conjonctif normalisé, filet titre OR et
    /// corps ([`article_search_predicates`]) ; la fusion entre les jambes se fait
    /// par rang (RRF, `1/(k + rang)`, jambes bornées à [`ARTICLE_LEG_LIMIT`]).
    ///
    /// Three legs: articles, body texts, and containers (`num = ''` towards
    /// `/norm/{juris}/{slug}`, ahead at equal rank). Plus, when the query names a code
    /// of the catalogue, its own legs. Every one of them is bound to the
    /// corpora [`lj_norm::jurisdictions::query_scope`] resolves.
    ///
    /// Articles valides à `at` (`None` = aujourd'hui), une ligne par article. La
    /// recherche à date déplace la fenêtre, pas le corpus : on sert la version
    /// couvrant `at` des articles cherchables d'aujourd'hui.
    #[tracing::instrument(name = "db.search_articles", skip(self, expansions), fields(db.system = "postgresql", limit, offset))]
    #[allow(clippy::too_many_arguments)]
    pub async fn search_articles(
        &self,
        query: &str,
        expansions: &[String],
        home: &[&str],
        places: &PlaceFilter,
        text_uid: Option<&str>,
        jurisdictions: &[&str],
        source: Option<&str>,
        category: Option<(&str, bool)>,
        named: Option<&NamedText>,
        at: Option<NaiveDate>,
        valid_from: Option<NaiveDate>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<ArticleSearchRow>> {
        // `$1` titre OR (requête enrichie), `$2` corps (requête brute ; la clause
        // usage_terms est ajoutée boostée dans [`article_search_predicates`]),
        // alternatives conjonctives en `$3..`, puis filtres optionnels et pagination.
        let scope = searched_scope(query, home, jurisdictions, valid_from, at);
        let mut params = ArticleSearchParams::new(article_title_query(query, expansions), query);
        let art_query = article_search_predicate(&mut params, query, expansions);
        let (mut art_filters, txt_filters, dropped) =
            params.push_filters(text_uid, source, category);
        let art_pred = format!("a.id @@@ {}", dropped.scan(&art_query));
        let (art_scope, txt_scope) = params.push_scope(&scope);
        let art_bounds = params.push_bounds(&scope);
        art_filters.push_str(&art_scope);
        art_filters.push_str(&art_bounds);
        let art_read = format!("{art_filters}{}", dropped.row("a.jurisdiction", "a.nature"));
        // The texts a place rules out are dropped past each leg's top-K: inside it,
        // a large set, the national tax law read from Polynesia, slows every leg.
        let by_articles = params.push_places(&places.articles);
        let (art_place, cont_place) = (by_articles.row("a"), by_articles.row("t"));
        let date_sql = sql_date_expr(at);
        let (valid_a, pick_a) = (article_valid_at("a", &date_sql), article_pick_order("a"));
        let (valid_a2, pick_a2) = (article_valid_at("a2", &date_sql), article_pick_order("a2"));
        let (norm_a, norm_a2) = (article_is_norm("a"), article_is_norm("a2"));
        // Jambes « texte nommé » : requête débarrassée du nom du texte, bornée au
        // texte visé (articles + sacs d'usage). Votes sommés avec les jambes globales.
        let named_code = named.map(|n| (n.text_uid.clone(), n.stripped.clone()));
        let (nmd_cte, nmd_union) = match &named_code {
            Some((uid, stripped)) => {
                let uid_ph = params.push(Box::new(uid.clone()));
                let conj_clause = match lj_norm::aliases::conj_title_query(stripped) {
                    Some(c) => {
                        let cph = params.push(Box::new(c));
                        format!(
                            "paradedb.boost({ARTICLE_TITLE_CONJ_BOOST}, \
                             paradedb.match('search_title', ${cph}, \
                             conjunction_mode => true)), "
                        )
                    }
                    None => String::new(),
                };
                let s_ph = params.push(Box::new(stripped.clone()));
                let nmd_query = dropped.scan(&format!(
                    "paradedb.boolean(should => ARRAY[{conj_clause}\
                     paradedb.boost({ARTICLE_TITLE_OR_BOOST}, paradedb.match('search_title', ${s_ph})), \
                     paradedb.match('texte', ${s_ph})])"
                ));
                let mut cte = format!(
                    "
                    nmd AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT DISTINCT ON (a.text_uid, a.num_key)
                               a.text_uid, a.slug, a.code_title AS title, a.num,
                               a.num_key, a.title_path, a.status, a.source,
                               a.date_debut, a.date_fin, a.text_date, a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) AS score
                          FROM legal_article a
                          WHERE a.id @@@ {nmd_query}
                            AND {valid_a}
                            AND {norm_a}
                            AND a.searchable
                            AND a.text_uid = ${uid_ph}{art_filters}
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT {ARTICLE_LEG_LIMIT}
                        ) j
                        JOIN legal_article a ON a.id = j.id{art_place}
                        ORDER BY a.text_uid, a.num_key, j.score DESC, {pick_a}
                      ) x
                    ),"
                );
                let mut union = format!(
                    "
                      UNION ALL
                      SELECT text_uid, slug, title, num, num_key, title_path,
                             status, source, date_debut, date_fin, text_date, texte,
                             1.0 / ({ARTICLE_RRF_K} + rk) AS rrf, -3 AS leg
                      FROM nmd"
                );
                if !lj_textsearch::usage::usage_reference_or_nav_query(stripped) {
                    let g_ph = params.push(Box::new(lj_textsearch::usage::usage_grams(stripped)));
                    cte.push_str(&format!(
                        "
                    nmd_us AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT a.text_uid, a.slug, a.code_title AS title, a.num,
                               a.num_key, a.title_path, a.status, a.source,
                               a.date_debut, a.date_fin, a.text_date, a.texte,
                               u.score
                        FROM (
                          SELECT u.text_uid, u.num_key, paradedb.score(u.id) AS score
                          FROM legal_article_usage u
                          WHERE u.id @@@ paradedb.match('terms', ${g_ph})
                            AND u.text_uid = ${uid_ph}
                          ORDER BY paradedb.score(u.id) DESC
                        ) u
                        JOIN LATERAL (
                          SELECT a2.text_uid, a2.slug, a2.code_title, a2.num,
                                 a2.num_key, a2.title_path, a2.status,
                                 a2.source, a2.date_debut, a2.date_fin, a2.texte, a2.searchable,
                                 a2.jurisdiction, a2.nature, a2.jurisdiction_paths,
                                 a2.text_date
                          FROM legal_article a2
                          WHERE a2.text_uid = u.text_uid AND a2.num_key = u.num_key
                            AND {valid_a2}
                            AND {norm_a2}
                          ORDER BY {pick_a2} LIMIT 1
                        ) a ON true
                        WHERE a.searchable{art_read}{art_place}
                        ORDER BY u.score DESC
                        LIMIT {ARTICLE_LEG_LIMIT}
                      ) x
                    ),"
                    ));
                    union.push_str(&format!(
                        "
                      UNION ALL
                      SELECT text_uid, slug, title, num, num_key, title_path,
                             status, source, date_debut, date_fin, text_date, texte,
                             {ARTICLE_NAMED_USAGE_WEIGHT} / ({ARTICLE_RRF_K} + rk) AS rrf, 4 AS leg
                      FROM nmd_us"
                    ));
                }
                (cte, union)
            }
            None => (String::new(), String::new()),
        };
        // Jambe conteneurs : forme conjonctive de la requête + expansions d'alias
        // pleine requête.
        let mut cont_alts: Vec<String> = Vec::new();
        cont_alts.extend(lj_norm::aliases::conj_title_query(query));
        cont_alts.extend(
            lj_norm::aliases::whole_query_expansions(query)
                .iter()
                .filter_map(|e| lj_norm::aliases::conj_title_query(e)),
        );
        cont_alts.dedup();
        let (cont_cte, cont_union) = match cont_alts.as_slice() {
            [_, ..] => {
                let clauses: Vec<String> = cont_alts
                    .iter()
                    .map(|alt| {
                        let ph = params.push(Box::new(alt.clone()));
                        format!("paradedb.match('title', ${ph}, conjunction_mode => true)")
                    })
                    .collect();
                let cont_match = match clauses.as_slice() {
                    [single] => single.clone(),
                    many => format!("paradedb.boolean(should => ARRAY[{}])", many.join(", ")),
                };
                (
                    // Fence `OFFSET 0` : seuls le match titre et la juridiction descendent dans le
                    // scan ParadeDB, les filtres non indexables restent au-dessus.
                    format!(
                        "
                    cont AS (
                      SELECT z.*, row_number() OVER (ORDER BY z.score DESC, z.slug) AS rk
                      FROM (
                        SELECT t.text_uid, t.slug, t.title,
                               coalesce(t.status, 'VIGUEUR') AS status,
                               lower(t.nature) AS source,
                               t.score
                        FROM (
                          SELECT t.text_uid, t.slug, t.title, t.status, t.nature,
                                 t.role, t.jurisdiction, paradedb.score(t.id) AS score
                          FROM legal_text t
                          WHERE t.id @@@ {cont_match}{txt_scope}
                          OFFSET 0
                        ) t
                        WHERE coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'{cont_place}
                          AND t.slug IS NOT NULL
                          AND {NAVIGABLE_TEXT_NATURES_SQL} AND {TEXT_ROLE_VISIBLE_SQL}
                          AND EXISTS (SELECT 1 FROM legal_article a
                                      WHERE a.text_uid = t.text_uid AND a.num <> ''
                                        AND {valid_a} AND {norm_a}{art_bounds}){txt_filters}
                        ORDER BY t.score DESC
                        LIMIT {ARTICLE_LEG_LIMIT}
                      ) z
                    ),"
                    ),
                    format!(
                        "
                      UNION ALL
                      SELECT text_uid, slug, title, '' AS num, '' AS num_key,
                             NULL AS title_path, status, source,
                             NULL::date AS date_debut, NULL::date AS date_fin,
                             NULL::date AS text_date, NULL::text AS texte,
                             1.0 / ({ARTICLE_RRF_K} + rk) AS rrf, -2 AS leg
                      FROM cont"
                    ),
                )
            }
            [] => (String::new(), String::new()),
        };
        // Jambe « usage » : grammes de la requête contre les sacs de contextes de
        // citation (`legal_article.usage_terms`). Elle recouvre la jambe articles, ses
        // votes se somment. Coupée sur les requêtes-référence et navigationnelles.
        // The bags are sorted before the join, so the LIMIT stops the article lookups
        // once enough bags pass: joined first, a query holding « code » looked up
        // 53,615 of the 53,787 bags, 2 s.
        let (us_cte, us_union) = if lj_textsearch::usage::usage_reference_or_nav_query(query) {
            (String::new(), String::new())
        } else {
            let g_ph = params.push(Box::new(lj_textsearch::usage::usage_grams(query)));
            (
                format!(
                    "
                    us AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT a.text_uid, a.slug, a.code_title AS title, a.num,
                               a.num_key, a.title_path, a.status, a.source,
                               a.date_debut, a.date_fin, a.text_date, a.texte,
                               u.score
                        FROM (
                          SELECT u.text_uid, u.num_key, paradedb.score(u.id) AS score
                          FROM legal_article_usage u
                          WHERE u.id @@@ paradedb.match('terms', ${g_ph})
                          ORDER BY paradedb.score(u.id) DESC
                        ) u
                        JOIN LATERAL (
                          SELECT a2.text_uid, a2.slug, a2.code_title, a2.num,
                                 a2.num_key, a2.title_path, a2.status,
                                 a2.source, a2.date_debut, a2.date_fin, a2.texte, a2.searchable,
                                 a2.jurisdiction, a2.nature, a2.jurisdiction_paths,
                                 a2.text_date
                          FROM legal_article a2
                          WHERE a2.text_uid = u.text_uid AND a2.num_key = u.num_key
                            AND {valid_a2}
                            AND {norm_a2}
                          ORDER BY {pick_a2} LIMIT 1
                        ) a ON true
                        WHERE a.searchable{art_read}{art_place}
                        ORDER BY u.score DESC
                        LIMIT {ARTICLE_LEG_LIMIT}
                      ) x
                    ),"
                ),
                format!(
                    "
                      UNION ALL
                      SELECT text_uid, slug, title, num, num_key, title_path,
                             status, source, date_debut, date_fin, text_date, texte,
                             {ARTICLE_USAGE_WEIGHT} / ({ARTICLE_RRF_K} + rk) AS rrf, 3 AS leg
                      FROM us"
                ),
            )
        };
        // Jambe usage scopée au numéro cité : les sacs de contextes de citation des
        // seuls articles qui portent le numéro. Le scope passe par le littéral
        // `num_key` de l'index, sous `const_score(0, …)` : clause requise, poussée dans
        // le scan Tantivy, sans peser sur le score.
        let (nus_cte, nus_union) = match lj_norm::article_key::query_article_keys(query).as_slice()
        {
            [] => (String::new(), String::new()),
            keys => {
                let k_ph = params.push(Box::new(keys.to_vec()));
                let g_ph = params.push(Box::new(lj_textsearch::usage::usage_grams(query)));
                (
                    format!(
                        "
                    nus AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT a.text_uid, a.slug, a.code_title AS title, a.num,
                               a.num_key, a.title_path, a.status, a.source,
                               a.date_debut, a.date_fin, a.text_date, a.texte,
                               u.score
                        FROM (
                          SELECT u.text_uid, u.num_key, paradedb.score(u.id) AS score
                          FROM legal_article_usage u
                          WHERE u.id @@@ paradedb.boolean(must => ARRAY[
                                  paradedb.match('terms', ${g_ph}),
                                  paradedb.const_score(0, paradedb.term_set('num_key', ${k_ph}::text[]))])
                          ORDER BY paradedb.score(u.id) DESC
                        ) u
                        JOIN LATERAL (
                          SELECT a2.text_uid, a2.slug, a2.code_title, a2.num,
                                 a2.num_key, a2.title_path, a2.status,
                                 a2.source, a2.date_debut, a2.date_fin, a2.texte, a2.searchable,
                                 a2.jurisdiction, a2.nature, a2.jurisdiction_paths,
                                 a2.text_date
                          FROM legal_article a2
                          WHERE a2.text_uid = u.text_uid AND a2.num_key = u.num_key
                            AND {valid_a2}
                            AND {norm_a2}
                          ORDER BY {pick_a2} LIMIT 1
                        ) a ON true
                        WHERE a.searchable{art_read}{art_place}
                        ORDER BY u.score DESC
                        LIMIT {ARTICLE_LEG_LIMIT}
                      ) x
                    ),"
                    ),
                    format!(
                        "
                      UNION ALL
                      SELECT text_uid, slug, title, num, num_key, title_path,
                             status, source, date_debut, date_fin, text_date, texte,
                             {ARTICLE_USAGE_WEIGHT} / ({ARTICLE_RRF_K} + rk) AS rrf, 5 AS leg
                      FROM nus"
                    ),
                )
            }
        };
        // Leg « number ∧ title »: among the articles carrying the cited number,
        // the one whose TITLE best answers the rest of the query. Once a number
        // is cited, the other words **name the text**: « directive 2004/38 CE
        // article 7 », « code de déontologie des avocats article 11 ». The
        // articles leg cannot use them: its OR title net weighs too little to
        // tell apart the many texts with an article of that number, and its
        // conjunctive clause falls on the first word outside the title. Here the
        // title is a `must` beside the number, so it orders alone.
        let (numt_cte, numt_union) = match article_num_query(&mut params, query) {
            None => (String::new(), String::new()),
            Some(num_q) => {
                let numt_query = dropped.scan(&format!(
                    "paradedb.boolean(must => ARRAY[{num_q}, paradedb.match('search_title', $1)])"
                ));
                (
                    format!(
                        "
                    numt AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT DISTINCT ON (a.text_uid, a.num_key)
                               a.text_uid, a.slug, a.code_title AS title, a.num,
                               a.num_key, a.title_path, a.status, a.source,
                               a.date_debut, a.date_fin, a.text_date, a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) AS score
                          FROM legal_article a
                          WHERE a.id @@@ {numt_query}
                            AND {valid_a}
                            AND {norm_a}
                            AND a.searchable{art_filters}
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT {ARTICLE_LEG_LIMIT}
                        ) j
                        JOIN legal_article a ON a.id = j.id{art_place}
                        ORDER BY a.text_uid, a.num_key, j.score DESC, {pick_a}
                      ) x
                    ),"
                    ),
                    format!(
                        "
                      UNION ALL
                      SELECT text_uid, slug, title, num, num_key, title_path,
                             status, source, date_debut, date_fin, text_date, texte,
                             {ARTICLE_NUM_TITLE_WEIGHT} / ({ARTICLE_RRF_K} + rk) AS rrf, 6 AS leg
                      FROM numt"
                    ),
                )
            }
        };
        let limit_ph = params.push(Box::new(limit));
        let offset_ph = params.push(Box::new(offset));
        // Jambes articles domestique et étrangère : deux scans ParadeDB single-table
        // TopK — prédicat et filtres indexés + `ORDER BY score LIMIT` dans le scan.
        // Les colonnes d'affichage ne sont jointes que pour les ≤ leg_limit lignes
        // retenues. Coupe du top-K par score seul, puis fusion par rang. Tiebreak
        // final par `leg` : conteneur, pays nommé, articles domestiques, textes à
        // corps, articles étrangers.
        let art_key = "CASE WHEN a.num = '' THEN '' ELSE a.num_key END";
        let art_scan = article_leg_scan(
            &art_pred,
            &valid_a,
            &norm_a,
            &art_filters,
            ARTICLE_LEG_LIMIT,
        );
        let sql = format!(
            "
                    WITH{cont_cte}{nmd_cte}{us_cte}{nus_cte}{numt_cte}
                    art AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        -- An unnumbered piece is a hit on its text, its best piece
                        -- the extract.
                        SELECT DISTINCT ON (a.text_uid, {art_key})
                               a.text_uid, a.slug, a.code_title AS title, a.num,
                               {art_key} AS num_key,
                               CASE WHEN a.num = '' THEN NULL ELSE a.title_path END AS title_path,
                               a.status, a.source,
                               a.date_debut, a.date_fin, a.text_date, a.texte, j.score
                        FROM ({art_scan}) j
                        JOIN legal_article a ON a.id = j.id{art_place}
                        ORDER BY a.text_uid, {art_key}, j.score DESC, {pick_a}
                      ) x
                    )
                    SELECT u.text_uid, u.slug, u.title, u.num, u.num_key,
                           u.title_path, u.status, u.source, u.date_debut, u.date_fin,
                           u.text_date, u.texte, sum(u.rrf)::float4 AS score, min(j.jurisdiction) AS jurisdiction
                    FROM (
                      SELECT text_uid, slug, title, num, num_key, title_path,
                             status, source, date_debut, date_fin, text_date, texte,
                             1.0 / ({ARTICLE_RRF_K} + rk) AS rrf, 0 AS leg
                      FROM art{cont_union}{nmd_union}{us_union}{nus_union}{numt_union}
                    ) u
                    JOIN legal_text j ON j.text_uid = u.text_uid
                    GROUP BY u.text_uid, u.slug, u.title, u.num, u.num_key,
                             u.title_path, u.status, u.source, u.date_debut, u.date_fin,
                             u.text_date, u.texte
                    ORDER BY sum(u.rrf) DESC, min(u.leg), u.num_key
                    LIMIT ${limit_ph} OFFSET ${offset_ph}
                    "
        );
        let rows = self.conn.query(&sql, &params.refs()).await?;
        Ok(rows
            .iter()
            .map(|r| ArticleSearchRow {
                text_uid: r.get(0),
                slug: r.get(1),
                code_title: r.get(2),
                num: r.get(3),
                num_key: r.get(4),
                title_path: r.get(5),
                status: r.get(6),
                source: r.get(7),
                date_debut: r.get(8),
                date_fin: r.get(9),
                text_date: r.get(10),
                texte: r.get(11),
                score: r.get(12),
                jurisdiction: r.get(13),
            })
            .collect())
    }

    /// The articles member of the total and the facets, under the same BM25
    /// predicate and filters as [`Self::search_articles`]: one
    /// `paradedb.aggregate`, the whole predicate written as `searchqueryinput`.
    ///
    /// The members carry no order and no cap; both come from
    /// [`merge_article_facets`], which is what makes them summable. They run one
    /// per connection: chained, the three cost far more than the heaviest alone.
    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(name = "db.article_facets_articles", skip(self, expansions), fields(db.system = "postgresql"))]
    pub async fn article_facets_articles(
        &self,
        query: &str,
        expansions: &[String],
        home: &[&str],
        places: &PlaceFilter,
        text_uid: Option<&str>,
        jurisdictions: &[&str],
        source: Option<&str>,
        category: Option<(&str, bool)>,
        at: Option<NaiveDate>,
        valid_from: Option<NaiveDate>,
    ) -> Result<ArticleFacetMember> {
        // Corps = requête brute ; la clause usage_terms est ajoutée par
        // `article_search_predicates`. Les filtres SQL deviennent des clauses `must` du
        // `searchqueryinput`. Jeu de paramètres dédié : chaque placeholder poussé doit
        // être référencé par la requête qui le porte.
        let scope = searched_scope(query, home, jurisdictions, valid_from, at);
        let mut params_art =
            ArticleSearchParams::new(article_title_query(query, expansions), query);
        let art_query = article_search_predicate(&mut params_art, query, expansions);
        let mut agg_filters = agg_jurisdiction(&mut params_art, &scope);
        agg_filters.push_str(&article_bounds_agg(&mut params_art, &scope));
        let mut agg_must_not = Vec::new();
        for (field, value) in [
            ("text_uid", text_uid.map(str::to_string)),
            ("source", source.map(str::to_string)),
        ] {
            if let Some(v) = value {
                let ph = params_art.push(Box::new(v));
                agg_filters.push_str(&format!(", paradedb.term('{field}', ${ph})"));
            }
        }
        agg_must_not.extend(params_art.push_category(category).must_not("nature"));
        agg_must_not.extend(params_art.push_places(&places.articles).must_not());
        let agg_must_not = match agg_must_not.as_slice() {
            [] => String::new(),
            many => format!(", must_not => ARRAY[{}]", many.join(", ")),
        };
        let valid_agg = article_valid_agg(&sql_date_expr(at));
        let agg_row = self
            .conn
            .query_one(
                &format!(
                    "SELECT paradedb.aggregate('legal_article_bm25', \
                       paradedb.boolean(must => ARRAY[{art_query}, \
                         {valid_agg}, \
                         paradedb.term('kind', 'article'), \
                         paradedb.term('searchable', true){agg_filters}]{agg_must_not}), \
                       '{ARTICLE_FACETS_AGG}'::json, \
                       bucket_limit => {ARTICLE_FACETS_BUCKET_LIMIT})::text"
                ),
                &params_art.refs(),
            )
            .await?;
        let agg_art: serde_json::Value = serde_json::from_str(agg_row.get(0))
            .map_err(|e| StoreError::Invalid(format!("agrégat facettes articles: {e}")))?;
        facet_member(&agg_art, "articles")
    }

    /// The containers member: a SQL `GROUPING SETS` over `legal_text`, whose
    /// predicate the index does not carry.
    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(name = "db.article_facets_containers", skip(self), fields(db.system = "postgresql"))]
    pub async fn article_facets_containers(
        &self,
        query: &str,
        home: &[&str],
        places: &PlaceFilter,
        text_uid: Option<&str>,
        jurisdictions: &[&str],
        source: Option<&str>,
        category: Option<(&str, bool)>,
        at: Option<NaiveDate>,
        valid_from: Option<NaiveDate>,
    ) -> Result<ArticleFacetMember> {
        let scope = searched_scope(query, home, jurisdictions, valid_from, at);
        let mut member = ArticleFacetMember {
            total: 0,
            axes: Default::default(),
        };

        let mut cont_alts: Vec<String> = Vec::new();
        cont_alts.extend(lj_norm::aliases::conj_title_query(query));
        cont_alts.extend(
            lj_norm::aliases::whole_query_expansions(query)
                .iter()
                .filter_map(|e| lj_norm::aliases::conj_title_query(e)),
        );
        cont_alts.dedup();
        // `GROUPING(...)` encodes the active set as a bitmask, a set bit for a
        // column NOT grouped: code 0b01111 to paths 0b11110, total 0b11111.
        let rows = match cont_alts.as_slice() {
            [] => Vec::new(),
            [_, ..] => {
                let mut params = ArticleSearchParams::empty();
                let clauses: Vec<String> = cont_alts
                    .iter()
                    .map(|alt| {
                        let ph = params.push(Box::new(alt.clone()));
                        format!("paradedb.match('title', ${ph}, conjunction_mode => true)")
                    })
                    .collect();
                let cont_match = match clauses.as_slice() {
                    [single] => single.clone(),
                    many => format!("paradedb.boolean(should => ARRAY[{}])", many.join(", ")),
                };
                let (_, txt_filters, _) = params.push_filters(text_uid, source, category);
                let (_, txt_scope) = params.push_scope(&scope);
                let art_bounds = params.push_bounds(&scope);
                let cont_place = params.push_places(&places.articles).row("t");
                let valid_a = article_valid_at("a", &sql_date_expr(at));
                let norm_a = article_is_norm("a");
                self.conn
                    .query(
                        &format!(
                            "
                    SELECT GROUPING(u.slug, u.jurisdiction, u.nature, u.source, u.paths) AS gset,
                           u.slug, u.jurisdiction, u.nature, u.source, count(*) AS n, u.paths
                    FROM (
                      SELECT t.slug, t.jurisdiction, upper(t.nature) AS nature,
                             lower(t.nature) AS source, t.jurisdiction_paths AS paths
                      FROM (
                        SELECT t.text_uid, t.slug, t.jurisdiction, t.nature,
                               t.status, t.role, t.jurisdiction_paths
                        FROM legal_text t
                        WHERE t.id @@@ {cont_match}{txt_scope}
                        OFFSET 0
                      ) t
                      WHERE coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'{cont_place}
                        AND t.slug IS NOT NULL
                        AND {NAVIGABLE_TEXT_NATURES_SQL} AND {TEXT_ROLE_VISIBLE_SQL}
                        AND EXISTS (SELECT 1 FROM legal_article a
                                    WHERE a.text_uid = t.text_uid
                                      AND {valid_a} AND {norm_a}{art_bounds}){txt_filters}
                    ) u
                    GROUP BY GROUPING SETS ((u.slug), (u.jurisdiction), (u.jurisdiction, u.nature),
                                            (u.source), (u.paths), ())
                    "
                        ),
                        &params.refs(),
                    )
                    .await?
            }
        };
        for r in &rows {
            let gset: i32 = r.get(0);
            let count: i64 = r.get(5);
            let (idx, value): (usize, Option<String>) = match gset {
                0b01111 => (0, r.get(1)),
                0b10111 => (1, r.get(2)),
                0b10011 => {
                    let (jurisdiction, nature): (Option<String>, Option<String>) =
                        (r.get(2), r.get(3));
                    let (jurisdiction, nature) =
                        (jurisdiction.unwrap_or_default(), nature.unwrap_or_default());
                    let category = lj_norm::category::text_category(&jurisdiction, &nature);
                    (2, Some(category.to_string()))
                }
                0b11101 => (3, r.get(4)),
                0b11110 => {
                    let paths: Option<Vec<String>> = r.get(6);
                    for path in paths.into_iter().flatten() {
                        *member.axes[4].entry(path).or_default() += count;
                    }
                    continue;
                }
                _ => {
                    member.total += count;
                    continue;
                }
            };
            if let Some(value) = value.filter(|v| !v.is_empty()) {
                *member.axes[idx].entry(value).or_default() += count;
            }
        }
        Ok(member)
    }

    /// The texts of the searched corpora, the most recently changed first: what
    /// the norm search lists with no query, one row per text. A text changes when
    /// one of its article versions enters into force, and on its own date;
    /// `valid_from` and `at` bound both, as they bound the search.
    #[tracing::instrument(name = "db.list_texts", skip(self), fields(db.system = "postgresql", limit, offset))]
    #[allow(clippy::too_many_arguments)]
    pub async fn list_texts(
        &self,
        filters: ArticleBrowseFilters<'_>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<TextListRow>> {
        let wanted = (offset + limit) as usize;
        let mut changes = Vec::new();
        for from in list_lookbacks(filters.valid_from, filters.at) {
            let bounded = ArticleBrowseFilters {
                valid_from: from,
                ..filters
            };
            changes = self.text_changes(bounded, wanted).await?;
            if changes.len() >= wanted {
                break;
            }
        }
        let page: Vec<(String, NaiveDate)> = changes
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect();
        let uids: Vec<&str> = page.iter().map(|(uid, _)| uid.as_str()).collect();
        let rows = self
            .conn
            .query(
                "SELECT text_uid, slug, title, jurisdiction, lower(nature) \
                 FROM legal_text WHERE text_uid = ANY($1)",
                &[&uids],
            )
            .await?;
        let mut by_uid: HashMap<String, tokio_postgres::Row> = rows
            .into_iter()
            .map(|r| (r.get::<_, String>(0), r))
            .collect();
        Ok(page
            .into_iter()
            .filter_map(|(uid, changed)| {
                let r = by_uid.remove(&uid)?;
                Some(TextListRow {
                    text_uid: uid,
                    slug: r.get(1),
                    title: r.get(2),
                    jurisdiction: r.get(3),
                    valid_from: changed,
                    source: r.get(4),
                })
            })
            .collect())
    }

    /// The `wanted` latest changes of the listed texts at or after `from`, one per
    /// text, newest first. An article leg takes each text's latest version start
    /// out of an aggregate ordered on it, a text leg its own date out of a top-K;
    /// a text in the first `wanted` of the merge is in the first `wanted` of its
    /// leg, so both legs stop there.
    async fn text_changes(
        &self,
        filters: ArticleBrowseFilters<'_>,
        wanted: usize,
    ) -> Result<Vec<(String, NaiveDate)>> {
        let mut params = ArticleSearchParams::empty();
        let pred = listed_article_predicate(&mut params, &filters, false);
        let agg = format!(
            r#"{{"t":{{"terms":{{"field":"text_uid","size":{wanted},"segment_size":{wanted},"order":{{"last":"desc"}}}},"aggs":{{"last":{{"max":{{"field":"date_debut"}}}}}}}}}}"#
        );
        let row = self
            .conn
            .query_one(
                &format!(
                    "SELECT paradedb.aggregate('legal_article_bm25', {pred}, \
                       '{agg}'::json, bucket_limit => {ARTICLE_FACETS_BUCKET_LIMIT})::text" // sql-splice-ok: agg carries a usize alone
                ),
                &params.refs(),
            )
            .await?;
        let agg: serde_json::Value = serde_json::from_str(row.get(0))
            .map_err(|e| StoreError::Invalid(format!("text changes: {e}")))?;
        let mut latest: HashMap<String, NaiveDate> = HashMap::new();
        for bucket in agg["t"]["buckets"].as_array().into_iter().flatten() {
            let (Some(uid), Some(day)) = (
                bucket["key"].as_str(),
                bucket["last"]["key_as_string"]
                    .as_str()
                    .and_then(|d| NaiveDate::parse_from_str(d.get(..10)?, "%Y-%m-%d").ok()),
            ) else {
                return Err(StoreError::Invalid(format!(
                    "text changes: bucket {bucket}"
                )));
            };
            latest.insert(uid.to_string(), day);
        }
        let mut params = ArticleSearchParams::empty();
        let pred = listed_text_predicate(&mut params, &filters);
        let wanted_ph = params.push(Box::new(wanted as i64));
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT t.text_uid, t.text_date FROM legal_text t WHERE t.id @@@ {pred} \
                     ORDER BY t.text_date DESC, t.id LIMIT ${wanted_ph}"
                ),
                &params.refs(),
            )
            .await?;
        for r in rows {
            let day: NaiveDate = r.get(1);
            latest
                .entry(r.get(0))
                .and_modify(|d| *d = (*d).max(day))
                .or_insert(day);
        }
        let mut changes: Vec<(String, NaiveDate)> = latest.into_iter().collect();
        changes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        changes.truncate(wanted);
        Ok(changes)
    }

    /// The total and the facets of [`Self::list_texts`]. Bounded below, its two
    /// legs count apart and the texts both hold come off once: the article leg
    /// counts texts with a `cardinality`, an estimate past a few thousand. With no
    /// lower bound every text of the corpora answers, and the text index counts
    /// them alone.
    #[tracing::instrument(name = "db.text_list_stats", skip(self), fields(db.system = "postgresql"))]
    pub async fn text_list_stats(
        &self,
        filters: ArticleBrowseFilters<'_>,
    ) -> Result<ArticleSearchStats> {
        let mut params = ArticleSearchParams::empty();
        let pred = listed_text_predicate(&mut params, &filters);
        let row = self
            .conn
            .query_one(
                &format!(
                    "SELECT paradedb.aggregate('legal_text_bm25', {pred}, \
                       '{TEXT_LIST_FACETS_AGG}'::json, \
                       bucket_limit => {ARTICLE_FACETS_BUCKET_LIMIT})::text"
                ),
                &params.refs(),
            )
            .await?;
        let agg: serde_json::Value = serde_json::from_str(row.get(0))
            .map_err(|e| StoreError::Invalid(format!("text list facets: {e}")))?;
        let mut member = facet_member(&agg, "liste")?;
        if filters.valid_from.is_none() {
            return Ok(merge_article_facets([member]));
        }
        let mut params = ArticleSearchParams::empty();
        let pred = listed_article_predicate(&mut params, &filters, false);
        let row = self
            .conn
            .query_one(
                &format!(
                    "SELECT paradedb.aggregate('legal_article_bm25', {pred}, \
                       '{LISTED_TEXTS_AGG}'::json, \
                       bucket_limit => {ARTICLE_FACETS_BUCKET_LIMIT})::text"
                ),
                &params.refs(),
            )
            .await?;
        let changed = listed_texts_counts(row.get(0))?;
        let mut params = ArticleSearchParams::empty();
        let pred = listed_article_predicate(&mut params, &filters, true);
        let row = self
            .conn
            .query_one(
                &format!(
                    "SELECT paradedb.aggregate('legal_article_bm25', {pred}, \
                       '{LISTED_TEXTS_AGG}'::json, \
                       bucket_limit => {ARTICLE_FACETS_BUCKET_LIMIT})::text"
                ),
                &params.refs(),
            )
            .await?;
        let both = listed_texts_counts(row.get(0))?;
        member.total += changed.total - both.total;
        for (axis, plus, minus) in [
            (1, &changed.jurisdiction, &both.jurisdiction),
            (2, &changed.category, &both.category),
            (3, &changed.source, &both.source),
            (4, &changed.path, &both.path),
        ] {
            for (value, n) in plus {
                *member.axes[axis].entry(value.clone()).or_default() += n;
            }
            for (value, n) in minus {
                *member.axes[axis].entry(value.clone()).or_default() -= n;
            }
            member.axes[axis].retain(|_, n| *n > 0);
        }
        Ok(merge_article_facets([member]))
    }

    /// Bras du banc de ranking articles : les
    /// variantes du prédicat titre ([`ArticleTitleMode`]) et de la fusion
    /// inter-jambes (score brut vs RRF) de [`Self::search_articles`], sans filtre
    /// ni pagination.
    #[tracing::instrument(name = "db.search_articles_rank_arm", skip(self, expansions), fields(db.system = "postgresql", limit))]
    pub async fn search_articles_rank_arm(
        &self,
        query: &str,
        expansions: &[String],
        title_mode: ArticleTitleMode,
        or_boost: f64,
        rrf: Option<ArticleRrf>,
        limit: i64,
    ) -> Result<Vec<ArticleRankHit>> {
        // Jambe titre selon le mode : `Or` = requête enrichie ($1) boostée ×4 ;
        // `Conj` = un `should` d'alternatives toutes conjonctives, normalisées pour le
        // titre et devant matcher en entier ; `OrConj` = les deux clauses. Rien ne
        // subsiste après normalisation → repli Or. $1 porte la forme utile au mode
        // choisi, Postgres ne typant pas un placeholder jamais référencé.
        let art_key = "CASE WHEN a.num = '' THEN '' ELSE a.num_key END";
        let mut conj_alts: Vec<String> = std::iter::once(query.to_string())
            .chain(expansions.iter().cloned())
            .filter_map(|q| lj_norm::aliases::conj_title_query(&q))
            .collect();
        conj_alts.dedup();
        let title_mode = if conj_alts.is_empty() {
            ArticleTitleMode::Or
        } else {
            title_mode
        };
        let title_param = match title_mode {
            ArticleTitleMode::Conj => conj_alts[0].clone(),
            _ => article_title_query(query, expansions),
        };
        let mut params = ArticleSearchParams::new(title_param, query);
        let mut alt_phs: Vec<usize> = Vec::new();
        match title_mode {
            ArticleTitleMode::Or => {}
            ArticleTitleMode::Conj => {
                alt_phs.push(1);
                for alt in &conj_alts[1..] {
                    alt_phs.push(params.push(Box::new(alt.clone())));
                }
            }
            ArticleTitleMode::OrConj => {
                for alt in &conj_alts {
                    alt_phs.push(params.push(Box::new(alt.clone())));
                }
            }
        }
        let title_clause = |field: &str| {
            let or = format!("paradedb.match('{field}', $1)");
            let conj = || {
                let alts: Vec<String> = alt_phs
                    .iter()
                    .map(|ph| format!("paradedb.match('{field}', ${ph}, conjunction_mode => true)"))
                    .collect();
                match alts.as_slice() {
                    [single] => single.clone(),
                    many => format!("paradedb.boolean(should => ARRAY[{}])", many.join(", ")),
                }
            };
            match title_mode {
                ArticleTitleMode::Or => format!("paradedb.boost(4, {or})"),
                ArticleTitleMode::Conj => format!("paradedb.boost(4, {})", conj()),
                // `or_boost` module le filet OR, inliné : `paradedb.boost` n'accepte pas de
                // placeholder.
                ArticleTitleMode::OrConj => {
                    format!(
                        "paradedb.boost(4, {}), paradedb.boost({or_boost}, {or})",
                        conj()
                    )
                }
            }
        };
        let (valid_a, norm_a, pick_a) = (
            article_valid_now("a"),
            article_is_norm("a"),
            article_pick_order("a"),
        );
        let (valid_a2, norm_a2, pick_a2) = (
            article_valid_now("a2"),
            article_is_norm("a2"),
            article_pick_order("a2"),
        );
        let art_pred = format!(
            "a.id @@@ paradedb.boolean(should => ARRAY[{}, paradedb.match('texte', $2)]) \
             AND {valid_a} AND {norm_a} AND a.searchable",
            title_clause("search_title")
        );

        let sql = match rrf {
            None => {
                let limit_ph = params.push(Box::new(limit));
                format!(
                    "
                    SELECT * FROM (
                      SELECT DISTINCT ON (a.text_uid, {art_key})
                             a.slug, a.num, {art_key} AS num_key, a.code_title AS title,
                             a.title_path, a.texte,
                             paradedb.score(a.id) AS score
                      FROM legal_article a
                      WHERE {art_pred}
                      ORDER BY a.text_uid, {art_key},
                               paradedb.score(a.id) DESC, {pick_a}
                    ) u
                    ORDER BY u.score DESC, u.num_key
                    LIMIT ${limit_ph}
                    "
                )
            }
            Some(ArticleRrf {
                k,
                leg_limit,
                split_title: true,
                ..
            }) => {
                let k_ph = params.push(Box::new(k));
                let leg_ph = params.push(Box::new(leg_limit));
                let limit_ph = params.push(Box::new(limit));
                // 2 jambes mono-clause (titre et corps séparés). Scores RRF sommés par doc.
                // `$1` = requête titre enrichie (mode Or).
                format!(
                    "
                    WITH art_t AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT DISTINCT ON (a.text_uid, {art_key})
                               a.slug, a.num, {art_key} AS num_key, a.code_title AS title,
                               a.title_path, a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) AS score
                          FROM legal_article a
                          WHERE a.id @@@ paradedb.match('search_title', $1)
                            AND {valid_a} AND {norm_a} AND a.searchable
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT ${leg_ph}
                        ) j
                        JOIN legal_article a ON a.id = j.id
                        ORDER BY a.text_uid, {art_key}, j.score DESC, {pick_a}
                      ) x
                    ),
                    art_b AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT DISTINCT ON (a.text_uid, {art_key})
                               a.slug, a.num, {art_key} AS num_key, a.code_title AS title,
                               a.title_path, a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) AS score
                          FROM legal_article a
                          WHERE a.id @@@ paradedb.match('texte', $2)
                            AND {valid_a} AND {norm_a} AND a.searchable
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT ${leg_ph}
                        ) j
                        JOIN legal_article a ON a.id = j.id
                        ORDER BY a.text_uid, {art_key}, j.score DESC, {pick_a}
                      ) x
                    )
                    SELECT u.slug, u.num, u.title, u.title_path, u.texte FROM (
                      SELECT slug, num, num_key, title, title_path, texte,
                             1.0 / (${k_ph}::float8 + rk) AS rrf, 0 AS leg
                      FROM art_t
                      UNION ALL
                      SELECT slug, num, num_key, title, title_path, texte,
                             1.0 / (${k_ph}::float8 + rk) AS rrf, 1 AS leg
                      FROM art_b
                    ) u
                    GROUP BY u.slug, u.num, u.num_key, u.title, u.title_path, u.texte
                    ORDER BY sum(u.rrf) DESC, min(u.leg), u.num_key
                    LIMIT ${limit_ph}
                    "
                )
            }
            Some(ArticleRrf {
                k,
                leg_limit,
                split_title: false,
                container,
                foreign_weight,
                container_alias,
                country_leg,
                foreign_score_merge,
                usage_weight,
                usage_table,
            }) => {
                let k_ph = params.push(Box::new(k));
                let leg_ph = params.push(Box::new(leg_limit));
                let limit_ph = params.push(Box::new(limit));
                // Prior de juridiction optionnel : la jambe articles se scinde en domestique
                // et étrangère pondérée `foreign_weight`. Deux fusions : par rang (jambe RRF
                // séparée) ou par score (`foreign_score_merge`, membre UNION pondéré).
                let mut prim_ph: Option<usize> = None;
                let (art_juris, art_merge_union, artf_cte, artf_union) = match foreign_weight {
                    Some(w) if foreign_score_merge => {
                        let ph = params.push(Box::new(primary_jurisdictions(query)));
                        prim_ph = Some(ph);
                        (
                            format!("\n  AND a.jurisdiction = ANY(${ph})"),
                            format!(
                                "
                        UNION ALL
                        (SELECT DISTINCT ON (a.text_uid, {art_key})
                               a.slug, a.num, {art_key} AS num_key, a.code_title AS title,
                               a.title_path, a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) * {w}::float8 AS score
                          FROM legal_article a
                          WHERE {art_pred}
                            AND a.jurisdiction <> ALL(${ph})
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT ${leg_ph}
                        ) j
                        JOIN legal_article a ON a.id = j.id
                        ORDER BY a.text_uid, {art_key}, j.score DESC, {pick_a})"
                            ),
                            String::new(),
                            String::new(),
                        )
                    }
                    Some(w) => {
                        let ph = params.push(Box::new(primary_jurisdictions(query)));
                        prim_ph = Some(ph);
                        (
                            format!("\n  AND a.jurisdiction = ANY(${ph})"),
                            String::new(),
                            format!(
                                "
                    art_f AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT DISTINCT ON (a.text_uid, {art_key})
                               a.slug, a.num, {art_key} AS num_key, a.code_title AS title,
                               a.title_path, a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) AS score
                          FROM legal_article a
                          WHERE {art_pred}
                            AND a.jurisdiction <> ALL(${ph})
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT ${leg_ph}
                        ) j
                        JOIN legal_article a ON a.id = j.id
                        ORDER BY a.text_uid, {art_key}, j.score DESC, {pick_a}
                      ) x
                    ),"
                            ),
                            format!(
                                "
                      UNION ALL
                      SELECT slug, num, num_key, title, title_path, texte,
                             {w}::float8 / (${k_ph}::float8 + rk) AS rrf, 2 AS leg
                      FROM art_f"
                            ),
                        )
                    }
                    None => (String::new(), String::new(), String::new(), String::new()),
                };
                // Jambe « termes d'usage » optionnelle : requête en grammes contre les sacs de
                // contextes de citation, jointe à la version en vigueur.
                let usage_weight = usage_weight
                    .filter(|_| !lj_textsearch::usage::usage_reference_or_nav_query(query));
                let (usage_cte, usage_union) = match usage_weight {
                    Some(uw) => {
                        let gq = lj_textsearch::usage::usage_grams(query);
                        let g_ph = params.push(Box::new(gq));
                        let uw_ph = params.push(Box::new(uw));
                        let usage_table = usage_table.unwrap_or("legal_article_usage");
                        (
                            format!(
                                "
                    us AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT la.slug, la.num, u.num_key, la.code_title AS title,
                               la.title_path, la.texte,
                               paradedb.score(u.id) AS score
                        FROM {usage_table} u
                        JOIN LATERAL (
                          SELECT a2.slug, a2.code_title, a2.num, a2.title_path,
                                 a2.texte, a2.searchable
                          FROM legal_article a2
                          WHERE a2.text_uid = u.text_uid AND a2.num_key = u.num_key
                            AND {valid_a2} AND {norm_a2}
                          ORDER BY {pick_a2} LIMIT 1
                        ) la ON true
                        WHERE u.id @@@ paradedb.match('terms', ${g_ph})
                          AND la.searchable
                        ORDER BY paradedb.score(u.id) DESC
                        LIMIT ${leg_ph}
                      ) x
                    ),"
                            ),
                            format!(
                                "
                      UNION ALL
                      SELECT slug, num, num_key, title, title_path, texte,
                             ${uw_ph}::float8 / (${k_ph}::float8 + rk) AS rrf, 3 AS leg
                      FROM us"
                            ),
                        )
                    }
                    None => (String::new(), String::new()),
                };
                // Fusion finale : les jambes historiques sont doc-disjointes, le merge-sort
                // des votes suffit. La jambe usage recouvre la jambe articles : ses votes se
                // somment par doc.
                let final_order = if usage_weight.is_some() {
                    "GROUP BY u.slug, u.num, u.num_key, u.title, u.title_path, u.texte
                    ORDER BY sum(u.rrf) DESC, min(u.leg), u.num_key"
                } else {
                    "ORDER BY u.rrf DESC, u.leg, u.num_key"
                };
                // Jambe « pays nommé » optionnelle ; la jambe articles exclut alors ces
                // juridictions. Tiebreak -1 : sous le conteneur, au-dessus du domestique.
                let ctry = country_leg
                    .then(|| lj_norm::jurisdictions::strip_query_jurisdictions(query))
                    .flatten();
                let (art_ctry_excl, ctry_cte, ctry_union) = match ctry {
                    Some((codes, stripped)) => {
                        let codes: Vec<String> = codes.into_iter().map(String::from).collect();
                        let codes_ph = params.push(Box::new(codes));
                        let conj_clause = match lj_norm::aliases::conj_title_query(&stripped) {
                            Some(c) => {
                                let cph = params.push(Box::new(c));
                                format!(
                                    "paradedb.boost({ARTICLE_TITLE_CONJ_BOOST}, \
                                         paradedb.match('search_title', ${cph}, \
                                         conjunction_mode => true)), "
                                )
                            }
                            None => String::new(),
                        };
                        let s_ph = params.push(Box::new(stripped));
                        (
                            format!("\n  AND a.jurisdiction <> ALL(${codes_ph})"),
                            format!(
                                "
                    ctry AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        SELECT DISTINCT ON (a.text_uid, {art_key})
                               a.slug, a.num, {art_key} AS num_key, a.code_title AS title,
                               a.title_path, a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) AS score
                          FROM legal_article a
                          WHERE a.id @@@ paradedb.boolean(should => ARRAY[{conj_clause}\
                                paradedb.boost({ARTICLE_TITLE_OR_BOOST}, paradedb.match('search_title', ${s_ph})), \
                                paradedb.match('texte', ${s_ph})])
                            AND {valid_a} AND {norm_a} AND a.searchable
                            AND a.jurisdiction = ANY(${codes_ph})
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT ${leg_ph}
                        ) j
                        JOIN legal_article a ON a.id = j.id
                        ORDER BY a.text_uid, {art_key}, j.score DESC, {pick_a}
                      ) x
                    ),"
                            ),
                            format!(
                                "
                      UNION ALL
                      SELECT slug, num, num_key, title, title_path, texte,
                             1.0 / (${k_ph}::float8 + rk) AS rrf, -1 AS leg
                      FROM ctry"
                            ),
                        )
                    }
                    None => (String::new(), String::new(), String::new()),
                };
                // Jambe conteneurs optionnelle : `legal_text` sans corps mais à articles en
                // vigueur, titre en conjonctif seul et sur la seule forme conjonctive de la
                // requête, bornée aux natures navigables comme un code et au prior de
                // juridiction quand il est actif. `leg = -1` : à rang RRF égal le conteneur
                // prime.
                let mut cont_alts: Vec<String> = Vec::new();
                if container {
                    cont_alts.extend(lj_norm::aliases::conj_title_query(query));
                    if container_alias {
                        cont_alts.extend(
                            lj_norm::aliases::whole_query_expansions(query)
                                .iter()
                                .filter_map(|e| lj_norm::aliases::conj_title_query(e)),
                        );
                    }
                    cont_alts.dedup();
                }
                let (cont_cte, cont_union) = match cont_alts.as_slice() {
                    [_, ..] => {
                        let clauses: Vec<String> = cont_alts
                            .iter()
                            .map(|alt| {
                                let ph = params.push(Box::new(alt.clone()));
                                format!("paradedb.match('title', ${ph}, conjunction_mode => true)")
                            })
                            .collect();
                        let conj = match clauses.as_slice() {
                            [single] => single.clone(),
                            many => {
                                format!("paradedb.boolean(should => ARRAY[{}])", many.join(", "))
                            }
                        };
                        let cont_juris = prim_ph
                            .map(|ph| format!("\n  AND t.jurisdiction = ANY(${ph})"))
                            .unwrap_or_default();
                        (
                            format!(
                                "
                    cont AS (
                      SELECT z.*, row_number() OVER (ORDER BY z.score DESC, z.slug) AS rk
                      FROM (
                        SELECT t.slug, t.title, paradedb.score(t.id) AS score
                        FROM legal_text t
                        WHERE t.id @@@ {conj}
                          AND coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'
                          AND t.slug IS NOT NULL
                          AND {NAVIGABLE_TEXT_NATURES_SQL} AND {TEXT_ROLE_VISIBLE_SQL}
                          AND EXISTS (SELECT 1 FROM legal_article a
                                      WHERE a.text_uid = t.text_uid
                                        AND {valid_a} AND {norm_a}){cont_juris}
                        ORDER BY paradedb.score(t.id) DESC
                        LIMIT ${leg_ph}
                      ) z
                    ),"
                            ),
                            format!(
                                "
                      UNION ALL
                      SELECT slug, '' AS num, '' AS num_key, title,
                             NULL AS title_path, NULL AS texte,
                             1.0 / (${k_ph}::float8 + rk) AS rrf, -2 AS leg
                      FROM cont"
                            ),
                        )
                    }
                    [] => (String::new(), String::new()),
                };
                // Chaque jambe est classée seule (l'ORDER BY interne reste sur le seul score
                // pour garder le Top-K scan ParadeDB), puis fusion par rang. Tiebreak : jambe
                // articles d'abord.
                format!(
                    "
                    WITH{cont_cte}{ctry_cte}{artf_cte}{usage_cte}
                    art AS (
                      SELECT x.*, row_number() OVER (ORDER BY x.score DESC, x.num_key) AS rk
                      FROM (
                        (SELECT DISTINCT ON (a.text_uid, {art_key})
                               a.slug, a.num, {art_key} AS num_key, a.code_title AS title,
                               a.title_path, a.texte, j.score
                        FROM (
                          SELECT a.id, paradedb.score(a.id) AS score
                          FROM legal_article a
                          WHERE {art_pred}{art_juris}{art_ctry_excl}
                          ORDER BY paradedb.score(a.id) DESC
                          LIMIT ${leg_ph}
                        ) j
                        JOIN legal_article a ON a.id = j.id
                        ORDER BY a.text_uid, {art_key}, j.score DESC, {pick_a}){art_merge_union}
                      ) x
                    )
                    SELECT u.slug, u.num, u.title, u.title_path, u.texte FROM (
                      SELECT slug, num, num_key, title, title_path, texte,
                             1.0 / (${k_ph}::float8 + rk) AS rrf, 0 AS leg
                      FROM art{artf_union}{usage_union}{ctry_union}{cont_union}
                    ) u
                    {final_order}
                    LIMIT ${limit_ph}
                    "
                )
            }
        };

        let rows = self.conn.query(&sql, &params.refs()).await?;
        Ok(rows
            .iter()
            .map(|r| match rrf {
                None => ArticleRankHit {
                    slug: r.get(0),
                    num: r.get(1),
                    code_title: r.get(3),
                    title_path: r.get(4),
                    texte: r.get(5),
                },
                Some(_) => ArticleRankHit {
                    slug: r.get(0),
                    num: r.get(1),
                    code_title: r.get(2),
                    title_path: r.get(3),
                    texte: r.get(4),
                },
            })
            .collect())
    }

    /// The rows of the articles leg of [`Self::search_articles`] today, read
    /// `depth` deep and not grouped by article, best score first.
    pub async fn article_leg_hits(
        &self,
        query: &str,
        expansions: &[String],
        home: &[&str],
        depth: i64,
    ) -> Result<Vec<ArticleLegHit>> {
        let scope = searched_scope(query, home, &[], None, None);
        let mut params = ArticleSearchParams::new(article_title_query(query, expansions), query);
        let art_pred = format!(
            "a.id @@@ {}",
            article_search_predicate(&mut params, query, expansions)
        );
        let (mut art_filters, _, _) = params.push_filters(None, None, None);
        let (art_scope, _) = params.push_scope(&scope);
        art_filters.push_str(&art_scope);
        art_filters.push_str(&params.push_bounds(&scope));
        let valid_a = article_valid_at("a", &sql_date_expr(None));
        let scan = article_leg_scan(
            &art_pred,
            &valid_a,
            &article_is_norm("a"),
            &art_filters,
            depth,
        );
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT j.id, a.text_uid, CASE WHEN a.num = '' THEN '' ELSE a.num_key END,
                            a.num, j.score::float8, a.slug, coalesce(a.code_title, '')
                     FROM ({scan}) j
                     JOIN legal_article a ON a.id = j.id
                     ORDER BY j.score DESC, j.id"
                ),
                &params.refs(),
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| ArticleLegHit {
                id: r.get(0),
                text_uid: r.get(1),
                key: r.get(2),
                num: r.get(3),
                score: r.get(4),
                slug: r.get(5),
                title: r.get(6),
            })
            .collect())
    }

    /// `head_only` bounds the catalogue to the `codes` fond, in SQL, so that
    /// `article_count` is counted for those texts only.
    #[tracing::instrument(name = "db.list_legal_texts", skip(self), fields(db.system = "postgresql"))]
    pub async fn list_legal_texts(&self, head_only: bool) -> Result<Vec<LegalTextCatalogRow>> {
        let head_filter = if head_only {
            "AND split_part(t.jurisdiction_paths[2], '.', 2) = ANY($1)"
        } else {
            ""
        };
        let code_types: Vec<&str> = match head_only {
            true => lj_norm::norm_fond::fond_types("codes"),
            false => Vec::new(),
        };
        let (valid_a, norm_a) = (article_valid_now("a"), article_is_norm("a"));
        let sql = format!(
            "
                SELECT t.text_uid, t.slug, t.title, t.nature, t.jurisdiction,
                       (SELECT count(DISTINCT a.num_key)
                        FROM legal_article a
                        WHERE a.text_uid = t.text_uid
                          AND {valid_a} AND {norm_a} AND a.num <> '') AS article_count
                FROM legal_text t
                WHERE {NORM_SCOPE}
                  -- Catalogue = textes *navigables comme un code* : codes
                  -- (`code*`/`CODE`), constitutions, lois/ordonnances. On écarte les
                  -- actes unitaires (arrêtés, décrets) et les conventions collectives
                  -- (`TI`/`IDCC`) : cités, mais pas parcourus comme un corpus.
                  AND (t.nature ILIKE 'code%'
                       OR upper(t.nature) IN ('CONSTITUTION', 'LOI_CONSTIT', 'LOI',
                           'LOI_ORGANIQUE', 'ORDONNANCE', 'DECRET_LOI', 'REGLEMENT',
                           'ETAT_CIVIL'))
                  {head_filter}
                  -- … et seulement ceux qui ont AU MOINS un article en vigueur : on
                  -- écarte les versions abrogées/transférées (toutes fenêtres closes
                  -- → 0 en vigueur) et les coquilles sans articles. Elles
                  -- doublonnent le code vivant avec un « 0 article » non navigable.
                  AND EXISTS (SELECT 1 FROM legal_article a
                              WHERE a.text_uid = t.text_uid
                                AND {valid_a} AND {norm_a} AND a.num <> '')
                ORDER BY t.title
                "
        );
        let rows = match head_only {
            true => self.conn.query(&sql, &[&code_types]).await?,
            false => self.conn.query(&sql, &[]).await?,
        };
        Ok(rows
            .iter()
            .map(|r| LegalTextCatalogRow {
                text_uid: r.get(0),
                slug: r.get(1),
                title: r.get(2),
                nature: r.get(3),
                jurisdiction: r.get(4),
                article_count: r.get(5),
            })
            .collect())
    }

    /// Table des matières d'un code : la version en vigueur de chaque article (une
    /// ligne par `num_key`), dans l'ordre de lecture persisté (`reading_rank` —
    /// `position` de la source quand elle existe, sinon divisions par clé médiane
    /// et tri naturel des `num_key`). Léger, sans corps. Champs bruts (mapping
    /// `TocEntry` côté `lj-api`). `expired` dit la fenêtre close de la version
    /// rendue : KALI laisse `status` à `VIGUEUR` sur des versions dépassées.
    #[tracing::instrument(name = "db.code_table_of_contents", skip(self), fields(db.system = "postgresql"))]
    pub async fn code_table_of_contents(&self, text_uid: &str) -> Result<Vec<TocArticleRow>> {
        let (valid_a, norm_a) = (article_valid_now("a"), article_is_norm("a"));
        let rows = self
            .conn
            .query(
                &format!(
                    "
                SELECT num, num_key, title_path, status, position, expired FROM (
                  -- Fenêtre du jour préférée, sinon dernière version ; vraie
                  -- version avant moignon, consolidée avant publication.
                  SELECT DISTINCT ON (num_key) num, num_key, title_path, status,
                         position, reading_rank, date_debut,
                         (a.date_fin IS NOT NULL AND a.date_fin < CURRENT_DATE) AS expired
                  FROM legal_article a
                  WHERE text_uid = $1 AND num <> ''
                  ORDER BY num_key, ({valid_a}) DESC, ({norm_a}) DESC,
                           (source = 'jorf') ASC, date_debut DESC NULLS LAST
                ) v
                ORDER BY reading_rank NULLS LAST, num_key
                "
                ),
                &[&text_uid],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| TocArticleRow {
                num: r.get(0),
                num_key: r.get(1),
                title_path: r.get(2),
                status: r.get(3),
                position: r.get(4),
                expired: r.get(5),
            })
            .collect())
    }

    /// Vue-lecture à plat d'un texte sans structure ingérée (BOFiP, circulaires…) :
    /// la version en vigueur de chaque article avec son corps, dans le même ordre
    /// de lecture que [`Self::code_table_of_contents`]. Champs au format
    /// [`TocReadingRow`] (que des articles, profondeur 1).
    #[tracing::instrument(name = "db.flat_text_reading", skip(self), fields(db.system = "postgresql"))]
    pub async fn flat_text_reading(&self, text_uid: &str) -> Result<Vec<TocReadingRow>> {
        let (valid_a, norm_a) = (article_valid_now("a"), article_is_norm("a"));
        let rows = self
            .conn
            .query(
                &format!(
                    "
                SELECT num, num_key, status, texte, nota, date_debut, title_path FROM (
                  -- Fenêtre du jour préférée, sinon dernière version ; vraie
                  -- version avant moignon, consolidée avant publication.
                  SELECT DISTINCT ON (num_key) num, num_key, status,
                         texte, nota, reading_rank, date_debut, title_path
                  FROM legal_article a
                  -- The `body` piece reads as the text's body, above its articles.
                  WHERE text_uid = $1 AND num_key <> 'body'
                  ORDER BY num_key, ({valid_a}) DESC, ({norm_a}) DESC,
                           (source = 'jorf') ASC, date_debut DESC NULLS LAST
                ) v
                ORDER BY reading_rank NULLS LAST, num_key
                "
                ),
                &[&text_uid],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let num: String = r.get(0);
                let (child_kind, label) = if num.is_empty() {
                    let path: Option<String> = r.get(6);
                    let title = path
                        .as_deref()
                        .and_then(|p| p.rsplit(" > ").next())
                        .unwrap_or_default();
                    ("piece", title.to_string())
                } else {
                    ("article", num)
                };
                TocReadingRow {
                    depth: 1,
                    child_kind: child_kind.to_string(),
                    child_num_key: Some(r.get(1)),
                    label,
                    etat: r.get(2),
                    texte: r.get(3),
                    nota: r.get(4),
                    date_debut: r.get(5),
                }
            })
            .collect())
    }

    /// Contexte de la vue courante par parcours bornés. L'effectif se compte à
    /// concurrence de `CONTEXT_FULL_MAX + 1`, puis division et fenêtre se lisent en
    /// balayant l'index `(text_uid, reading_rank)` depuis le rang du courant,
    /// plafonné à `CONTEXT_DIVISION_MAX` de chaque côté — une division est une
    /// plage contiguë de rangs, le balayage s'arrête au premier changement de
    /// `title_path`. Les articles au rang encore NULL sont invisibles des balayages.
    #[tracing::instrument(name = "db.article_context", skip(self), fields(db.system = "postgresql"))]
    pub async fn article_context(
        &self,
        text_uid: &str,
        num_key: &str,
    ) -> Result<Vec<ArticleNeighborRow>> {
        let probe = CONTEXT_FULL_MAX as i64 + 1;
        let (valid_a, norm_a, pick_a) = (
            article_valid_now("a"),
            format!("{} AND a.num <> ''", article_is_norm("a")),
            article_pick_order("a"),
        );
        let cur = self
            .conn
            .query_opt(
                &format!(
                    "
                SELECT c.num, c.num_key, c.status, c.reading_rank, c.title_path,
                       (SELECT count(*) FROM (
                          SELECT DISTINCT num_key FROM legal_article a
                          WHERE text_uid = $1 AND {valid_a} AND {norm_a} LIMIT $3
                        ) s) AS n
                FROM (
                  SELECT num, num_key, status, reading_rank, title_path
                  FROM legal_article a
                  WHERE text_uid = $1 AND num_key = $2 AND {valid_a} AND {norm_a}
                  ORDER BY {pick_a} LIMIT 1
                ) c
                "
                ),
                &[&text_uid, &num_key, &probe],
            )
            .await?;
        let Some(cur) = cur else {
            return Ok(Vec::new());
        };

        if cur.get::<_, i64>(5) <= CONTEXT_FULL_MAX as i64 {
            let rows = self
                .conn
                .query(
                    &format!(
                        "
                    SELECT num, num_key, status FROM (
                      SELECT DISTINCT ON (num_key) num, num_key, status, reading_rank
                      FROM legal_article a
                      WHERE text_uid = $1 AND {valid_a} AND {norm_a}
                      ORDER BY num_key, {pick_a}
                    ) s
                    ORDER BY reading_rank NULLS LAST, num_key
                    "
                    ),
                    &[&text_uid],
                )
                .await?;
            return Ok(rows
                .iter()
                .map(|r| {
                    let nk: String = r.get(1);
                    ArticleNeighborRow {
                        num: r.get(0),
                        current: nk == num_key,
                        num_key: nk,
                        status: r.get(2),
                    }
                })
                .collect());
        }

        let current_row = ArticleNeighborRow {
            num: cur.get(0),
            num_key: cur.get(1),
            status: cur.get(2),
            current: true,
        };
        let Some(cur_rank) = cur.get::<_, Option<i32>>(3) else {
            // Rang en cours d'invalidation : impossible de situer l'article dans l'ordre,
            // il s'affiche seul jusqu'au passage du finisseur.
            return Ok(vec![current_row]);
        };
        let cur_title: Option<String> = cur.get(4);

        let cap = CONTEXT_DIVISION_MAX as i64;
        let rows = self
            .conn
            .query(
                &format!(
                    "
                SELECT num, num_key, status, title_path, reading_rank FROM (
                  (SELECT DISTINCT ON (reading_rank) num, num_key, status, title_path, reading_rank
                   FROM legal_article a
                   WHERE text_uid = $1 AND {valid_a} AND {norm_a} AND reading_rank < $2
                   ORDER BY reading_rank DESC, {pick_a} LIMIT $3)
                  UNION ALL
                  (SELECT DISTINCT ON (reading_rank) num, num_key, status, title_path, reading_rank
                   FROM legal_article a
                   WHERE text_uid = $1 AND {valid_a} AND {norm_a} AND reading_rank > $2
                   ORDER BY reading_rank ASC, {pick_a} LIMIT $3)
                ) s
                "
                ),
                &[&text_uid, &cur_rank, &cap],
            )
            .await?;

        struct Neighbor {
            num: String,
            num_key: String,
            status: String,
            title_path: Option<String>,
            rank: i32,
        }
        let (mut before, mut after): (Vec<Neighbor>, Vec<Neighbor>) = (Vec::new(), Vec::new());
        for r in &rows {
            let x = Neighbor {
                num: r.get(0),
                num_key: r.get(1),
                status: r.get(2),
                title_path: r.get(3),
                rank: r.get(4),
            };
            if x.rank < cur_rank {
                before.push(x);
            } else {
                after.push(x);
            }
        }
        // Du plus proche au plus lointain, de chaque côté.
        before.sort_by_key(|x| std::cmp::Reverse(x.rank));
        after.sort_by_key(|x| x.rank);

        let run = |side: &[Neighbor]| {
            side.iter()
                .take_while(|x| x.title_path == cur_title)
                .count()
        };
        let (b_run, a_run) = (run(&before), run(&after));
        let (nb, na) = if cur_title.is_some() && b_run + 1 + a_run <= CONTEXT_DIVISION_MAX {
            (b_run, a_run)
        } else {
            (CONTEXT_WINDOW, CONTEXT_WINDOW)
        };

        let neighbor = |x: &Neighbor| ArticleNeighborRow {
            num: x.num.clone(),
            num_key: x.num_key.clone(),
            status: x.status.clone(),
            current: false,
        };
        let mut out = Vec::with_capacity(nb + 1 + na);
        out.extend(before.iter().take(nb).rev().map(neighbor));
        out.push(current_row);
        out.extend(after.iter().take(na).map(neighbor));
        Ok(out)
    }

    /// Every version of a text's articles, keys in the base's collation order.
    #[tracing::instrument(name = "db.article_versions", skip(self), fields(db.system = "postgresql"))]
    pub async fn article_versions(&self, text_uid: &str) -> Result<ArticleVersions> {
        let rows = self
            .conn
            .query(
                "SELECT num_key, num, status, reading_rank, title_path, date_debut, date_fin
                 FROM legal_article WHERE text_uid = $1
                 ORDER BY num_key, date_debut DESC",
                &[&text_uid],
            )
            .await?;
        let mut interned: HashMap<String, Arc<str>> = HashMap::new();
        let mut intern = |s: String| -> Arc<str> {
            interned
                .entry(s)
                .or_insert_with_key(|s| Arc::from(s.as_str()))
                .clone()
        };
        let mut keys: Vec<String> = Vec::new();
        let mut versions = Vec::with_capacity(rows.len());
        for r in &rows {
            let num_key: &str = r.get(0);
            if keys.last().map(String::as_str) != Some(num_key) {
                keys.push(num_key.to_string());
            }
            versions.push(ArticleVersion {
                key: (keys.len() - 1) as u32,
                num: r.get(1),
                status: intern(r.get(2)),
                reading_rank: r.get(3),
                title_path: r.get::<_, Option<String>>(4).map(&mut intern),
                date_debut: r.get(5),
                date_fin: r.get(6),
            });
        }
        let interned_bytes = interned.keys().map(|k| k.len() + 48).sum();
        Ok(ArticleVersions {
            keys,
            versions,
            interned_bytes,
        })
    }
}

struct ArticleVersion {
    key: u32,
    num: String,
    status: Arc<str>,
    reading_rank: Option<i32>,
    title_path: Option<Arc<str>>,
    date_debut: NaiveDate,
    date_fin: Option<NaiveDate>,
}

impl ArticleVersion {
    fn covers(&self, date: NaiveDate) -> bool {
        self.date_debut <= date && self.date_fin.is_none_or(|f| f >= date)
    }
}

/// Every version of one text's articles, grouped by key, the latest first.
pub struct ArticleVersions {
    /// In the base's collation order, which byte order is not.
    keys: Vec<String>,
    versions: Vec<ArticleVersion>,
    interned_bytes: usize,
}

impl ArticleVersions {
    /// The neighbours of `num_key` at `date`: the whole text when short, else the
    /// division of that day's version, every article whose version of that day
    /// carries its `title_path` wherever today's order puts it, else a window.
    pub fn context_at(&self, num_key: &str, date: NaiveDate) -> Vec<ArticleNeighborRow> {
        let mut served: Vec<&ArticleVersion> = Vec::new();
        let mut current: Option<(u32, Option<&ArticleVersion>)> = None;
        for group in self.versions.chunk_by(|a, b| a.key == b.key) {
            let key = group[0].key;
            if self.keys[key as usize] == num_key {
                current = Some((key, group.iter().find(|v| v.covers(date))));
            }
            if let Some(v) = group.iter().find(|v| !v.num.is_empty() && v.covers(date)) {
                served.push(v);
            }
        }
        let Some((current_key, current_version)) = current else {
            return Vec::new();
        };
        if !served.iter().any(|v| v.key == current_key) {
            return Vec::new();
        }
        let title = current_version.and_then(|v| v.title_path.as_ref());
        let same_division = |v: &ArticleVersion| v.title_path.as_ref() == title;
        served.sort_unstable_by_key(|v| (v.reading_rank.is_none(), v.reading_rank, v.key));
        let at = served
            .iter()
            .position(|v| v.key == current_key)
            .expect("the current article is served");
        let division_size = served.iter().filter(|v| same_division(v)).count();
        let picked: Vec<&ArticleVersion> = if served.len() <= CONTEXT_FULL_MAX {
            served
        } else if title.is_some() && division_size <= CONTEXT_DIVISION_MAX {
            served.into_iter().filter(|v| same_division(v)).collect()
        } else {
            let end = (at + CONTEXT_WINDOW + 1).min(served.len());
            served[at.saturating_sub(CONTEXT_WINDOW)..end].to_vec()
        };
        picked
            .into_iter()
            .map(|v| ArticleNeighborRow {
                num: v.num.clone(),
                num_key: self.keys[v.key as usize].clone(),
                status: v.status.to_string(),
                current: v.key == current_key,
            })
            .collect()
    }

    /// Approximate bytes held, for a cache's weigher.
    pub fn weight(&self) -> usize {
        let keys: usize = self.keys.iter().map(|k| k.len() + 24).sum();
        let versions: usize = self.versions.iter().map(|v| v.num.len() + 88).sum();
        keys + versions + self.interned_bytes
    }
}

/// Entrée du calcul de rang de lecture : la version canonique d'un article,
/// réduite à ce dont l'ordre dépend.
struct ReadingOrderInput {
    num_key: String,
    title_path: Option<String>,
    position: Option<i32>,
    piece: bool,
}

/// Constantes du ranking de la recherche d'articles : boost du titre
/// conjonctif, poids du filet titre OR, constante RRF et profondeur de jambe
/// avant fusion.
const ARTICLE_TITLE_CONJ_BOOST: &str = "4";
const ARTICLE_TITLE_OR_BOOST: &str = "0.25";
/// Boost de la clause `num` ([`article_num_clause`]), multiplicatif sur le BM25
/// de la jambe articles.
const ARTICLE_NUM_BOOST: &str = "2";
/// Poids RRF de la jambe usage_terms : sacs de contextes de citation, votes
/// sommés avec les autres jambes (elle recouvre la jambe articles).
const ARTICLE_USAGE_WEIGHT: &str = "0.8";
/// Poids RRF de la jambe usage scopée au code nommé : grammes du concept seul
/// contre les sacs du code visé.
const ARTICLE_NAMED_USAGE_WEIGHT: &str = "4.0";
/// Poids RRF de la jambe « numéro ∧ titre ». Comme [`ARTICLE_NAMED_USAGE_WEIGHT`],
/// il doit dominer les votes lexicaux partiels : la jambe ne se déclenche que
/// sur une identification explicite — un numéro cité ET un titre qui répond —
/// et sa cible n'a souvent aucun autre vote, la jambe articles la laissant hors
/// de ses 200 premiers.
const ARTICLE_NUM_TITLE_WEIGHT: &str = "4.0";
const ARTICLE_RRF_K: &str = "60";
/// Candidates a cited reference confronts with its title. Wide enough for the
/// homonymous codes a jurisdiction filter leaves, narrow enough to stay a Top-K.
const CITED_ARTICLE_CANDIDATES: i64 = 20;
/// Titles an act number is read against.
const ACT_NUMBER_TITLES: i64 = 2_000;
const IDENTIFIED_TEXTS: usize = 3;
/// The reference list stops here, and its count with it: « article 1 » is
/// carried by nearly every text of the fonds.
const REFERENCE_CARRIERS_LIMIT: i64 = 100;
const ARTICLE_LEG_LIMIT: i64 = 200;
/// « En vigueur » = la fenêtre de validité, seule définition du référentiel :
/// le littéral `status = 'VIGUEUR'` laisse dehors les articles que DILA ne
/// stampe pas, dont tout le fonds JORF. Mêmes bornes que la lecture à date
/// (`law_article_at_date`) : début inconnu `0001-01-01`, fin ouverte `NULL`.
/// `date_debut`/`date_fin` étant des fast fields, le prédicat se pousse entier
/// dans le scan Tantivy.
pub(crate) fn article_valid_now(a: &str) -> String {
    article_valid_at(a, "CURRENT_DATE")
}
/// La même fenêtre à une date arbitraire : `d` est une expression SQL de date
/// (`CURRENT_DATE` ou littéral `DATE '2020-06-15'` rendu depuis un `NaiveDate`
/// — le type ne sait produire que `%Y-%m-%d`, la splice est sûre).
/// An article row is searched when its text has a slug and is no individual
/// measure, vehicle nor habilitation; a `body` piece while its text is in force.
const ARTICLE_SEARCHABLE_SQL: &str = "(t.slug IS NOT NULL \
     AND t.role NOT IN ('individuel', 'vehicule', 'habilitation') \
     AND (a.num_key <> 'body' OR coalesce(t.status, 'VIGUEUR') = 'VIGUEUR'))";
pub(crate) fn article_valid_at(a: &str, d: &str) -> String {
    format!("{a}.date_debut <= {d} AND ({a}.date_fin IS NULL OR {a}.date_fin >= {d})")
}
/// Rend l'expression SQL de la date de consultation : `CURRENT_DATE` en
/// l'absence de date demandée, sinon le littéral ISO du `NaiveDate`.
fn sql_date_expr(at: Option<NaiveDate>) -> String {
    match at {
        Some(d) => format!("DATE '{d}'"),
        None => "CURRENT_DATE".to_string(),
    }
}
/// Seule définition de `legal_article.kind`, posée par
/// [`Repository::refresh_article_denorm`] et lue par la recherche via
/// [`article_is_norm`].
///
/// Un dispositif modificatif est consommé : son effet vit dans sa cible, et
/// LEGI ne garde de la ligne qu'une mention générée de ce qu'elle a modifié.
/// C'est le corps qui le dit, et lui seul.
/// La recherche ne sert que des normes : un edit est lisible sur la fiche de
/// son texte, avec son arête, mais n'est pas un document qu'on cherche.
/// Égalité sur deux valeurs, `kind` étant un littéral de
/// `legal_article_bm25` : poussée en `term`.
pub(crate) fn article_is_norm(a: &str) -> String {
    format!("{a}.kind = 'article'")
}
/// La même fenêtre qu'[`article_valid_at`], en `searchqueryinput` pour le
/// chemin `paradedb.aggregate`. `d` = expression SQL de date
/// ([`sql_date_expr`]). Deux clauses à splicer dans un `must` : `range` sur
/// `date_debut`, et la disjonction `date_fin IS NULL OR date_fin >= $d` en
/// `should` d'un `must_not exists` et d'un `range`. Le `must_not` est adossé à
/// `all()` : un boolean Tantivy sans clause positive ne matche rien.
fn article_valid_agg(d: &str) -> String {
    format!(
        "paradedb.range('date_debut', daterange(NULL, {d}, '(]')), \
         paradedb.boolean(should => ARRAY[\
           paradedb.boolean(must => ARRAY[paradedb.all()], \
                            must_not => ARRAY[paradedb.exists('date_fin')]), \
           paradedb.range('date_fin', daterange({d}, NULL, '[)'))])"
    )
}
/// The lower bounds a listing with none stated widens through, newest first: the
/// latest changes sit in the last days, and a window that holds a page leaves its
/// order exact.
fn list_lookbacks(valid_from: Option<NaiveDate>, at: Option<NaiveDate>) -> Vec<Option<NaiveDate>> {
    match valid_from {
        Some(from) => vec![Some(from)],
        None => {
            let at = at.unwrap_or_else(|| chrono::Utc::now().date_naive());
            [31, 366, 3653]
                .into_iter()
                .map(|days| Some(at - chrono::Days::new(days)))
                .chain([None])
                .collect()
        }
    }
}
/// The article leg of a listing on `legal_article_bm25`: the versions valid at
/// `at` whose known start sits at or after `valid_from`, of texts dated inside
/// the bounds too when `dated`. A start the source left unknown dates nothing
/// here: the text's own date stands for it, on the text leg.
fn listed_article_predicate(
    params: &mut ArticleSearchParams,
    f: &ArticleBrowseFilters<'_>,
    dated: bool,
) -> String {
    let scope = searched_scope("", f.home, f.jurisdictions, f.valid_from, f.at);
    let mut must = agg_jurisdiction(params, &scope);
    let mut must_not = Vec::new();
    push_listed_terms(params, f, &mut must);
    must_not.extend(params.push_category(f.category).must_not("nature"));
    if dated {
        must.push_str(&text_bounds_agg(params, &scope));
    }
    let known_start = match scope.valid_from {
        Some(from) => {
            let ph = params.push(Box::new(from));
            format!("daterange(${ph}::date, NULL, '[)')")
        }
        None => format!("daterange({UNKNOWN_START}, NULL, '()')"),
    };
    let must_not = match must_not.as_slice() {
        [] => String::new(),
        clauses => format!(", must_not => ARRAY[{}]", clauses.join(", ")),
    };
    format!(
        "paradedb.boolean(must => ARRAY[{}, \
         paradedb.term('kind', 'article'), paradedb.term('searchable', true), \
         paradedb.range('date_debut', {known_start}){must}]{must_not})",
        article_valid_agg(&sql_date_expr(scope.at)),
    )
}
/// The text leg of a listing on `legal_text_bm25`: the texts whose own date
/// sits inside the bounds. `source` reads `lower(nature)` there, as on the
/// search's text leg.
fn listed_text_predicate(params: &mut ArticleSearchParams, f: &ArticleBrowseFilters<'_>) -> String {
    let scope = searched_scope("", f.home, f.jurisdictions, f.valid_from, f.at);
    let mut must = agg_jurisdiction(params, &scope);
    let mut must_not = vec![TEXT_ROLE_HIDDEN_TERMS.to_string()];
    push_listed_terms(params, f, &mut must);
    must_not.extend(params.push_category(f.category).must_not("nature_up"));
    let (from, at) = (
        params.push(Box::new(scope.valid_from)),
        params.push(Box::new(
            scope.at.unwrap_or_else(|| chrono::Utc::now().date_naive()),
        )),
    );
    format!(
        "paradedb.boolean(must => ARRAY[paradedb.exists('slug'), \
         paradedb.range('text_date', daterange(${from}::date, ${at}::date, '[]')){must}], \
         must_not => ARRAY[{}])",
        must_not.join(", ")
    )
}
/// The `text_uid` and `source` terms of a listing leg, each opening on its separator.
fn push_listed_terms(
    params: &mut ArticleSearchParams,
    f: &ArticleBrowseFilters<'_>,
    must: &mut String,
) {
    for (field, value) in [("text_uid", f.text_uid), ("source", f.source)] {
        if let Some(v) = value {
            let ph = params.push(Box::new(v.to_string()));
            must.push_str(&format!(", paradedb.term('{field}', ${ph})")); // sql-splice-ok: field name from code
        }
    }
}
/// The texts an article aggregate holds, in all and by order, category, source and path.
struct ListedTexts {
    total: i64,
    jurisdiction: BTreeMap<String, i64>,
    category: BTreeMap<String, i64>,
    source: BTreeMap<String, i64>,
    path: BTreeMap<String, i64>,
}

fn listed_texts_counts(raw: &str) -> Result<ListedTexts> {
    let agg: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| StoreError::Invalid(format!("listed texts: {e}")))?;
    let texts = |v: &serde_json::Value| v["value"].as_f64().unwrap_or(0.0).round() as i64;
    let axis = |name: &str| -> Result<BTreeMap<String, i64>> {
        if agg[name]["sum_other_doc_count"].as_i64() != Some(0) {
            return Err(StoreError::Invalid(format!(
                "listed texts: axis {name} truncated"
            )));
        }
        Ok(agg[name]["buckets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| Some((b["key"].as_str()?.to_string(), texts(&b["texts"]))))
            .collect())
    };
    let mut category: BTreeMap<String, i64> = BTreeMap::new();
    for (jurisdiction, nature, bucket) in order_natures(&agg, "listed texts")? {
        *category
            .entry(lj_norm::category::text_category(jurisdiction, nature).to_string())
            .or_default() += texts(&bucket["texts"]);
    }
    Ok(ListedTexts {
        total: texts(&agg["texts"]),
        jurisdiction: axis("jurisdiction")?,
        category,
        source: axis("source")?,
        path: axis("path")?,
    })
}
/// Départage des lignes d'un même article, après le score dans les jambes de
/// recherche : la version consolidée d'abord, la publication au JO
/// (`source='jorf'`) ensuite, puis la plus récente. Le score prime parce que
/// les deux fonds sont complémentaires — d'un article modificatif, LEGI ne
/// garde qu'un moignon et le lien vers sa cible, le JO garde le texte.
fn article_pick_order(a: &str) -> String {
    format!("({a}.source = 'jorf'), {a}.date_debut DESC")
}
/// A search row read by column name, `status` already coalesced, no score.
fn article_search_row(r: &tokio_postgres::Row) -> ArticleSearchRow {
    ArticleSearchRow {
        text_uid: r.get("text_uid"),
        slug: r.get("slug"),
        code_title: r.get("code_title"),
        num: r.get("num"),
        num_key: r.get("num_key"),
        title_path: r.get("title_path"),
        status: r.get("status"),
        source: r.get("source"),
        date_debut: r.get("date_debut"),
        date_fin: r.get("date_fin"),
        text_date: r.get("text_date"),
        texte: r.get("texte"),
        score: 0.0,
        jurisdiction: r.get("jurisdiction"),
    }
}
/// Agrégation Tantivy des facettes articles (`paradedb.aggregate`, une passe) :
/// un `terms` par axe + le total en `value_count` sur `id` (les `terms`
/// ignorent les docs sans valeur, pas le total). L'axe `slug` ne demande que son
/// top-100, tampon de fusion pour le top-[`ARTICLE_CODE_FACET_LIMIT`] servi.
/// Les trois autres axes sont énumérés en entier et restent exacts, contrôlés
/// par `sum_other_doc_count` au parsing.
const ARTICLE_FACETS_AGG: &str = r#"{"slug":{"terms":{"field":"slug","size":100,"segment_size":5000}},"jurisdiction":{"terms":{"field":"jurisdiction","size":500},"aggs":{"nature":{"terms":{"field":"nature","size":500}}}},"source":{"terms":{"field":"source","size":500}},"path":{"terms":{"field":"jurisdiction_paths","size":1000}},"total":{"value_count":{"field":"id"}}}"#;
const ARTICLE_FACETS_BUCKET_LIMIT: i64 = 1_000_000;
/// Texts, not versions, on the article leg of a listing: `cardinality` of
/// `text_uid`, whole and per order, nature, source and path.
const LISTED_TEXTS_AGG: &str = r#"{"texts":{"cardinality":{"field":"text_uid"}},"jurisdiction":{"terms":{"field":"jurisdiction","size":500},"aggs":{"texts":{"cardinality":{"field":"text_uid"}},"nature":{"terms":{"field":"nature","size":500},"aggs":{"texts":{"cardinality":{"field":"text_uid"}}}}}},"source":{"terms":{"field":"source","size":500},"aggs":{"texts":{"cardinality":{"field":"text_uid"}}}},"path":{"terms":{"field":"jurisdiction_paths","size":1000},"aggs":{"texts":{"cardinality":{"field":"text_uid"}}}}}"#;
/// The facets of a text listing, on the fast fields of `legal_text_bm25`:
/// `nature` and `source` read two fields, `upper` and `lower` of the same column,
/// where `legal_article` carries two denormalised columns. No `slug` axis: a
/// listed text is its own code.
const TEXT_LIST_FACETS_AGG: &str = r#"{"jurisdiction":{"terms":{"field":"jurisdiction","size":500},"aggs":{"nature":{"terms":{"field":"nature_up","size":500}}}},"source":{"terms":{"field":"source","size":500}},"path":{"terms":{"field":"jurisdiction_paths","size":1000}},"total":{"value_count":{"field":"id"}}}"#;
/// Plafond de l'axe `code` retourné par les stats (top par count).
const ARTICLE_CODE_FACET_LIMIT: usize = 20;
/// What a norm search reads: its corpora, the stated paths when one of them
/// names a type of text (`FR.LOI`), and the bounds on the versions it serves.
struct Searched {
    corpora: lj_norm::jurisdictions::QueryScope,
    typed: Vec<String>,
    /// The earliest entry into force of a version served.
    valid_from: Option<NaiveDate>,
    /// The day read, when stated: nothing that starts after it is served.
    at: Option<NaiveDate>,
}

impl Searched {
    /// The corpora of a citation, which lands whatever the filters say.
    fn corpora(corpora: lj_norm::jurisdictions::QueryScope) -> Self {
        Self {
            corpora,
            typed: Vec::new(),
            valid_from: None,
            at: None,
        }
    }
}

/// The corpora a query searches: the orders of the stated paths, else those it
/// designates from `home`. The country token it names comes out of it either way.
fn searched_scope(
    query: &str,
    home: &[&str],
    stated: &[&str],
    valid_from: Option<NaiveDate>,
    at: Option<NaiveDate>,
) -> Searched {
    let named = lj_norm::jurisdictions::query_scope(query, home);
    let corpora = match stated {
        [] => named,
        stated => {
            let mut orders: Vec<String> = Vec::new();
            for path in stated {
                let order = path.split_once('.').map_or(*path, |(order, _)| order);
                if !orders.iter().any(|o| o == order) {
                    orders.push(order.to_string());
                }
            }
            lj_norm::jurisdictions::QueryScope {
                jurisdictions: orders,
                stripped: named.stripped,
            }
        }
    };
    let typed = match stated.iter().any(|p| p.contains('.')) {
        true => stated.iter().map(|p| p.to_string()).collect(),
        false => Vec::new(),
    };
    Searched {
        corpora,
        typed,
        valid_from,
        at,
    }
}

/// The jurisdiction clauses of an aggregate leg, each opening on its separator.
fn agg_jurisdiction(params: &mut ArticleSearchParams, searched: &Searched) -> String {
    let mut should = |field: &'static str, values: &[String]| {
        let terms: Vec<String> = values
            .iter()
            .map(|v| {
                let ph = params.push(Box::new(v.clone()));
                format!("paradedb.term('{field}', ${ph})") // sql-splice-ok: field name from code
            })
            .collect();
        format!(", paradedb.boolean(should => ARRAY[{}])", terms.join(", "))
    };
    let mut clauses = should("jurisdiction", &searched.corpora.jurisdictions);
    if !searched.typed.is_empty() {
        clauses.push_str(&should("jurisdiction_paths", &searched.typed));
    }
    clauses
}

/// A version start the source left unknown, which the text's own date stands for.
const UNKNOWN_START: &str = "DATE '0001-01-01'";

/// The bounds of [`Searched`] on an article aggregate, each opening on its
/// separator: [`ArticleSearchParams::push_bounds`] as `searchqueryinput`.
fn article_bounds_agg(params: &mut ArticleSearchParams, searched: &Searched) -> String {
    let mut clauses = String::new();
    if let Some(from) = searched.valid_from {
        let ph = params.push(Box::new(from));
        clauses.push_str(&format!(
            ", paradedb.boolean(should => ARRAY[\
               paradedb.range('date_debut', daterange(${ph}::date, NULL, '[)')), \
               paradedb.boolean(must => ARRAY[\
                 paradedb.range('date_debut', daterange({UNKNOWN_START}, {UNKNOWN_START}, '[]')), \
                 paradedb.range('text_date', daterange(${ph}::date, NULL, '[)'))])])"
        ));
    }
    if let Some(at) = searched.at {
        let ph = params.push(Box::new(at));
        clauses.push_str(&format!(
            ", paradedb.boolean(should => ARRAY[\
               paradedb.range('date_debut', daterange({UNKNOWN_START}, NULL, '()')), \
               paradedb.boolean(must => ARRAY[paradedb.all()], \
                                must_not => ARRAY[paradedb.exists('text_date')]), \
               paradedb.range('text_date', daterange(NULL, ${ph}::date, '(]'))])"
        ));
    }
    clauses
}

/// The bounds of [`Searched`] on a text aggregate: its own date.
fn text_bounds_agg(params: &mut ArticleSearchParams, searched: &Searched) -> String {
    match (searched.valid_from, searched.at) {
        (None, None) => String::new(),
        (Some(from), at) => {
            let (from, at) = (params.push(Box::new(from)), params.push(Box::new(at)));
            format!(", paradedb.range('text_date', daterange(${from}::date, ${at}::date, '[]'))")
        }
        (None, Some(at)) => {
            let ph = params.push(Box::new(at));
            format!(
                ", paradedb.boolean(should => ARRAY[\
                   paradedb.boolean(must => ARRAY[paradedb.all()], \
                                    must_not => ARRAY[paradedb.exists('text_date')]), \
                   paradedb.range('text_date', daterange(NULL, ${ph}::date, '(]'))])"
            )
        }
    }
}

/// Natures « navigables comme un code » — miroir du filtre du catalogue
/// `/api/codes`, alias `t` requis. Borne la jambe conteneurs.
/// The facet axes of an aggregate, in the order [`ArticleFacetMember`] holds them.
const FACET_AXES: [&str; 5] = ["slug", "jurisdiction", "category", "source", "path"];

/// The nature buckets under each order bucket of an aggregate: `(order, nature, bucket)`.
fn order_natures<'a>(
    agg: &'a serde_json::Value,
    what: &str,
) -> Result<Vec<(&'a str, &'a str, &'a serde_json::Value)>> {
    let mut out = Vec::new();
    for order in agg["jurisdiction"]["buckets"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let natures = &order["nature"];
        if natures["sum_other_doc_count"].as_i64() != Some(0) {
            return Err(StoreError::Invalid(format!("{what}: natures truncated")));
        }
        let Some(jurisdiction) = order["key"].as_str() else {
            return Err(StoreError::Invalid(format!(
                "{what}: order bucket without key"
            )));
        };
        for bucket in natures["buckets"].as_array().into_iter().flatten() {
            let Some(nature) = bucket["key"].as_str() else {
                return Err(StoreError::Invalid(format!(
                    "{what}: nature bucket without key"
                )));
            };
            out.push((jurisdiction, nature, bucket));
        }
    }
    Ok(out)
}

/// Reads one `paradedb.aggregate` result into a member. `slug` is capped inside
/// the aggregation itself, so a remainder is expected there; on an axis
/// enumerated whole a remainder is silently wrong counts, hence the hard error.
fn facet_member(agg: &serde_json::Value, membre: &str) -> Result<ArticleFacetMember> {
    let mut axes: [std::collections::BTreeMap<String, i64>; 5] = Default::default();
    for (jurisdiction, nature, bucket) in order_natures(agg, membre)? {
        *axes[2]
            .entry(lj_norm::category::text_category(jurisdiction, nature).to_string())
            .or_default() += bucket["doc_count"].as_i64().unwrap_or(0);
    }
    for (map, axis) in axes.iter_mut().zip(FACET_AXES) {
        if axis == "category" {
            continue;
        }
        let terms = &agg[axis];
        if axis != "slug" && terms["sum_other_doc_count"].as_i64() != Some(0) {
            return Err(StoreError::Invalid(format!(
                "agrégat facettes {membre}: axe {axis} tronqué"
            )));
        }
        for bucket in terms["buckets"].as_array().into_iter().flatten() {
            let (Some(value), Some(count)) = (bucket["key"].as_str(), bucket["doc_count"].as_i64())
            else {
                return Err(StoreError::Invalid(format!(
                    "agrégat facettes {membre}: bucket inattendu sur l'axe {axis}"
                )));
            };
            if !value.is_empty() {
                *map.entry(value.to_string()).or_default() += count;
            }
        }
    }
    Ok(ArticleFacetMember {
        total: agg["total"]["value"].as_f64().unwrap_or(0.0) as i64,
        axes,
    })
}

/// Sums the members by axis value. The `code` axis is capped at its top-N on the
/// way out: the whole enumeration serves the exact counts, it is not served.
pub fn merge_article_facets(
    members: impl IntoIterator<Item = ArticleFacetMember>,
) -> ArticleSearchStats {
    let mut total = 0;
    let mut axes: [std::collections::BTreeMap<String, i64>; 5] = Default::default();
    for member in members {
        total += member.total;
        for (map, part) in axes.iter_mut().zip(member.axes) {
            for (value, count) in part {
                *map.entry(value).or_default() += count;
            }
        }
    }
    let [mut code, jurisdiction, category, source, path] = axes.map(|map| {
        let mut axis: Vec<FacetCount> = map
            .into_iter()
            .map(|(value, count)| FacetCount { value, count })
            .collect();
        axis.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
        axis
    });
    code.truncate(ARTICLE_CODE_FACET_LIMIT);
    ArticleSearchStats {
        total,
        code,
        jurisdiction,
        category,
        source,
        path,
    }
}

const NAVIGABLE_TEXT_NATURES_SQL: &str = "(t.nature ILIKE 'code%' \
     OR upper(t.nature) IN ('CONSTITUTION', 'LOI_CONSTIT', 'LOI', \
         'LOI_ORGANIQUE', 'ORDONNANCE', 'DECRET_LOI', 'REGLEMENT', \
         'ETAT_CIVIL'))";

/// Visibilité par défaut, alias `t` requis : la recherche ne sert pas les
/// parutions sans objet consultable — actes individuels (nominations…),
/// véhicules de publication, lois d'habilitation. Elles restent résolvables par
/// uid.
const TEXT_ROLE_VISIBLE_SQL: &str = "t.role NOT IN ('individuel', 'vehicule', 'habilitation')";

/// A text with something to read, a row in `legal_article`, alias `t` required.
const TEXT_READABLE_SQL: &str = "EXISTS (SELECT 1 FROM legal_article a \
     WHERE a.text_uid = t.text_uid)";

/// Miroir Tantivy de [`TEXT_ROLE_VISIBLE_SQL`] : les clauses `must_not` d'un
/// `searchqueryinput`, sur le champ fast `role`. Même liste de rôles — les deux
/// formes se lisent ensemble.
const TEXT_ROLE_HIDDEN_TERMS: &str = "paradedb.term('role', 'individuel'), \
     paradedb.term('role', 'vehicule'), paradedb.term('role', 'habilitation')";

/// Juridictions primaires d'une requête : le droit applicable en France
/// (FR/EU/INTL) + les pays que la requête nomme
/// ([`lj_norm::jurisdictions::query_jurisdictions`]).
fn primary_jurisdictions(query: &str) -> Vec<String> {
    let mut primary: Vec<String> = ["FR", "EU", "INTL"].map(String::from).to_vec();
    primary.extend(
        lj_norm::jurisdictions::query_jurisdictions(query)
            .into_iter()
            .map(String::from),
    );
    primary
}

/// The BM25 predicate of the articles leg of the article search and of its
/// facets, on `legal_article_bm25`: a `should` of the normalised conjunctive
/// title, boosted, one per query or expansion alternative and each matching the
/// whole title, the OR title net at a low weight ($1), the text ($2) and the
/// cited number. Pushes the conjunctive alternatives into `params` ($3..); a
/// query reduced to nothing keeps the OR net alone. Returns the bare
/// `searchqueryinput` expression, without `id @@@`.
fn article_search_predicate(
    params: &mut ArticleSearchParams,
    query: &str,
    expansions: &[String],
) -> String {
    let alt_phs = article_conj_alt_phs(params, query, expansions);
    format!(
        "paradedb.boolean(should => ARRAY[{}, paradedb.match('texte', $2){}])",
        article_title_clauses("search_title", &alt_phs),
        article_num_clause(params, query),
    )
}

/// The scan of the articles leg: the ids its predicate and filters keep, best
/// score first, `limit` of them.
fn article_leg_scan(pred: &str, valid_a: &str, norm_a: &str, filters: &str, limit: i64) -> String {
    format!(
        "SELECT a.id, paradedb.score(a.id) AS score
         FROM legal_article a
         WHERE {pred}
           AND {valid_a}
           AND {norm_a}
           AND a.searchable{filters}
         ORDER BY paradedb.score(a.id) DESC
         LIMIT {limit}"
    )
}

/// Alternatives conjonctives poussées dans `params` ($3..).
fn article_conj_alt_phs(
    params: &mut ArticleSearchParams,
    query: &str,
    expansions: &[String],
) -> Vec<usize> {
    let mut conj_alts: Vec<String> = std::iter::once(query.to_string())
        .chain(expansions.iter().cloned())
        .filter_map(|q| lj_norm::aliases::conj_title_query(&q))
        .collect();
    conj_alts.dedup();
    conj_alts
        .into_iter()
        .map(|a| params.push(Box::new(a)))
        .collect()
}

/// Clauses titre d'une jambe : conjonctif normalisé boosté par alternative,
/// plus le filet titre OR à faible poids ($1). Requête réduite à rien
/// (« article ») → filet OR seul.
fn article_title_clauses(field: &str, alt_phs: &[usize]) -> String {
    let or = format!("paradedb.boost({ARTICLE_TITLE_OR_BOOST}, paradedb.match('{field}', $1))");
    if alt_phs.is_empty() {
        return or;
    }
    let alts: Vec<String> = alt_phs
        .iter()
        .map(|ph| format!("paradedb.match('{field}', ${ph}, conjunction_mode => true)"))
        .collect();
    let conj = match alts.as_slice() {
        [single] => single.clone(),
        many => format!("paradedb.boolean(should => ARRAY[{}])", many.join(", ")),
    };
    format!("paradedb.boost({ARTICLE_TITLE_CONJ_BOOST}, {conj}), {or}")
}

/// The `num` clause of the articles leg: the article number cited in the
/// query, searched on the field that carries it, returned ready to concatenate
/// into a `should` (`, clause`; an empty string when the query cites no number).
///
/// Two forms per number, because the corpus carries both: glued (`L423-7`, a
/// single token) and dotted or starred (`D. 423-7`, two tokens), hence the
/// conjunctive form rendered by [`lj_norm::article_key::display`], which finds
/// them without confusing them (`L. 423-7` does not reach `R*423-7`).
fn article_num_clause(params: &mut ArticleSearchParams, query: &str) -> String {
    match article_num_query(params, query) {
        Some(q) => format!(", paradedb.boost({ARTICLE_NUM_BOOST}, {q})"),
        None => String::new(),
    }
}

/// Le `should` nu des numéros cités, sans boost — pour les deux consommateurs :
/// la clause de la jambe articles ([`article_num_clause`]) et la jambe
/// « numéro ∧ titre ». `None` si la requête ne cite aucun numéro.
fn article_num_query(params: &mut ArticleSearchParams, query: &str) -> Option<String> {
    article_num_query_for(params, &lj_norm::article_key::query_article_keys(query))
}

fn article_num_query_for(params: &mut ArticleSearchParams, keys: &[String]) -> Option<String> {
    let mut alts: Vec<String> = Vec::new();
    for key in keys {
        let mut forms = vec![key.clone()];
        forms.extend(
            [
                lj_norm::article_key::display(key),
                lj_norm::article_key::spaced_ordinal(key).unwrap_or_default(),
                lj_norm::article_key::dotted_form(key).unwrap_or_default(),
            ]
            .into_iter()
            .filter(|f| !f.is_empty() && f != key),
        );
        for form in forms {
            let ph = params.push(Box::new(form));
            alts.push(format!(
                "paradedb.match('num', ${ph}, conjunction_mode => true)"
            ));
        }
    }
    (!alts.is_empty()).then(|| format!("paradedb.boolean(should => ARRAY[{}])", alts.join(", ")))
}

/// Construit la jambe titre : requête enrichie des expansions d'alias (OR
/// implicite du `match`).
fn article_title_query(query: &str, expansions: &[String]) -> String {
    if expansions.is_empty() {
        query.to_string()
    } else {
        format!("{query} {}", expansions.join(" "))
    }
}

/// Accumulateur de paramètres SQL pour la recherche d'articles et ses facettes.
/// `$1`/`$2` sont toujours la jambe titre et la jambe corps ; les filtres
/// optionnels (`text_uid`/`jurisdiction`/`nature`/`source`) puis les bornes de
/// pagination sont poussés à la suite.
struct ArticleSearchParams {
    params: Vec<Box<dyn ToSql + Sync + Send>>,
}

impl ArticleSearchParams {
    fn new(title_query: String, body_query: &str) -> Self {
        Self {
            params: vec![Box::new(title_query), Box::new(body_query.to_string())],
        }
    }

    /// Sans jambe titre ni jambe corps — pour une requête qui ne référence pas
    /// `$1`/`$2` (Postgres refuse un placeholder jamais référencé).
    fn empty() -> Self {
        Self { params: Vec::new() }
    }

    /// Pousse une valeur et renvoie son numéro de placeholder `$N` (1-indexé).
    fn push(&mut self, value: Box<dyn ToSql + Sync + Send>) -> usize {
        self.params.push(value);
        self.params.len()
    }

    /// The clauses binding a leg to the corpora searched, `a.` then `t.`. Hits
    /// and facets both come here, so a count never covers what the hits do not
    /// serve.
    fn push_scope(&mut self, searched: &Searched) -> (String, String) {
        let ph = self.push(Box::new(searched.corpora.jurisdictions.clone()));
        let mut clauses = format!("\n  AND {{0}}.jurisdiction = ANY(${ph})");
        // One `= ANY` per path: the planner pushes that form into the index as a
        // term, and leaves an array overlap (`&&`) to a heap filter.
        let typed: Vec<String> = searched
            .typed
            .iter()
            .map(|path| {
                let ph = self.push(Box::new(path.clone()));
                format!("${ph} = ANY({{0}}.jurisdiction_paths)")
            })
            .collect();
        if !typed.is_empty() {
            clauses.push_str(&format!("\n  AND ({})", typed.join(" OR ")));
        }
        (clauses.replace("{0}", "a"), clauses.replace("{0}", "t"))
    }

    /// The bounds of [`Searched`] on the article versions served: a version by
    /// its entry into force, the text's own date standing for a start the source
    /// left unknown. The form reaches the index whole, so a leg's top-K is taken
    /// inside it.
    fn push_bounds(&mut self, searched: &Searched) -> String {
        let mut art = String::new();
        if let Some(from) = searched.valid_from {
            let ph = self.push(Box::new(from));
            art.push_str(&format!(
                "\n  AND (a.date_debut >= ${ph} \
                 OR (a.date_debut = {UNKNOWN_START} AND a.text_date >= ${ph}))"
            ));
        }
        if let Some(at) = searched.at {
            let ph = self.push(Box::new(at));
            art.push_str(&format!(
                "\n  AND (a.date_debut <> {UNKNOWN_START} \
                 OR a.text_date IS NULL OR a.text_date <= ${ph})"
            ));
        }
        art
    }

    /// Pushes the filters given and returns two `AND …` clauses, empty when no
    /// filter applies: one for the articles leg, one for the containers leg, on
    /// the same placeholders. `source` reads `a.source` on articles and
    /// `lower(t.nature)` on containers. `category` filters both legs: `(c, true)`
    /// keeps category `c`, `(c, false)` drops it, on articles through the
    /// [`CategoryClause`] returned beside.
    fn push_filters(
        &mut self,
        text_uid: Option<&str>,
        source: Option<&str>,
        category: Option<(&str, bool)>,
    ) -> (String, String, CategoryClause) {
        let mut art = String::new();
        let mut txt = String::new();
        for (art_col, txt_col, value) in [
            ("a.text_uid", "t.text_uid", text_uid),
            ("a.source", "lower(t.nature)", source),
        ] {
            if let Some(v) = value {
                let ph = self.push(Box::new(v.to_string()));
                art.push_str(&format!("\n  AND {art_col} = ${ph}"));
                txt.push_str(&format!("\n  AND {txt_col} = ${ph}"));
            }
        }
        let clause = self.push_category(category);
        txt.push_str(&clause.row("t.jurisdiction", "upper(t.nature)"));
        (art, txt, clause)
    }

    /// Binds the cells of `category`, kept (`true`) or dropped.
    fn push_category(&mut self, category: Option<(&str, bool)>) -> CategoryClause {
        let Some((category, keep)) = category else {
            return CategoryClause {
                cells: Vec::new(),
                keep: false,
            };
        };
        let mut bind = |members: lj_norm::category::Members| match members {
            lj_norm::category::Members::In(v) => (self.push(Box::new(strings(&v))), true),
            lj_norm::category::Members::NotIn(v) => (self.push(Box::new(strings(&v))), false),
        };
        let cells = lj_norm::category::category_cells(category)
            .into_iter()
            .map(|c| (bind(c.orders), bind(c.natures)))
            .collect();
        CategoryClause { cells, keep }
    }

    /// Binds one side of a [`PlaceFilter`], rendered per leg by [`PlaceClauses`].
    fn push_places(&mut self, ruled_out: &[String]) -> PlaceClauses {
        PlaceClauses {
            ruled_out: (!ruled_out.is_empty()).then(|| self.push(Box::new(ruled_out.to_vec()))),
        }
    }

    fn refs(&self) -> Vec<&(dyn ToSql + Sync)> {
        // `Send` sur les boxes garde le future de la méthode `Send` (frontière axum
        // Handler) ; on retombe ici sur `&(dyn ToSql + Sync)` attendu par
        // tokio-postgres.
        self.params
            .iter()
            .map(|p| &**p as &(dyn ToSql + Sync))
            .collect()
    }
}

/// One side of a [`PlaceFilter`] bound to its placeholder.
struct PlaceClauses {
    ruled_out: Option<usize>,
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

/// A category an articles leg keeps or drops, as bound cells of orders and natures.
/// Inside a BM25 scan SQL changes the scores where a `must_not` does not: a kept category is the `must_not` of its complement.
struct CategoryClause {
    cells: Vec<((usize, bool), (usize, bool))>,
    keep: bool,
}

impl CategoryClause {
    /// The texts of the category on `nature_field`, as one Tantivy query.
    fn texts(&self, nature_field: &str) -> String {
        let cells: Vec<String> = self
            .cells
            .iter()
            .map(|((o, o_in), (n, n_in))| {
                let (mut must, mut must_not) = (Vec::new(), Vec::new());
                for (field, ph, keep) in [("jurisdiction", o, o_in), (nature_field, n, n_in)] {
                    let set = format!("paradedb.term_set('{field}', ${ph}::text[])"); // sql-splice-ok: field name from code
                    match keep {
                        true => must.push(set),
                        false => must_not.push(set),
                    }
                }
                if must.is_empty() {
                    must.push("paradedb.all()".to_string());
                }
                match must_not.as_slice() {
                    [] => format!("paradedb.boolean(must => ARRAY[{}])", must.join(", ")),
                    _ => format!(
                        "paradedb.boolean(must => ARRAY[{}], must_not => ARRAY[{}])",
                        must.join(", "),
                        must_not.join(", ")
                    ),
                }
            })
            .collect();
        format!("paradedb.boolean(should => ARRAY[{}])", cells.join(", "))
    }

    /// The `must_not` clause that applies the filter, none when there is none.
    fn must_not(&self, nature_field: &str) -> Option<String> {
        if self.cells.is_empty() {
            return None;
        }
        let texts = self.texts(nature_field);
        Some(match self.keep {
            true => format!(
                "paradedb.boolean(must => ARRAY[paradedb.all()], must_not => ARRAY[{texts}])"
            ),
            false => texts,
        })
    }

    /// The Tantivy `query` of a scan on `legal_article_bm25`, filtered.
    fn scan(&self, query: &str) -> String {
        match self.must_not("nature") {
            Some(clause) => {
                format!("paradedb.boolean(must => ARRAY[{query}], must_not => ARRAY[{clause}])")
            }
            None => query.to_string(),
        }
    }

    /// The SQL condition on the columns of a row read after a scan.
    fn row(&self, jurisdiction: &str, nature: &str) -> String {
        if self.cells.is_empty() {
            return String::new();
        }
        let cells: Vec<String> = self
            .cells
            .iter()
            .map(|((o, o_in), (n, n_in))| {
                let op = |keep: &bool| if *keep { "= ANY" } else { "<> ALL" };
                format!(
                    "({jurisdiction} {}(${o}) AND {nature} {}(${n}))",
                    op(o_in),
                    op(n_in)
                )
            })
            .collect();
        let not = if self.keep { "" } else { "NOT " };
        format!("\n  AND {not}({})", cells.join(" OR "))
    }
}

impl PlaceClauses {
    /// The `must_not` clauses of a Tantivy query. A SQL `<> ALL` over the same
    /// list unfolds into one `exists` per text.
    fn must_not(&self) -> Vec<String> {
        self.ruled_out
            .map(|p| format!("paradedb.term_set('text_uid', ${p}::text[])"))
            .into_iter()
            .collect()
    }

    /// The condition on `alias`, a row a leg has already retrieved. An anti-join
    /// rather than `<> ALL`, which the planner estimates element by element.
    fn row(&self, alias: &str) -> String {
        self.ruled_out
            .map(|p| format!("\n  AND {}", Self::absent(p, alias)))
            .unwrap_or_default()
    }

    fn absent(p: usize, alias: &str) -> String {
        format!(
            "NOT EXISTS (SELECT 1 FROM unnest(${p}::text[]) r(uid) WHERE r.uid = {alias}.text_uid)"
        )
    }
}

/// Les six statements de classification des rôles : reset à `instrument` puis
/// cinq passes par signaux sûrs — fonction pure, dumpable pour validation psql.
/// `scoped` borne chaque statement aux `text_uid` d'un lot (`$1`).
///
/// Passes : motifs de titre pour `individuel` et `habilitation` ; pour
/// `vehicule` deux signaux, un décret « portant publication » vide porteur
/// d'une arête `modifie` sortante résolue, et la cible d'une arête `publie`
/// entrante possédée par une fiche instrument. Une loi constitutionnelle est
/// `modificatif` de l'instrument-Constitution.
fn text_role_sqls(scoped: bool) -> [String; 6] {
    // Deux fragments selon l'alias de la passe (`legal_text` nu ou `t`).
    let scope = if scoped {
        " AND text_uid = ANY($1)"
    } else {
        ""
    };
    let scope_t = if scoped {
        " AND t.text_uid = ANY($1)"
    } else {
        ""
    };
    [
        format!(
            "UPDATE legal_text SET role = 'instrument' WHERE role <> 'instrument'{scope}"
        ),
        format!(
            "
            UPDATE legal_text SET role = 'individuel'
            WHERE text_uid LIKE 'JORFTEXT%'{scope}
              AND (title ~* 'portant (nomination|promotion|titularisation|naturalisation|cessation de fonctions|admission à la retraite|acceptation de la démission|détachement|radiation des cadres|radiation \\()'
                   OR title ~* '^avis de vacance'
                   OR title ~* 'accordant la nationalité française|conférant l.honorariat|acceptant la démission|inscription au tableau d.avancement')
            "
        ),
        format!(
            "
            UPDATE legal_text SET role = 'habilitation'
            WHERE text_uid LIKE 'JORFTEXT%'{scope}
              AND title ~* 'autorisant (la ratification|l.approbation|l.adhésion|l.accession|le Président de la République à (ratifier|approuver|adhérer))'
              AND title !~* ' et (portant|modifiant)'
            "
        ),
        format!(
            "
            UPDATE legal_text t SET role = 'vehicule'
            WHERE t.text_uid LIKE 'JORFTEXT%'{scope_t}
              AND t.title ~* 'portant publication'
              AND NOT EXISTS (
                    SELECT 1 FROM legal_article a WHERE a.text_uid = t.text_uid)
              AND EXISTS (
                    SELECT 1 FROM legal_link l
                    WHERE l.owner_text_uid = t.text_uid
                      AND l.verb = 'modifie'
                      AND l.direction = 'outgoing'
                      AND l.target_text_uid IS NOT NULL
                      AND l.target_text_uid <> t.text_uid)
            "
        ),
        format!(
            "
            UPDATE legal_text t SET role = 'vehicule'
            WHERE t.role <> 'vehicule'{scope_t}
              AND EXISTS (
                    SELECT 1 FROM legal_link l
                    WHERE l.verb = 'publie'
                      AND l.direction = 'incoming'
                      AND l.target_text_uid = t.text_uid)
            "
        ),
        format!(
            "UPDATE legal_text SET role = 'modificatif' WHERE nature = 'LOI_CONSTIT'{scope}"
        ),
    ]
}

/// Texte court : on affiche tous les articles (DDHC, courtes lois/traités).
const CONTEXT_FULL_MAX: usize = 40;
/// Division enclosante affichée entière tant qu'elle ne dépasse pas ce seuil.
const CONTEXT_DIVISION_MAX: usize = 80;
/// Demi-fenêtre de repli (par position) quand la division est trop grosse.
const CONTEXT_WINDOW: usize = 6;

#[cfg(test)]
mod tests {
    use super::{text_role_sqls, ArticleVersion, ArticleVersions};
    use chrono::NaiveDate;
    use std::sync::Arc;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    type Row<'a> = (
        &'a str,
        &'a str,
        Option<i32>,
        Option<&'a str>,
        &'a str,
        Option<&'a str>,
    );

    /// `(num_key, num, rank, title_path, start, end)`, keys in collation order,
    /// each key's versions latest first.
    fn text(rows: &[Row]) -> ArticleVersions {
        let mut keys: Vec<String> = Vec::new();
        let versions = rows
            .iter()
            .map(|&(key, num, rank, title, start, end)| {
                if keys.last().map(String::as_str) != Some(key) {
                    keys.push(key.to_string());
                }
                ArticleVersion {
                    key: (keys.len() - 1) as u32,
                    num: num.to_string(),
                    status: Arc::from("VIGUEUR"),
                    reading_rank: rank,
                    title_path: title.map(Arc::from),
                    date_debut: day(start),
                    date_fin: end.map(day),
                }
            })
            .collect();
        ArticleVersions {
            keys,
            versions,
            interned_bytes: 0,
        }
    }

    fn keys(text: &ArticleVersions, key: &str, at: &str) -> Vec<(String, bool)> {
        text.context_at(key, day(at))
            .into_iter()
            .map(|r| (r.num_key, r.current))
            .collect()
    }

    /// Fifty articles ranked 0..50 under `T`, past the whole-text threshold.
    fn long_text(extra: &[Row]) -> ArticleVersions {
        let names: Vec<String> = (0..50).map(|i| format!("a{i:02}")).collect();
        let mut rows: Vec<Row> = names
            .iter()
            .enumerate()
            .map(|(i, k)| {
                (
                    k.as_str(),
                    k.as_str(),
                    Some(i as i32),
                    Some("T"),
                    "2000-01-01",
                    None,
                )
            })
            .collect();
        rows.extend_from_slice(extra);
        text(&rows)
    }

    #[test]
    fn dated_division_gathers_members_out_of_todays_order() {
        let t = long_text(&[
            ("b1", "b1", Some(10), Some("U"), "2001-01-01", None),
            ("b2", "b2", Some(30), Some("U"), "2001-01-01", None),
            // Moved out of U on the day read: the version opening that day wins.
            ("b3", "b3", Some(31), Some("V"), "2003-06-03", None),
            (
                "b3",
                "b3",
                Some(31),
                Some("U"),
                "2001-01-01",
                Some("2003-06-03"),
            ),
            ("b4", "b4", None, Some("U"), "2001-01-01", None),
            ("b5", "", Some(5), Some("U"), "2001-01-01", None),
        ]);
        let expected = |k: &str, cur: bool| (k.to_string(), cur);
        assert_eq!(
            keys(&t, "b2", "2003-06-03"),
            vec![
                expected("b1", false),
                expected("b2", true),
                expected("b4", false)
            ]
        );
        assert_eq!(keys(&t, "b5", "2003-06-03"), vec![]);
    }

    #[test]
    fn dated_context_without_division_is_a_window_of_six() {
        let t = long_text(&[("c1", "c1", Some(20), None, "2001-01-01", None)]);
        let got = keys(&t, "c1", "2010-01-01");
        assert_eq!(got.len(), 13);
        assert_eq!(got[6], ("c1".to_string(), true));
        assert_eq!(got[0].0, "a15");
        assert_eq!(got[12].0, "a26");
    }

    #[test]
    fn text_role_sqls_cover_passes_and_scope() {
        let full = text_role_sqls(false);
        assert!(full.iter().all(|s| !s.contains("ANY($1)")));
        assert!(full[0].contains("role = 'instrument'"));
        assert!(full[1].contains("'individuel'"));
        assert!(full[2].contains("'habilitation'"));
        assert!(full[3].contains("'vehicule'") && full[3].contains("'modifie'"));
        assert!(full[4].contains("'vehicule'") && full[4].contains("'publie'"));
        assert!(full[5].contains("'modificatif'"));
        let scoped = text_role_sqls(true);
        assert!(scoped.iter().all(|s| s.contains("ANY($1)")));
    }
}
