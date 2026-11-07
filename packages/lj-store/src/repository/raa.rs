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
    pub pdf_checksum: Option<i64>,
    /// One entry per page, `None` while the page awaits its OCR.
    pub pages: Option<Vec<Option<String>>>,
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

impl DecisionRepository<'_> {
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

    /// Next recueils to read, newest first: the unread, and with `with_ocr` those
    /// awaiting an OCR. A recueil visited in the last six hours waits.
    #[tracing::instrument(name = "db.raa_queue", skip(self), fields(db.system = "postgresql"))]
    pub async fn raa_queue(&self, with_ocr: bool, limit: i64) -> Result<Vec<RaaQueued>> {
        let rows = self
            .conn
            .query(
                "SELECT id, url, pdf_checksum, pages, ocr_pages FROM raa_document \
                 WHERE (status = 'listed' OR ($1 AND status = 'ocr')) \
                   AND failures < $2 \
                   AND (visited_at IS NULL OR visited_at < now() - interval '6 hours') \
                 ORDER BY published DESC NULLS LAST, id \
                 LIMIT $3",
                &[&with_ocr, &MAX_FAILURES, &limit],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| RaaQueued {
                id: r.get(0),
                url: r.get(1),
                pdf_checksum: r.get(2),
                pages: r.get(3),
                ocr_pages: r.get::<_, Option<Vec<i32>>>(4).unwrap_or_default(),
            })
            .collect())
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
