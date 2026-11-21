//! `raa_document`, the prefectures' recueils, from their listing to their text.

use chrono::NaiveDate;

use super::DecisionRepository;
use crate::error::Result;

const RAA_LOCK_KEY: i64 = 0x6c6a_5f72_6161_5f64; // « lj_raa_d »

/// A transient failure gives up on the recueil after this many.
const MAX_FAILURES: i32 = 5;

pub struct RaaListedRow<'a> {
    pub id: i64,
    pub url: &'a str,
    pub publisher: &'a str,
    pub dept: Option<&'a str>,
    pub published: Option<NaiveDate>,
    pub title: &'a str,
}

pub struct RaaQueued {
    pub id: i64,
    pub url: String,
}

pub struct RaaScanned {
    pub id: i64,
    pub url: String,
    pub pdf_checksum: i64,
    /// One entry per page, `None` while the page awaits its OCR.
    pub pages: Vec<Option<String>>,
    pub ocr_pages: Vec<i32>,
}

pub struct RaaRead<'a> {
    pub id: i64,
    pub pdf_checksum: i64,
    pub pages: &'a [Option<String>],
    pub ocr_pages: &'a [i32],
    /// Set once every page is read, which makes the recueil `read`.
    pub body: Option<&'a str>,
}

/// A recueil as its notice page reads it.
pub struct RaaNotice {
    pub id: i64,
    pub url: String,
    pub publisher: String,
    pub published: Option<NaiveDate>,
    pub title: String,
    pub body: Option<String>,
}

/// A recueil as a journal search lists it.
#[derive(Debug)]
pub struct RaaHitRow {
    pub id: i64,
    pub url: String,
    pub publisher: String,
    pub dept: Option<String>,
    pub published: Option<NaiveDate>,
    pub title: String,
    /// The passage of the body the question matched.
    pub excerpt: Option<String>,
}

/// The recueils a journal search reads: none taken down, those of the départements `$1`
/// (all when empty), and every word of the question `$n` in the title or the body.
fn raa_predicate(text: Option<usize>) -> String {
    let mut predicate =
        "r.status <> 'gone' AND (cardinality($1::text[]) = 0 OR r.dept = ANY($1))".to_string();
    if let Some(n) = text {
        predicate.push_str(&format!(
            " AND r.id @@@ paradedb.boolean(should => ARRAY[
                 paradedb.match('title', ${n}, conjunction_mode => true),
                 paradedb.match('body', ${n}, conjunction_mode => true)])"
        ));
    }
    predicate
}

impl DecisionRepository<'_> {
    /// A page of the recueils a journal search reads, newest first.
    #[tracing::instrument(name = "db.raa_search", skip(self, depts), fields(db.system = "postgresql"))]
    pub async fn raa_search(
        &self,
        text: Option<&str>,
        depts: &[String],
        limit: i64,
        offset: i64,
    ) -> Result<Vec<RaaHitRow>> {
        let excerpt = match text {
            Some(_) => {
                "paradedb.snippet(r.body, start_tag => '', end_tag => '', max_num_chars => 300)"
            }
            None => "NULL::text",
        };
        let sql = format!(
            "SELECT r.id, r.url, r.publisher, r.dept, r.published, r.title, {excerpt}
             FROM raa_document r
             WHERE {}
             ORDER BY r.published DESC NULLS LAST, r.id DESC
             LIMIT $2 OFFSET $3",
            raa_predicate(text.map(|_| 4))
        );
        let rows = match text {
            Some(q) => {
                self.conn
                    .query(&sql, &[&depts, &limit, &offset, &q])
                    .await?
            }
            None => self.conn.query(&sql, &[&depts, &limit, &offset]).await?,
        };
        Ok(rows
            .iter()
            .map(|r| RaaHitRow {
                id: r.get(0),
                url: r.get(1),
                publisher: r.get(2),
                dept: r.get(3),
                published: r.get(4),
                title: r.get(5),
                excerpt: r.get(6),
            })
            .collect())
    }

    /// How many recueils the search reads.
    #[tracing::instrument(name = "db.raa_total", skip(self, depts), fields(db.system = "postgresql"))]
    pub async fn raa_total(&self, text: Option<&str>, depts: &[String]) -> Result<i64> {
        let sql = format!(
            "SELECT count(*) FROM raa_document r WHERE {}",
            raa_predicate(text.map(|_| 2))
        );
        let row = match text {
            Some(q) => self.conn.query_one(&sql, &[&depts, &q]).await?,
            None => self.conn.query_one(&sql, &[&depts]).await?,
        };
        Ok(row.get(0))
    }

    /// The recueils the search reads per département, whatever départements it chose.
    #[tracing::instrument(name = "db.raa_depts", skip(self), fields(db.system = "postgresql"))]
    pub async fn raa_depts(&self, text: Option<&str>) -> Result<Vec<(String, i64)>> {
        let sql = format!(
            "SELECT r.dept, count(*) FROM raa_document r
             WHERE r.dept IS NOT NULL AND {}
             GROUP BY r.dept",
            raa_predicate(text.map(|_| 2))
        );
        let all: &[String] = &[];
        let rows = match text {
            Some(q) => self.conn.query(&sql, &[&all, &q]).await?,
            None => self.conn.query(&sql, &[&all]).await?,
        };
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Session lock held by the one run that fetches, released with its connection.
    #[tracing::instrument(name = "db.raa_lock", skip(self), fields(db.system = "postgresql"))]
    pub async fn raa_lock(&self) -> Result<bool> {
        let row = self
            .conn
            .query_one("SELECT pg_try_advisory_lock($1)", &[&RAA_LOCK_KEY])
            .await?;
        Ok(row.get(0))
    }

    /// Recueils not seen before, returning how many.
    #[tracing::instrument(name = "db.raa_list", skip(self, rows), fields(db.system = "postgresql", rows = rows.len()))]
    pub async fn raa_list(&self, rows: &[RaaListedRow<'_>]) -> Result<u64> {
        let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
        let urls: Vec<&str> = rows.iter().map(|r| r.url).collect();
        let publishers: Vec<&str> = rows.iter().map(|r| r.publisher).collect();
        let depts: Vec<Option<&str>> = rows.iter().map(|r| r.dept).collect();
        let dates: Vec<Option<NaiveDate>> = rows.iter().map(|r| r.published).collect();
        let titles: Vec<&str> = rows.iter().map(|r| r.title).collect();
        Ok(self
            .conn
            .execute(
                "INSERT INTO raa_document (id, url, publisher, dept, published, title) \
                 SELECT * FROM unnest($1::bigint[], $2::text[], $3::text[], $4::text[], \
                                      $5::date[], $6::text[]) \
                 ON CONFLICT (url) DO NOTHING",
                &[&ids, &urls, &publishers, &depts, &dates, &titles],
            )
            .await?)
    }

    /// Next recueils to fetch, newest first; one visited in the last six hours waits.
    #[tracing::instrument(name = "db.raa_queue", skip(self), fields(db.system = "postgresql"))]
    pub async fn raa_queue(&self, limit: i64) -> Result<Vec<RaaQueued>> {
        let rows = self
            .conn
            .query(
                "SELECT id, url FROM raa_document \
                 WHERE status = 'listed' AND failures < $1 \
                   AND (visited_at IS NULL OR visited_at < now() - interval '6 hours') \
                 ORDER BY published DESC NULLS LAST, id \
                 LIMIT $2",
                &[&MAX_FAILURES, &limit],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| RaaQueued {
                id: r.get(0),
                url: r.get(1),
            })
            .collect())
    }

    /// Recueils awaiting their OCR, the least recently visited first.
    #[tracing::instrument(name = "db.raa_ocr_queue", skip(self), fields(db.system = "postgresql"))]
    pub async fn raa_ocr_queue(&self, limit: i64) -> Result<Vec<RaaScanned>> {
        let rows = self
            .conn
            .query(
                "SELECT id, url, pdf_checksum, pages, ocr_pages FROM raa_document \
                 WHERE status = 'ocr' AND failures < $1 \
                 ORDER BY visited_at, id \
                 LIMIT $2",
                &[&MAX_FAILURES, &limit],
            )
            .await?;
        rows.into_iter()
            .map(|r| {
                Ok(RaaScanned {
                    id: r.try_get(0)?,
                    url: r.try_get(1)?,
                    pdf_checksum: r.try_get(2)?,
                    pages: r.try_get(3)?,
                    ocr_pages: r.try_get(4)?,
                })
            })
            .collect()
    }

    /// Recueils with a page read by the OCR, by id from `after` on.
    #[tracing::instrument(name = "db.raa_ocr_read", skip(self), fields(db.system = "postgresql"))]
    pub async fn raa_ocr_read(&self, after: i64, limit: i64) -> Result<Vec<RaaScanned>> {
        let rows = self
            .conn
            .query(
                "SELECT id, url, pdf_checksum, pages, ocr_pages FROM raa_document \
                 WHERE id > $1 AND cardinality(ocr_pages) > 0 \
                 ORDER BY id \
                 LIMIT $2",
                &[&after, &limit],
            )
            .await?;
        rows.into_iter()
            .map(|r| {
                Ok(RaaScanned {
                    id: r.try_get(0)?,
                    url: r.try_get(1)?,
                    pdf_checksum: r.try_get(2)?,
                    pages: r.try_get(3)?,
                    ocr_pages: r.try_get(4)?,
                })
            })
            .collect()
    }

    #[tracing::instrument(name = "db.raa_write", skip(self, r), fields(db.system = "postgresql", id = r.id))]
    pub async fn raa_write(&self, r: &RaaRead<'_>) -> Result<()> {
        let status = if r.body.is_some() { "read" } else { "ocr" };
        self.conn
            .execute(
                "UPDATE raa_document SET status = $2, pdf_checksum = $3, pages = $4, \
                   ocr_pages = $5, body = $6, visited_at = now(), failures = 0 \
                 WHERE id = $1",
                &[
                    &r.id,
                    &status,
                    &r.pdf_checksum,
                    &r.pages,
                    &r.ocr_pages,
                    &r.body,
                ],
            )
            .await?;
        Ok(())
    }

    /// `gone` or `unreadable`: left out of the queue for good.
    #[tracing::instrument(name = "db.raa_close", skip(self), fields(db.system = "postgresql"))]
    pub async fn raa_close(&self, id: i64, status: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE raa_document SET status = $2, visited_at = now() WHERE id = $1",
                &[&id, &status],
            )
            .await?;
        Ok(())
    }

    #[tracing::instrument(name = "db.raa_failed", skip(self), fields(db.system = "postgresql"))]
    pub async fn raa_failed(&self, id: i64) -> Result<()> {
        self.conn
            .execute(
                "UPDATE raa_document SET failures = failures + 1, visited_at = now() WHERE id = $1",
                &[&id],
            )
            .await?;
        Ok(())
    }

    /// A recueil its prefecture still serves.
    #[tracing::instrument(name = "db.raa_notice", skip(self), fields(db.system = "postgresql"))]
    pub async fn raa_notice(&self, id: i64) -> Result<Option<RaaNotice>> {
        let row = self
            .conn
            .query_opt(
                "SELECT id, url, publisher, published, title, body FROM raa_document \
                 WHERE id = $1 AND status <> 'gone'",
                &[&id],
            )
            .await?;
        Ok(row.map(|r| RaaNotice {
            id: r.get(0),
            url: r.get(1),
            publisher: r.get(2),
            published: r.get(3),
            title: r.get(4),
            body: r.get(5),
        }))
    }

    /// Recueils per status.
    #[tracing::instrument(name = "db.raa_counts", skip(self), fields(db.system = "postgresql"))]
    pub async fn raa_counts(&self) -> Result<Vec<(String, i64)>> {
        let rows = self
            .conn
            .query(
                "SELECT status, count(*) FROM raa_document GROUP BY status ORDER BY status",
                &[],
            )
            .await?;
        Ok(rows.into_iter().map(|r| (r.get(0), r.get(1))).collect())
    }
}
