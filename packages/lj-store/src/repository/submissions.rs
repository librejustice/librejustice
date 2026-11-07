//! `decision_submissions`, the files accounts submit, until their review.

use chrono::{DateTime, Utc};

use super::DecisionRepository;
use crate::error::Result;

pub struct PendingSubmission {
    pub id: i64,
    pub account: String,
    pub bytes: i32,
    pub created_at: DateTime<Utc>,
}

impl DecisionRepository<'_> {
    #[tracing::instrument(name = "db.insert_submission", skip(self, file), fields(db.system = "postgresql", bytes = file.len()))]
    pub async fn insert_submission(&self, account: &str, file: &[u8]) -> Result<i64> {
        let row = self
            .conn
            .query_one(
                "INSERT INTO decision_submissions (account, file) VALUES ($1, $2) RETURNING id",
                &[&account, &file],
            )
            .await?;
        Ok(row.get(0))
    }

    #[tracing::instrument(name = "db.pending_submissions", skip(self), fields(db.system = "postgresql"))]
    pub async fn pending_submissions(&self) -> Result<Vec<PendingSubmission>> {
        let rows = self
            .conn
            .query(
                "SELECT id, account, length(file), created_at FROM decision_submissions ORDER BY id",
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| PendingSubmission {
                id: r.get(0),
                account: r.get(1),
                bytes: r.get(2),
                created_at: r.get(3),
            })
            .collect())
    }

    /// The account and the file of a submission.
    #[tracing::instrument(name = "db.submission", skip(self), fields(db.system = "postgresql"))]
    pub async fn submission(&self, id: i64) -> Result<Option<(String, Vec<u8>)>> {
        let row = self
            .conn
            .query_opt(
                "SELECT account, file FROM decision_submissions WHERE id = $1",
                &[&id],
            )
            .await?;
        Ok(row.map(|r| (r.get(0), r.get(1))))
    }

    #[tracing::instrument(name = "db.delete_submission", skip(self), fields(db.system = "postgresql"))]
    pub async fn delete_submission(&self, id: i64) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM decision_submissions WHERE id = $1", &[&id])
            .await?;
        Ok(n == 1)
    }
}
