//! The journal issues and the publication of each text, one row per language.
//! A text's row is merged member by member, and a member that reads the same
//! writes nothing.

use std::collections::HashSet;

use chrono::NaiveDate;
use lj_core::normalizer::normalize_chars;
use serde_json::Value;
use tokio_postgres::types::ToSql;

use super::DecisionRepository;
use crate::error::Result;

/// The language a publication row is written in, and the authority of its
/// text: `authentic`, `official`, `translation` or `automatic`.
#[derive(Clone, Copy, Debug)]
pub struct Edition<'a> {
    pub language: &'a str,
    pub status: &'a str,
}

pub struct JournalIssueRow {
    pub issue_uid: String,
    pub journal_code: String,
    pub num: Option<String>,
    pub date_publi: Option<NaiveDate>,
    pub eli: Option<String>,
    pub title: String,
    pub toc: Value,
}

/// A text's place in an issue's table of contents, from 1.
pub struct IssuePlacement {
    pub text_uid: String,
    pub issue_uid: String,
    pub issue_rank: i32,
}

/// What a text's `TEXTE_VERSION` writes, an object keyed by the columns of
/// `text_publication`, `keywords` and `blocks` always present.
pub type PublicationFacts = Value;

/// The publication of a decree that publishes a treaty record still empty.
pub struct TreatyCutCandidate {
    pub instrument_uid: String,
    pub vehicle_uid: String,
    pub date_publi: Option<NaiveDate>,
    pub structure: Value,
    pub sections: Value,
    pub articles: Value,
}

/// The French JO publication of a text with entries in its articles.
pub struct PieceSource {
    pub text_uid: String,
    pub date_publi: Option<NaiveDate>,
    pub structure: Value,
    pub sections: Value,
    pub articles: Value,
    pub blocks: Value,
    /// A protected act, or a decree whose treaty was cut out of it, the treaty
    /// holding its entries: never pieces.
    pub held_out: bool,
    /// A decree publishing an instrument: an entry its structure does not number
    /// is the instrument's, never numbered from its text.
    pub publishes: bool,
}

/// A part of a JO text with no number, an unnumbered `JORFARTI` or a block around
/// the articles, written as a piece of its text.
pub struct JorfPiece {
    pub text_uid: String,
    pub num_key: String,
    pub source_uid: String,
    pub position: i32,
    pub title_path: Option<String>,
    pub date_debut: NaiveDate,
    pub texte: Option<String>,
    pub nota: Option<String>,
    pub content_checksum: u64,
}

/// A publication read off a journal's own site. `body` is `None` when a row
/// of `legal_text` holds the text.
pub struct PublicationRow {
    pub text_uid: String,
    pub journal_code: String,
    pub nature: Option<String>,
    pub nor: Option<String>,
    pub title: String,
    pub date_texte: Option<NaiveDate>,
    pub date_publi: Option<NaiveDate>,
    pub url: String,
    pub body: Option<String>,
}

impl DecisionRepository<'_> {
    #[tracing::instrument(name = "db.upsert_journal_issues", skip_all, fields(db.system = "postgresql"))]
    pub async fn upsert_journal_issues(
        &self,
        edition: Edition<'_>,
        rows: &[JournalIssueRow],
    ) -> Result<u64> {
        let uids: Vec<&str> = rows.iter().map(|r| r.issue_uid.as_str()).collect();
        let journals: Vec<&str> = rows.iter().map(|r| r.journal_code.as_str()).collect();
        let nums: Vec<Option<&str>> = rows.iter().map(|r| r.num.as_deref()).collect();
        let dates: Vec<Option<NaiveDate>> = rows.iter().map(|r| r.date_publi).collect();
        let elis: Vec<Option<&str>> = rows.iter().map(|r| r.eli.as_deref()).collect();
        let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
        let tocs: Vec<&Value> = rows.iter().map(|r| &r.toc).collect();
        Ok(self
            .conn
            .execute(
                "INSERT INTO journal_issue AS i
                     (issue_uid, language, status, journal_code, num, date_publi, eli, title, toc)
                 SELECT v.uid, $8, $9, v.journal, v.num, v.date, v.eli, v.title, v.toc
                 FROM unnest($1::text[], $2::text[], $3::text[], $4::date[],
                             $5::text[], $6::text[], $7::jsonb[])
                      AS v(uid, journal, num, date, eli, title, toc)
                 ON CONFLICT (issue_uid, language) DO UPDATE SET
                     status = EXCLUDED.status,
                     journal_code = EXCLUDED.journal_code, num = EXCLUDED.num,
                     date_publi = EXCLUDED.date_publi, eli = EXCLUDED.eli,
                     title = EXCLUDED.title, toc = EXCLUDED.toc
                 WHERE (i.status, i.journal_code, i.num, i.date_publi, i.eli, i.title, i.toc)
                       IS DISTINCT FROM
                       (EXCLUDED.status, EXCLUDED.journal_code, EXCLUDED.num,
                        EXCLUDED.date_publi, EXCLUDED.eli, EXCLUDED.title, EXCLUDED.toc)",
                &[
                    &uids,
                    &journals,
                    &nums,
                    &dates,
                    &elis,
                    &titles,
                    &tocs,
                    &edition.language,
                    &edition.status,
                ],
            )
            .await?)
    }

    /// Each text once: the issues these placements name are written.
    #[tracing::instrument(name = "db.place_publications", skip_all, fields(db.system = "postgresql"))]
    pub async fn place_publications(
        &self,
        edition: Edition<'_>,
        rows: &[IssuePlacement],
    ) -> Result<u64> {
        let texts: Vec<&str> = rows.iter().map(|r| r.text_uid.as_str()).collect();
        let issues: Vec<&str> = rows.iter().map(|r| r.issue_uid.as_str()).collect();
        let ranks: Vec<i32> = rows.iter().map(|r| r.issue_rank).collect();
        Ok(self
            .conn
            .execute(
                "INSERT INTO text_publication AS t
                     (text_uid, language, status, issue_uid, issue_rank)
                 SELECT v.text_uid, $4, $5, v.issue_uid, v.issue_rank
                 FROM unnest($1::text[], $2::text[], $3::int[]) AS v(text_uid, issue_uid, issue_rank)
                 ON CONFLICT (text_uid, language) DO UPDATE SET
                     issue_uid = EXCLUDED.issue_uid, issue_rank = EXCLUDED.issue_rank
                 WHERE (t.issue_uid, t.issue_rank)
                       IS DISTINCT FROM (EXCLUDED.issue_uid, EXCLUDED.issue_rank)",
                &[&texts, &issues, &ranks, &edition.language, &edition.status],
            )
            .await?)
    }

    /// Each text once. A row naming no nature keeps the one `upsert_publications` wrote.
    #[tracing::instrument(name = "db.upsert_publication_facts", skip_all, fields(db.system = "postgresql"))]
    pub async fn upsert_publication_facts(
        &self,
        edition: Edition<'_>,
        rows: &[PublicationFacts],
    ) -> Result<u64> {
        let rows = Value::Array(rows.to_vec());
        Ok(self
            .conn
            .execute(
                "INSERT INTO text_publication AS t
                     (text_uid, language, status, nature, num_parution, num_sequence, origine_publi,
                      date_publi, eli, page_deb, page_fin, ministere, autorite, title,
                      title_full, keywords, enterprise, blocks)
                 SELECT r.text_uid, $2, $3, r.nature, r.num_parution, r.num_sequence, r.origine_publi,
                        r.date_publi, r.eli, r.page_deb, r.page_fin, r.ministere, r.autorite,
                        r.title, r.title_full, r.keywords, r.enterprise, r.blocks
                 FROM jsonb_populate_recordset(NULL::text_publication, $1::jsonb) r
                 ON CONFLICT (text_uid, language) DO UPDATE SET
                     status = EXCLUDED.status, nature = coalesce(EXCLUDED.nature, t.nature),
                     num_parution = EXCLUDED.num_parution, num_sequence = EXCLUDED.num_sequence,
                     origine_publi = EXCLUDED.origine_publi, date_publi = EXCLUDED.date_publi,
                     eli = EXCLUDED.eli, page_deb = EXCLUDED.page_deb,
                     page_fin = EXCLUDED.page_fin, ministere = EXCLUDED.ministere,
                     autorite = EXCLUDED.autorite, title = EXCLUDED.title,
                     title_full = EXCLUDED.title_full, keywords = EXCLUDED.keywords,
                     enterprise = EXCLUDED.enterprise, blocks = EXCLUDED.blocks
                 WHERE (t.status, t.nature, t.num_parution, t.num_sequence, t.origine_publi,
                        t.date_publi, t.eli, t.page_deb, t.page_fin, t.ministere, t.autorite,
                        t.title, t.title_full, t.keywords, t.enterprise, t.blocks)
                       IS DISTINCT FROM
                       (EXCLUDED.status, coalesce(EXCLUDED.nature, t.nature), EXCLUDED.num_parution,
                        EXCLUDED.num_sequence, EXCLUDED.origine_publi,
                        EXCLUDED.date_publi, EXCLUDED.eli, EXCLUDED.page_deb,
                        EXCLUDED.page_fin, EXCLUDED.ministere, EXCLUDED.autorite,
                        EXCLUDED.title, EXCLUDED.title_full, EXCLUDED.keywords,
                        EXCLUDED.enterprise, EXCLUDED.blocks)",
                &[&rows, &edition.language, &edition.status],
            )
            .await?)
    }

    /// The body of a text's publication in `edition`'s language, `false` when it has none.
    #[tracing::instrument(name = "db.set_publication_body", skip(self, body), fields(db.system = "postgresql"))]
    pub async fn set_publication_body(
        &self,
        edition: Edition<'_>,
        text_uid: &str,
        body: &str,
    ) -> Result<bool> {
        Ok(self
            .conn
            .execute(
                "UPDATE text_publication SET body = $3 \
                 WHERE text_uid = $1 AND language = $2 AND body IS DISTINCT FROM $3",
                &[&text_uid, &edition.language, &body],
            )
            .await?
            > 0)
    }

    /// Each text once.
    #[tracing::instrument(name = "db.upsert_publications", skip_all, fields(db.system = "postgresql"))]
    pub async fn upsert_publications(
        &self,
        edition: Edition<'_>,
        rows: &[PublicationRow],
    ) -> Result<u64> {
        let uids: Vec<&str> = rows.iter().map(|r| r.text_uid.as_str()).collect();
        let journals: Vec<&str> = rows.iter().map(|r| r.journal_code.as_str()).collect();
        let natures: Vec<Option<&str>> = rows.iter().map(|r| r.nature.as_deref()).collect();
        let nors: Vec<Option<&str>> = rows.iter().map(|r| r.nor.as_deref()).collect();
        let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
        let signed: Vec<Option<NaiveDate>> = rows.iter().map(|r| r.date_texte).collect();
        let published: Vec<Option<NaiveDate>> = rows.iter().map(|r| r.date_publi).collect();
        let urls: Vec<&str> = rows.iter().map(|r| r.url.as_str()).collect();
        let bodies: Vec<Option<&str>> = rows.iter().map(|r| r.body.as_deref()).collect();
        Ok(self
            .conn
            .execute(
                "INSERT INTO text_publication AS t
                     (text_uid, language, status, journal_code, nature, nor, title, date_texte,
                      date_publi, url, body)
                 SELECT v.uid, $10, $11, v.journal, v.nature, v.nor, v.title, v.signed,
                        v.published, v.url, v.body
                 FROM unnest($1::text[], $2::text[], $3::text[], $4::text[], $5::text[],
                             $6::date[], $7::date[], $8::text[], $9::text[])
                      AS v(uid, journal, nature, nor, title, signed, published, url, body)
                 ON CONFLICT (text_uid, language) DO UPDATE SET
                     status = EXCLUDED.status,
                     journal_code = EXCLUDED.journal_code, nature = EXCLUDED.nature,
                     nor = EXCLUDED.nor, title = EXCLUDED.title,
                     date_texte = EXCLUDED.date_texte, date_publi = EXCLUDED.date_publi,
                     url = EXCLUDED.url, body = EXCLUDED.body
                 WHERE (t.status, t.journal_code, t.nature, t.nor, t.title, t.date_texte,
                        t.date_publi, t.url, t.body)
                       IS DISTINCT FROM
                       (EXCLUDED.status, EXCLUDED.journal_code, EXCLUDED.nature, EXCLUDED.nor, EXCLUDED.title,
                        EXCLUDED.date_texte, EXCLUDED.date_publi, EXCLUDED.url, EXCLUDED.body)",
                &[
                    &uids,
                    &journals,
                    &natures,
                    &nors,
                    &titles,
                    &signed,
                    &published,
                    &urls,
                    &bodies,
                    &edition.language,
                    &edition.status,
                ],
            )
            .await?)
    }

    /// Each text once, with its items in published order.
    #[tracing::instrument(name = "db.upsert_publication_structures", skip_all, fields(db.system = "postgresql"))]
    pub async fn upsert_publication_structures(
        &self,
        edition: Edition<'_>,
        rows: &[(String, Value)],
    ) -> Result<u64> {
        let (texts, structures): (Vec<&str>, Vec<&Value>) =
            rows.iter().map(|(t, s)| (t.as_str(), s)).unzip();
        Ok(self
            .conn
            .execute(
                "INSERT INTO text_publication AS t (text_uid, language, status, structure)
                 SELECT v.text_uid, $3, $4, v.structure
                 FROM unnest($1::text[], $2::jsonb[]) AS v(text_uid, structure)
                 ON CONFLICT (text_uid, language) DO UPDATE SET structure = EXCLUDED.structure
                 WHERE t.structure IS DISTINCT FROM EXCLUDED.structure",
                &[&texts, &structures, &edition.language, &edition.status],
            )
            .await?)
    }

    /// Each text once, with an object of its sections keyed by `JORFSCTA`.
    pub async fn merge_publication_sections(
        &self,
        edition: Edition<'_>,
        rows: &[(String, Value)],
    ) -> Result<u64> {
        self.merge_publication_map("sections", edition, rows).await
    }

    /// Each text once, with an object of its articles keyed by `JORFARTI`.
    pub async fn merge_publication_articles(
        &self,
        edition: Edition<'_>,
        rows: &[(String, Value)],
    ) -> Result<u64> {
        self.merge_publication_map("articles", edition, rows).await
    }

    /// The decree publishing a treaty record with no body, when it is the
    /// record's only one, its unnumbered pieces merged into the articles of its
    /// French publication. The record holds no article, or with `held` only
    /// articles cut from the JO.
    #[tracing::instrument(name = "db.treaty_cut_candidates", skip_all, fields(db.system = "postgresql"))]
    pub async fn treaty_cut_candidates(
        &self,
        vehicles: Option<&[String]>,
        held: bool,
    ) -> Result<Vec<TreatyCutCandidate>> {
        Ok(self
            .conn
            .query(
                "SELECT c.* FROM (
                   SELECT l.owner_text_uid, p.text_uid, p.date_publi,
                          coalesce(p.structure, '[]'::jsonb), coalesce(p.sections, '{}'::jsonb),
                          coalesce(p.articles, '{}'::jsonb) || coalesce(
                            (SELECT jsonb_object_agg(a.source_uid, jsonb_build_object('text', a.texte))
                             FROM legal_article a
                             WHERE a.text_uid = p.text_uid AND a.num = '' AND a.texte IS NOT NULL),
                            '{}'::jsonb) AS articles
                   FROM legal_link l
                   JOIN text_publication p ON p.text_uid = l.target_text_uid AND p.language = 'fr'
                   JOIN legal_text t ON t.text_uid = l.owner_text_uid
                   WHERE l.verb = 'publie' AND l.direction = 'incoming'
                     AND l.owner_text_uid LIKE 'INTL/%'
                     AND ($1::text[] IS NULL OR p.text_uid = ANY($1))
                     AND NOT EXISTS (SELECT 1 FROM legal_article a
                                     WHERE a.text_uid = l.owner_text_uid
                                       AND (NOT $2 OR a.source <> 'jorf'))
                     AND NOT EXISTS (SELECT 1 FROM legal_link o
                                     WHERE o.owner_text_uid = l.owner_text_uid
                                       AND o.verb = 'publie' AND o.direction = 'incoming'
                                       AND o.target_text_uid <> l.target_text_uid)
                 ) c
                 WHERE c.articles <> '{}'::jsonb
                 ORDER BY 1",
                &[&vehicles, &held],
            )
            .await?
            .iter()
            .map(|r| TreatyCutCandidate {
                instrument_uid: r.get(0),
                vehicle_uid: r.get(1),
                date_publi: r.get(2),
                structure: r.get(3),
                sections: r.get(4),
                articles: r.get(5),
            })
            .collect())
    }

    /// The decree's unnumbered pieces a treaty record now holds, removed from
    /// the decree; its publication keeps them as printed.
    #[tracing::instrument(name = "db.release_vehicle_pieces", skip(self, ids), fields(db.system = "postgresql"))]
    pub async fn release_vehicle_pieces(&self, vehicle_uid: &str, ids: &[&str]) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "DELETE FROM legal_article
                 WHERE text_uid = $1 AND num = '' AND source_uid = ANY($2)",
                &[&vehicle_uid, &ids],
            )
            .await?)
    }

    /// The `JORFARTI` among `uids` that `legal_article` holds as numbered rows.
    #[tracing::instrument(name = "db.jorf_articles_held", skip_all, fields(db.system = "postgresql", uids = uids.len()))]
    pub async fn jorf_articles_held(&self, uids: &[&str]) -> Result<HashSet<String>> {
        Ok(self
            .conn
            .query(
                "SELECT DISTINCT source_uid FROM legal_article
                 WHERE source = 'jorf' AND source_uid = ANY($1) AND num <> ''",
                &[&uids],
            )
            .await?
            .iter()
            .map(|r| r.get(0))
            .collect())
    }

    /// Up to `limit` French JO publications with entries, after `after` in
    /// `text_uid` order and among `only` when given.
    #[tracing::instrument(name = "db.jorf_piece_sources", skip(self, only), fields(db.system = "postgresql"))]
    pub async fn jorf_piece_sources(
        &self,
        after: &str,
        limit: i64,
        only: Option<&[String]>,
    ) -> Result<Vec<PieceSource>> {
        let protected = format!("%{}%", lj_vocab::PROTECTED_ACCESS_MARKER);
        Ok(self
            .conn
            .query(
                "SELECT p.text_uid, p.date_publi, coalesce(p.structure, '[]'::jsonb),
                        p.sections, p.articles, coalesce(p.blocks, '{}'::jsonb),
                        t.title LIKE $4
                          OR EXISTS (SELECT 1 FROM legal_link l
                                     JOIN legal_article a ON a.text_uid = l.owner_text_uid
                                     WHERE l.target_text_uid = p.text_uid
                                       AND l.verb = 'publie' AND l.direction = 'incoming'
                                       AND l.owner_text_uid LIKE 'INTL/%'),
                        EXISTS (SELECT 1 FROM legal_link l
                                WHERE l.target_text_uid = p.text_uid
                                  AND l.verb = 'publie' AND l.direction = 'incoming'
                                  AND l.owner_text_uid NOT LIKE 'JORFTEXT%')
                 FROM text_publication p
                 JOIN legal_text t ON t.text_uid = p.text_uid
                 WHERE p.language = 'fr' AND p.journal_code = 'JORF'
                   AND p.text_uid > $1
                   AND ($3::text[] IS NULL OR p.text_uid = ANY($3))
                   AND p.articles <> '{}'::jsonb
                 ORDER BY p.text_uid
                 LIMIT $2",
                &[&after, &limit, &only, &protected],
            )
            .await?
            .iter()
            .map(|r| PieceSource {
                text_uid: r.get(0),
                date_publi: r.get(1),
                structure: r.get(2),
                sections: r.get(3),
                articles: r.get(4),
                blocks: r.get(5),
                held_out: r.get(6),
                publishes: r.get(7),
            })
            .collect())
    }

    /// Writes `pieces`, a JO piece having one version, and closes each version of
    /// their keys the day before the next one; returns how many it wrote.
    #[tracing::instrument(name = "db.move_jorf_pieces", skip_all, fields(db.system = "postgresql", pieces = pieces.len()))]
    pub async fn move_jorf_pieces(&self, pieces: &[JorfPiece]) -> Result<u64> {
        let mut texts = Vec::with_capacity(pieces.len());
        let mut keys = Vec::with_capacity(pieces.len());
        let mut ids = Vec::with_capacity(pieces.len());
        let mut positions = Vec::with_capacity(pieces.len());
        let mut paths = Vec::with_capacity(pieces.len());
        let mut dates = Vec::with_capacity(pieces.len());
        let mut textes = Vec::with_capacity(pieces.len());
        let mut notas = Vec::with_capacity(pieces.len());
        let mut checksums = Vec::with_capacity(pieces.len());
        for p in pieces {
            texts.push(p.text_uid.as_str());
            keys.push(p.num_key.as_str());
            ids.push(p.source_uid.as_str());
            positions.push(p.position);
            paths.push(p.title_path.as_deref());
            dates.push(p.date_debut);
            textes.push(p.texte.as_deref().map(normalize_chars));
            notas.push(p.nota.as_deref().map(normalize_chars));
            checksums.push(i64::from_ne_bytes(p.content_checksum.to_ne_bytes()));
        }
        let written = self
            .conn
            .execute(
                "WITH v AS (
                   SELECT * FROM unnest($1::text[], $2::text[], $3::text[], $4::int4[],
                                        $5::text[], $6::date[], $7::text[], $8::text[],
                                        $9::int8[])
                     AS v(text_uid, num_key, id, position, title_path, date_debut, texte,
                          nota, checksum)),
                 redated AS (
                   DELETE FROM legal_article a USING v
                   WHERE a.text_uid = v.text_uid AND a.num_key = v.num_key AND a.num = ''
                     AND a.source = 'jorf' AND a.date_debut <> v.date_debut)
                 INSERT INTO legal_article AS a
                   (text_uid, num, num_key, position, title_path, status, date_debut,
                    texte, nota, content_checksum, source, source_uid, translation,
                    source_asof)
                 SELECT v.text_uid, '', v.num_key, v.position, v.title_path, '',
                        v.date_debut, v.texte, v.nota, v.checksum, 'jorf', v.id,
                        'officiel', CURRENT_DATE
                 FROM v
                 ON CONFLICT (text_uid, num_key, date_debut) DO UPDATE SET
                   texte = EXCLUDED.texte,
                   nota = EXCLUDED.nota,
                   content_checksum = EXCLUDED.content_checksum,
                   position = EXCLUDED.position,
                   title_path = EXCLUDED.title_path,
                   source_asof = EXCLUDED.source_asof,
                   reading_rank = NULL
                 WHERE a.num = '' AND a.source = 'jorf'
                   AND (a.content_checksum IS DISTINCT FROM EXCLUDED.content_checksum
                        OR a.position IS DISTINCT FROM EXCLUDED.position
                        OR a.title_path IS DISTINCT FROM EXCLUDED.title_path)",
                &[
                    &texts, &keys, &ids, &positions, &paths, &dates, &textes, &notas, &checksums,
                ],
            )
            .await?;
        self.conn
            .execute(
                "UPDATE legal_article a SET date_fin = v.suivante - 1
                 FROM (SELECT ctid, date_fin,
                              lead(date_debut) OVER (PARTITION BY text_uid, num_key
                                                     ORDER BY date_debut) AS suivante
                       FROM legal_article
                       WHERE (text_uid, num_key) IN (SELECT * FROM unnest($1::text[], $2::text[]))
                         AND status <> 'MODIFIE_MORT_NE') v
                 WHERE a.ctid = v.ctid AND v.suivante IS NOT NULL
                   AND (v.date_fin IS NULL OR v.date_fin >= v.suivante)",
                &[&texts, &keys],
            )
            .await?;
        Ok(written)
    }

    /// Deletes the JO pieces of `texts`, or of every text, whose article the text
    /// holds numbered under the piece's id; returns how many went.
    #[tracing::instrument(name = "db.drop_twinned_jorf_pieces", skip_all, fields(db.system = "postgresql"))]
    pub async fn drop_twinned_jorf_pieces(&self, texts: Option<&[String]>) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "DELETE FROM legal_article a
                 WHERE ($1::text[] IS NULL OR a.text_uid = ANY($1))
                   AND a.source = 'jorf' AND a.num = '' AND a.num_key = lower(a.source_uid)
                   AND EXISTS (SELECT 1 FROM legal_article n
                               WHERE n.source_uid = a.source_uid AND n.text_uid = a.text_uid
                                 AND n.num <> '')",
                &[&texts],
            )
            .await?)
    }

    /// Deletes the JO rows of the articles `ids` under any key but their own id,
    /// a treaty record's cut of them aside; returns how many went.
    #[tracing::instrument(name = "db.drop_rekeyed_jorf_rows", skip_all, fields(db.system = "postgresql", ids = ids.len()))]
    pub async fn drop_rekeyed_jorf_rows(&self, ids: &[String]) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "DELETE FROM legal_article
                 WHERE source = 'jorf' AND source_uid = ANY($1)
                   AND num_key <> lower(source_uid) AND text_uid NOT LIKE 'INTL/%'",
                &[&ids],
            )
            .await?)
    }

    /// Deletes the JO pieces of `texts` whose article the text holds consolidated,
    /// under the key `rows` gives its id, `(text_uid, JORFARTI, num_key)`; returns
    /// how many went.
    #[tracing::instrument(name = "db.drop_consolidated_jorf_pieces", skip_all, fields(db.system = "postgresql", texts = texts.len()))]
    pub async fn drop_consolidated_jorf_pieces(
        &self,
        texts: &[String],
        rows: &[(String, String, String)],
    ) -> Result<u64> {
        let ids: Vec<&str> = rows.iter().map(|r| r.1.as_str()).collect();
        let keys: Vec<&str> = rows.iter().map(|r| r.2.as_str()).collect();
        Ok(self
            .conn
            .execute(
                "DELETE FROM legal_article a
                 WHERE a.text_uid = ANY($1) AND a.source = 'jorf' AND a.num = ''
                   AND a.num_key = lower(a.source_uid)
                   AND EXISTS (SELECT 1 FROM legal_article n
                               WHERE n.text_uid = a.text_uid AND n.num <> ''
                                 AND n.source <> 'jorf' AND n.num_key IN (
                                       SELECT v.num_key
                                       FROM unnest($2::text[], $3::text[]) AS v(id, num_key)
                                       WHERE v.id = a.source_uid))",
                &[&texts, &ids, &keys],
            )
            .await?)
    }

    /// Gives the numbered JORF rows of `texts` their place in the printed text,
    /// `(text_uid, JORFARTI, position)`, and returns how many moved.
    #[tracing::instrument(name = "db.set_jorf_positions", skip_all, fields(db.system = "postgresql", rows = rows.len()))]
    pub async fn set_jorf_positions(&self, rows: &[(String, String, i32)]) -> Result<u64> {
        let texts: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        let ids: Vec<&str> = rows.iter().map(|r| r.1.as_str()).collect();
        let positions: Vec<i32> = rows.iter().map(|r| r.2).collect();
        Ok(self
            .conn
            .execute(
                "UPDATE legal_article a SET position = v.position, reading_rank = NULL
                 FROM unnest($1::text[], $2::text[], $3::int4[]) AS v(text_uid, id, position)
                 WHERE a.text_uid = v.text_uid AND a.source = 'jorf' AND a.source_uid = v.id
                   AND a.num <> '' AND a.position IS DISTINCT FROM v.position",
                &[&texts, &ids, &positions],
            )
            .await?)
    }

    #[tracing::instrument(name = "db.merge_publication_map", skip(self, rows), fields(db.system = "postgresql"))]
    async fn merge_publication_map(
        &self,
        column: &str,
        edition: Edition<'_>,
        rows: &[(String, Value)],
    ) -> Result<u64> {
        let (texts, entries): (Vec<&str>, Vec<&Value>) =
            rows.iter().map(|(t, e)| (t.as_str(), e)).unzip();
        Ok(self
            .conn
            .execute(
                &format!(
                    "INSERT INTO text_publication AS t (text_uid, language, status, {column})
                     SELECT v.text_uid, $3, $4, v.entries
                     FROM unnest($1::text[], $2::jsonb[]) AS v(text_uid, entries)
                     ON CONFLICT (text_uid, language) DO UPDATE SET {column} = t.{column} || EXCLUDED.{column}
                     WHERE EXISTS (SELECT 1 FROM jsonb_each(EXCLUDED.{column}) e
                                   WHERE t.{column} -> e.key IS DISTINCT FROM e.value)"
                ),
                &[&texts, &entries, &edition.language, &edition.status],
            )
            .await?)
    }

    /// Applies a deletion list: the rows of the texts and issues it names, and
    /// the sections and articles it names out of every text's maps.
    #[tracing::instrument(name = "db.delete_publication_members", skip_all, fields(db.system = "postgresql"))]
    pub async fn delete_publication_members(
        &self,
        issues: &[String],
        texts: &[String],
        sections: &[String],
        articles: &[String],
    ) -> Result<u64> {
        let mut deleted = 0;
        if !texts.is_empty() {
            deleted += self
                .conn
                .execute(
                    "DELETE FROM text_publication WHERE text_uid = ANY($1)",
                    &[&texts],
                )
                .await?;
        }
        if !issues.is_empty() {
            deleted += self
                .conn
                .execute(
                    "DELETE FROM journal_issue WHERE issue_uid = ANY($1)",
                    &[&issues],
                )
                .await?;
        }
        for (column, uids) in [("sections", sections), ("articles", articles)] {
            if !uids.is_empty() {
                deleted += self
                    .conn
                    .execute(
                        &format!(
                            "UPDATE text_publication SET {column} = {column} - $1::text[]
                             WHERE {column} ?| $1::text[]"
                        ),
                        &[&uids],
                    )
                    .await?;
            }
        }
        Ok(deleted)
    }
}

/// The issue that published a text, named on the text's page.
pub struct IssueRef {
    pub title: String,
    pub eli: Option<String>,
}

/// The publication of a text: its blocks by name and the issue that carried it.
pub struct TextPublicationRow {
    pub journal_code: String,
    pub journal_label: Option<String>,
    pub blocks: Value,
    pub issue: Option<IssueRef>,
}

/// A publication read as a notice: what the journal printed, with or without a norm behind it.
pub struct NoticePublicationRow {
    pub text_uid: String,
    pub journal_code: String,
    pub journal_label: Option<String>,
    pub language: String,
    pub languages: Vec<String>,
    pub title: String,
    pub nature: Option<String>,
    pub nor: Option<String>,
    pub date_texte: Option<NaiveDate>,
    pub date_publi: Option<NaiveDate>,
    pub issue: Option<IssueRef>,
    pub ministere: Option<String>,
    pub autorite: Option<String>,
    pub url: Option<String>,
    pub eli: Option<String>,
    pub blocks: Value,
    pub structure: Value,
    pub sections: Value,
    pub articles: Value,
    pub body: Option<String>,
    /// The norm the publication carries, `(slug, title, jurisdiction)`.
    pub norm: Option<(String, String, String)>,
}

/// French, else English, else an authentic language.
const PUBLICATION_LANGUAGE_ORDER: &str =
    "p.language = 'fr' DESC, p.language = 'en' DESC, p.status = 'authentic' DESC, p.language";

impl DecisionRepository<'_> {
    /// The French publication, else the English, else an authentic one.
    #[tracing::instrument(name = "db.text_publication", skip(self), fields(db.system = "postgresql"))]
    pub async fn text_publication(&self, text_uid: &str) -> Result<Option<TextPublicationRow>> {
        Ok(self
            .conn
            .query_opt(
                &format!(
                    "SELECT p.blocks, i.title, i.eli, p.journal_code, j.label
                     FROM text_publication p
                     LEFT JOIN journal_issue i
                            ON (i.issue_uid, i.language) = (p.issue_uid, p.language)
                     LEFT JOIN journal j ON j.code = p.journal_code
                     WHERE p.text_uid = $1
                     ORDER BY {PUBLICATION_LANGUAGE_ORDER}
                     LIMIT 1"
                ),
                &[&text_uid],
            )
            .await?
            .map(|r| TextPublicationRow {
                journal_code: r.get(3),
                journal_label: r.get(4),
                blocks: r.get(0),
                issue: r.get::<_, Option<String>>(1).map(|title| IssueRef {
                    title,
                    eli: r.get(2),
                }),
            }))
    }

    /// A text's publication in the language [`Self::text_publication`] picks, read whole.
    #[tracing::instrument(name = "db.notice_publication", skip(self), fields(db.system = "postgresql"))]
    pub async fn notice_publication(&self, text_uid: &str) -> Result<Option<NoticePublicationRow>> {
        Ok(self
            .conn
            .query_opt(
                &format!(
                    "SELECT p.text_uid, p.journal_code, j.label, p.language,
                            ARRAY(SELECT q.language FROM text_publication q
                                   WHERE q.text_uid = p.text_uid ORDER BY q.language),
                            COALESCE(NULLIF(p.title_full, ''), p.title, ''),
                            COALESCE(p.nature, t.nature), COALESCE(NULLIF(p.nor, ''), t.nor),
                            p.date_texte, p.date_publi,
                            i.title, i.eli, p.ministere, p.autorite, p.url, p.eli,
                            p.blocks, p.articles, p.body, t.slug, t.title, t.jurisdiction,
                            coalesce(p.structure, '[]'::jsonb), p.sections
                     FROM text_publication p
                     LEFT JOIN journal j ON j.code = p.journal_code
                     LEFT JOIN journal_issue i
                            ON (i.issue_uid, i.language) = (p.issue_uid, p.language)
                     LEFT JOIN legal_text t ON t.text_uid = p.text_uid
                     WHERE p.text_uid = $1
                     ORDER BY {PUBLICATION_LANGUAGE_ORDER}
                     LIMIT 1"
                ),
                &[&text_uid],
            )
            .await?
            .map(|r| NoticePublicationRow {
                text_uid: r.get(0),
                journal_code: r.get(1),
                journal_label: r.get(2),
                language: r.get(3),
                languages: r.get(4),
                title: r.get(5),
                nature: r.get(6),
                nor: r.get(7),
                date_texte: r.get(8),
                date_publi: r.get(9),
                issue: r.get::<_, Option<String>>(10).map(|title| IssueRef {
                    title,
                    eli: r.get(11),
                }),
                ministere: r.get(12),
                autorite: r.get(13),
                url: r.get(14),
                eli: r.get(15),
                blocks: r.get(16),
                structure: r.get(22),
                sections: r.get(23),
                articles: r.get(17),
                body: r.get(18),
                norm: r
                    .get::<_, Option<String>>(19)
                    .map(|slug| (slug, r.get(20), r.get(21))),
            }))
    }
}

/// An official journal's publication as a journal search lists it.
#[derive(Debug)]
pub struct JoActRow {
    pub text_uid: String,
    pub journal_code: String,
    pub journal_label: Option<String>,
    pub title: String,
    pub date_publi: Option<NaiveDate>,
    pub nor: Option<String>,
    pub issuer: Option<String>,
    pub url: Option<String>,
    /// The start of its first article.
    pub excerpt: Option<String>,
}

/// The publications a journal search reads: `$1` the text uids left out with those
/// DILA marks protected, `$journals` the journal codes read, every journal without it,
/// `$text` every word of the query in the title, what was printed around the articles,
/// or the body.
fn jo_predicate(journals: Option<usize>, text: Option<usize>) -> String {
    let journals = journals
        .map(|n| {
            format!(
                "paradedb.term_set(terms => ARRAY(
                     SELECT paradedb.term('journal_code', c) FROM unnest(${n}::text[]) c))"
            )
        })
        .unwrap_or_else(|| "paradedb.all()".to_string());
    let words = text
        .map(|n| {
            format!(
                ", paradedb.boolean(should => ARRAY[
                     paradedb.match('heading', ${n}, conjunction_mode => true),
                     paradedb.match('printed', ${n}, conjunction_mode => true),
                     paradedb.match('body', ${n}, conjunction_mode => true)])"
            )
        })
        .unwrap_or_default();
    format!(
        "p.id @@@ paradedb.boolean(
             must => ARRAY[{journals}{words}],
             must_not => ARRAY[
                 paradedb.phrase('heading', ARRAY['acces', 'protege']),
                 paradedb.term_set(terms => ARRAY(
                     SELECT paradedb.term('text_uid', u) FROM unnest($1::text[]) u))])"
    )
}

/// The parameters after the first `fixed`: the journals when some are named, then the
/// words, and the predicate numbering them.
fn jo_params<'a>(
    fixed: Vec<&'a (dyn ToSql + Sync)>,
    journals: &'a Option<&'a [String]>,
    text: &'a Option<&'a str>,
) -> (Vec<&'a (dyn ToSql + Sync)>, String) {
    let mut params = fixed;
    let journals_at = journals.as_ref().map(|j| {
        params.push(j);
        params.len()
    });
    let text_at = text.as_ref().map(|q| {
        params.push(q);
        params.len()
    });
    (params, jo_predicate(journals_at, text_at))
}

impl DecisionRepository<'_> {
    /// A page of official journals' publications, newest first.
    #[tracing::instrument(name = "db.jo_acts", skip(self, hidden), fields(db.system = "postgresql"))]
    pub async fn jo_acts(
        &self,
        text: Option<&str>,
        journals: Option<&[String]>,
        hidden: &[String],
        limit: i64,
        offset: i64,
    ) -> Result<Vec<JoActRow>> {
        let fixed: Vec<&(dyn ToSql + Sync)> = vec![&hidden, &limit, &offset];
        let (params, predicate) = jo_params(fixed, &journals, &text);
        let sql = format!(
            "SELECT p.text_uid, p.journal_code, j.label,
                    COALESCE(NULLIF(p.title_full, ''), p.title, ''), p.date_publi,
                    NULLIF(p.nor, ''), COALESCE(p.ministere, p.autorite), p.url,
                    left((SELECT a.value->>'text' FROM jsonb_each(p.articles) a
                          ORDER BY a.key LIMIT 1), 400)
             FROM text_publication p
             LEFT JOIN journal j ON j.code = p.journal_code
             WHERE {predicate}
             ORDER BY p.date_publi DESC NULLS LAST, p.id DESC
             LIMIT $2 OFFSET $3"
        );
        Ok(self
            .conn
            .query(&sql, &params)
            .await?
            .iter()
            .map(|r| JoActRow {
                text_uid: r.get(0),
                journal_code: r.get(1),
                journal_label: r.get(2),
                title: r.get(3),
                date_publi: r.get(4),
                nor: r.get(5),
                issuer: r.get(6),
                url: r.get(7),
                excerpt: r.get(8),
            })
            .collect())
    }

    /// How many official journals' publications the search reads.
    #[tracing::instrument(name = "db.jo_total", skip(self, hidden), fields(db.system = "postgresql"))]
    pub async fn jo_total(
        &self,
        text: Option<&str>,
        journals: Option<&[String]>,
        hidden: &[String],
    ) -> Result<i64> {
        let fixed: Vec<&(dyn ToSql + Sync)> = vec![&hidden];
        let (params, predicate) = jo_params(fixed, &journals, &text);
        let sql = format!("SELECT count(*) FROM text_publication p WHERE {predicate}");
        Ok(self.conn.query_one(&sql, &params).await?.get(0))
    }

    /// The codes of the journals the referential names.
    #[tracing::instrument(name = "db.journal_codes", skip(self), fields(db.system = "postgresql"))]
    pub async fn journal_codes(&self) -> Result<Vec<String>> {
        Ok(self
            .conn
            .query("SELECT code FROM journal ORDER BY code", &[])
            .await?
            .iter()
            .map(|r| r.get(0))
            .collect())
    }

    /// `(text_uid, title)` of the publications of `journals` DILA does not mark protected
    /// whose title holds one of `words`.
    #[tracing::instrument(name = "db.jo_unmarked_titles", skip(self), fields(db.system = "postgresql"))]
    pub async fn jo_unmarked_titles_with_any(
        &self,
        words: &str,
        journals: &[String],
    ) -> Result<Vec<(String, String)>> {
        Ok(self
            .conn
            .query(
                "SELECT p.text_uid, COALESCE(NULLIF(p.title_full, ''), p.title, '')
                 FROM text_publication p
                 WHERE p.id @@@ paradedb.boolean(
                     must => ARRAY[
                         paradedb.term_set(terms => ARRAY(
                             SELECT paradedb.term('journal_code', c) FROM unnest($2::text[]) c)),
                         paradedb.match('heading', $1)],
                     must_not => ARRAY[paradedb.phrase('heading', ARRAY['acces', 'protege'])])",
                &[&words, &journals],
            )
            .await?
            .iter()
            .map(|r| (r.get(0), r.get(1)))
            .collect())
    }
}
