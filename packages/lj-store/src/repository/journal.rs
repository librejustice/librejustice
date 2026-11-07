//! The JO issues and the publication of each JORF text. A text's row is merged
//! member by member, and a member that reads the same writes nothing.

use std::collections::HashSet;

use chrono::NaiveDate;
use serde_json::Value;

use super::DecisionRepository;
use crate::error::Result;

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
    pub async fn upsert_journal_issues(&self, rows: &[JournalIssueRow]) -> Result<u64> {
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
                     (issue_uid, journal_code, num, date_publi, eli, title, toc)
                 SELECT * FROM unnest($1::text[], $2::text[], $3::text[], $4::date[],
                                      $5::text[], $6::text[], $7::jsonb[])
                 ON CONFLICT (issue_uid) DO UPDATE SET
                     journal_code = EXCLUDED.journal_code, num = EXCLUDED.num,
                     date_publi = EXCLUDED.date_publi, eli = EXCLUDED.eli,
                     title = EXCLUDED.title, toc = EXCLUDED.toc
                 WHERE (i.journal_code, i.num, i.date_publi, i.eli, i.title, i.toc)
                       IS DISTINCT FROM
                       (EXCLUDED.journal_code, EXCLUDED.num, EXCLUDED.date_publi,
                        EXCLUDED.eli, EXCLUDED.title, EXCLUDED.toc)",
                &[&uids, &journals, &nums, &dates, &elis, &titles, &tocs],
            )
            .await?)
    }

    /// Each text once: the issues these placements name are written.
    #[tracing::instrument(name = "db.place_publications", skip_all, fields(db.system = "postgresql"))]
    pub async fn place_publications(&self, rows: &[IssuePlacement]) -> Result<u64> {
        let texts: Vec<&str> = rows.iter().map(|r| r.text_uid.as_str()).collect();
        let issues: Vec<&str> = rows.iter().map(|r| r.issue_uid.as_str()).collect();
        let ranks: Vec<i32> = rows.iter().map(|r| r.issue_rank).collect();
        Ok(self
            .conn
            .execute(
                "INSERT INTO text_publication AS t (text_uid, issue_uid, issue_rank)
                 SELECT * FROM unnest($1::text[], $2::text[], $3::int[])
                 ON CONFLICT (text_uid) DO UPDATE SET
                     issue_uid = EXCLUDED.issue_uid, issue_rank = EXCLUDED.issue_rank
                 WHERE (t.issue_uid, t.issue_rank)
                       IS DISTINCT FROM (EXCLUDED.issue_uid, EXCLUDED.issue_rank)",
                &[&texts, &issues, &ranks],
            )
            .await?)
    }

    /// Each text once.
    #[tracing::instrument(name = "db.upsert_publication_facts", skip_all, fields(db.system = "postgresql"))]
    pub async fn upsert_publication_facts(&self, rows: &[PublicationFacts]) -> Result<u64> {
        let rows = Value::Array(rows.to_vec());
        Ok(self
            .conn
            .execute(
                "INSERT INTO text_publication AS t
                     (text_uid, num_parution, num_sequence, origine_publi, date_publi, eli,
                      page_deb, page_fin, ministere, autorite, title, title_full, keywords,
                      enterprise, blocks)
                 SELECT r.text_uid, r.num_parution, r.num_sequence, r.origine_publi,
                        r.date_publi, r.eli, r.page_deb, r.page_fin, r.ministere, r.autorite,
                        r.title, r.title_full, r.keywords, r.enterprise, r.blocks
                 FROM jsonb_populate_recordset(NULL::text_publication, $1::jsonb) r
                 ON CONFLICT (text_uid) DO UPDATE SET
                     num_parution = EXCLUDED.num_parution, num_sequence = EXCLUDED.num_sequence,
                     origine_publi = EXCLUDED.origine_publi, date_publi = EXCLUDED.date_publi,
                     eli = EXCLUDED.eli, page_deb = EXCLUDED.page_deb,
                     page_fin = EXCLUDED.page_fin, ministere = EXCLUDED.ministere,
                     autorite = EXCLUDED.autorite, title = EXCLUDED.title,
                     title_full = EXCLUDED.title_full, keywords = EXCLUDED.keywords,
                     enterprise = EXCLUDED.enterprise, blocks = EXCLUDED.blocks
                 WHERE (t.num_parution, t.num_sequence, t.origine_publi, t.date_publi, t.eli,
                        t.page_deb, t.page_fin, t.ministere, t.autorite, t.title,
                        t.title_full, t.keywords, t.enterprise, t.blocks)
                       IS DISTINCT FROM
                       (EXCLUDED.num_parution, EXCLUDED.num_sequence, EXCLUDED.origine_publi,
                        EXCLUDED.date_publi, EXCLUDED.eli, EXCLUDED.page_deb,
                        EXCLUDED.page_fin, EXCLUDED.ministere, EXCLUDED.autorite,
                        EXCLUDED.title, EXCLUDED.title_full, EXCLUDED.keywords,
                        EXCLUDED.enterprise, EXCLUDED.blocks)",
                &[&rows],
            )
            .await?)
    }

    /// Each text once.
    #[tracing::instrument(name = "db.upsert_publications", skip_all, fields(db.system = "postgresql"))]
    pub async fn upsert_publications(&self, rows: &[PublicationRow]) -> Result<u64> {
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
                     (text_uid, journal_code, nature, nor, title, date_texte, date_publi, url, body)
                 SELECT * FROM unnest($1::text[], $2::text[], $3::text[], $4::text[], $5::text[],
                                      $6::date[], $7::date[], $8::text[], $9::text[])
                 ON CONFLICT (text_uid) DO UPDATE SET
                     journal_code = EXCLUDED.journal_code, nature = EXCLUDED.nature,
                     nor = EXCLUDED.nor, title = EXCLUDED.title,
                     date_texte = EXCLUDED.date_texte, date_publi = EXCLUDED.date_publi,
                     url = EXCLUDED.url, body = EXCLUDED.body
                 WHERE (t.journal_code, t.nature, t.nor, t.title, t.date_texte, t.date_publi,
                        t.url, t.body)
                       IS DISTINCT FROM
                       (EXCLUDED.journal_code, EXCLUDED.nature, EXCLUDED.nor, EXCLUDED.title,
                        EXCLUDED.date_texte, EXCLUDED.date_publi, EXCLUDED.url, EXCLUDED.body)",
                &[
                    &uids, &journals, &natures, &nors, &titles, &signed, &published, &urls, &bodies,
                ],
            )
            .await?)
    }

    /// Each text once, with its items in published order.
    #[tracing::instrument(name = "db.upsert_publication_structures", skip_all, fields(db.system = "postgresql"))]
    pub async fn upsert_publication_structures(&self, rows: &[(String, Value)]) -> Result<u64> {
        let (texts, structures): (Vec<&str>, Vec<&Value>) =
            rows.iter().map(|(t, s)| (t.as_str(), s)).unzip();
        Ok(self
            .conn
            .execute(
                "INSERT INTO text_publication AS t (text_uid, structure)
                 SELECT * FROM unnest($1::text[], $2::jsonb[])
                 ON CONFLICT (text_uid) DO UPDATE SET structure = EXCLUDED.structure
                 WHERE t.structure IS DISTINCT FROM EXCLUDED.structure",
                &[&texts, &structures],
            )
            .await?)
    }

    /// Each text once, with an object of its sections keyed by `JORFSCTA`.
    pub async fn merge_publication_sections(&self, rows: &[(String, Value)]) -> Result<u64> {
        self.merge_publication_map("sections", rows).await
    }

    /// Each text once, with an object of its articles keyed by `JORFARTI`. An
    /// entry a treaty record holds keeps its pointer.
    #[tracing::instrument(name = "db.merge_publication_articles", skip_all, fields(db.system = "postgresql"))]
    pub async fn merge_publication_articles(&self, rows: &[(String, Value)]) -> Result<u64> {
        let (texts, entries): (Vec<&str>, Vec<&Value>) =
            rows.iter().map(|(t, e)| (t.as_str(), e)).unzip();
        Ok(self
            .conn
            .execute(
                "INSERT INTO text_publication AS t (text_uid, articles)
                 SELECT * FROM unnest($1::text[], $2::jsonb[])
                 ON CONFLICT (text_uid) DO UPDATE SET articles = t.articles || (
                     SELECT coalesce(jsonb_object_agg(e.key, e.value), '{}'::jsonb)
                     FROM jsonb_each(EXCLUDED.articles) e
                     WHERE NOT coalesce((t.articles -> e.key) ? 'held', false))
                 WHERE EXISTS (SELECT 1 FROM jsonb_each(EXCLUDED.articles) e
                               WHERE t.articles -> e.key IS DISTINCT FROM e.value
                                 AND NOT coalesce((t.articles -> e.key) ? 'held', false))",
                &[&texts, &entries],
            )
            .await?)
    }

    /// The decree publishing a treaty record with neither body nor article, when
    /// it is the record's only one.
    #[tracing::instrument(name = "db.treaty_cut_candidates", skip_all, fields(db.system = "postgresql"))]
    pub async fn treaty_cut_candidates(
        &self,
        vehicles: Option<&[String]>,
    ) -> Result<Vec<TreatyCutCandidate>> {
        Ok(self
            .conn
            .query(
                "SELECT l.owner_text_uid, p.text_uid, p.date_publi,
                        coalesce(p.structure, '[]'::jsonb), coalesce(p.sections, '{}'::jsonb),
                        p.articles
                 FROM legal_link l
                 JOIN text_publication p ON p.text_uid = l.target_text_uid
                 JOIN legal_text t ON t.text_uid = l.owner_text_uid
                 WHERE l.verb = 'publie' AND l.direction = 'incoming'
                   AND l.owner_text_uid LIKE 'INTL/%'
                   AND ($1::text[] IS NULL OR p.text_uid = ANY($1))
                   AND p.articles IS NOT NULL AND p.articles <> '{}'::jsonb
                   AND coalesce(t.body, '') = ''
                   AND NOT EXISTS (SELECT 1 FROM legal_article a
                                   WHERE a.text_uid = l.owner_text_uid)
                   AND NOT EXISTS (SELECT 1 FROM legal_link o
                                   WHERE o.owner_text_uid = l.owner_text_uid
                                     AND o.verb = 'publie' AND o.direction = 'incoming'
                                     AND o.target_text_uid <> l.target_text_uid)
                 ORDER BY l.owner_text_uid",
                &[&vehicles],
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

    /// The vehicle's copy of each entry becomes a pointer to its holder.
    #[tracing::instrument(name = "db.hold_publication_articles", skip(self, ids), fields(db.system = "postgresql"))]
    pub async fn hold_publication_articles(
        &self,
        vehicle_uid: &str,
        ids: &[&str],
        instrument_uid: &str,
    ) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "UPDATE text_publication SET articles = articles || (
                     SELECT jsonb_object_agg(id, jsonb_build_object('held', $3::text))
                     FROM unnest($2::text[]) id)
                 WHERE text_uid = $1",
                &[&vehicle_uid, &ids, &instrument_uid],
            )
            .await?)
    }

    /// The `JORFARTI` among `uids` that `legal_article` holds as JORF rows.
    #[tracing::instrument(name = "db.jorf_articles_held", skip_all, fields(db.system = "postgresql", uids = uids.len()))]
    pub async fn jorf_articles_held(&self, uids: &[&str]) -> Result<HashSet<String>> {
        Ok(self
            .conn
            .query(
                "SELECT DISTINCT source_uid FROM legal_article
                 WHERE source = 'jorf' AND source_uid = ANY($1)",
                &[&uids],
            )
            .await?
            .iter()
            .map(|r| r.get(0))
            .collect())
    }

    #[tracing::instrument(name = "db.merge_publication_map", skip(self, rows), fields(db.system = "postgresql"))]
    async fn merge_publication_map(&self, column: &str, rows: &[(String, Value)]) -> Result<u64> {
        let (texts, entries): (Vec<&str>, Vec<&Value>) =
            rows.iter().map(|(t, e)| (t.as_str(), e)).unzip();
        Ok(self
            .conn
            .execute(
                &format!(
                    "INSERT INTO text_publication AS t (text_uid, {column})
                     SELECT * FROM unnest($1::text[], $2::jsonb[])
                     ON CONFLICT (text_uid) DO UPDATE SET {column} = t.{column} || EXCLUDED.{column}
                     WHERE EXISTS (SELECT 1 FROM jsonb_each(EXCLUDED.{column}) e
                                   WHERE t.{column} -> e.key IS DISTINCT FROM e.value)"
                ),
                &[&texts, &entries],
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
    pub blocks: Value,
    pub issue: Option<IssueRef>,
}

impl DecisionRepository<'_> {
    #[tracing::instrument(name = "db.text_publication", skip(self), fields(db.system = "postgresql"))]
    pub async fn text_publication(&self, text_uid: &str) -> Result<Option<TextPublicationRow>> {
        Ok(self
            .conn
            .query_opt(
                "SELECT p.blocks, i.title, i.eli
                 FROM text_publication p
                 LEFT JOIN journal_issue i ON i.issue_uid = p.issue_uid
                 WHERE p.text_uid = $1",
                &[&text_uid],
            )
            .await?
            .map(|r| TextPublicationRow {
                blocks: r.get(0),
                issue: r.get::<_, Option<String>>(1).map(|title| IssueRef {
                    title,
                    eli: r.get(2),
                }),
            }))
    }
}
