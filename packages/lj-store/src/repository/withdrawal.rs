//! `decision_withdrawal`, the decisions withdrawn by hand, held at their address.

use chrono::NaiveDate;

use super::types::ExtractedFields;
use super::DecisionRepository;
use crate::error::{Result, StoreError};

impl DecisionRepository<'_> {
    /// Same court, same date and a docket in common with a withdrawal.
    #[tracing::instrument(name = "db.is_withdrawn", skip_all, fields(db.system = "postgresql"))]
    pub async fn is_withdrawn(&self, e: &ExtractedFields) -> Result<bool> {
        let (Some(court), Some(date)) = (e.jurisdiction.as_ref(), e.date_lecture) else {
            return Ok(false);
        };
        let courts: Vec<String> = self
            .conn
            .query(
                "SELECT jurisdiction_code FROM decision_withdrawal \
                 WHERE date_lecture = $1 AND docket_keys && lj_docket_keys($2)",
                &[&date, &e.docket_numbers],
            )
            .await?
            .into_iter()
            .map(|r| r.get(0))
            .collect();
        if courts.is_empty() {
            return Ok(false);
        }
        Ok(courts.contains(&self.court_code(court).await?))
    }

    /// Holds the decision's address, then tombstones its active provenances,
    /// which empties it. Returns the provenances tombstoned.
    #[tracing::instrument(name = "db.withdraw", skip(self, reason), fields(db.system = "postgresql"))]
    pub async fn withdraw(&self, public_id: &str, reason: &str) -> Result<Vec<String>> {
        let row = self
            .conn
            .query_opt(
                "SELECT id, jurisdiction_code, date_lecture, lj_docket_keys(docket_numbers) \
                 FROM decisions WHERE public_id = $1",
                &[&public_id],
            )
            .await?
            .ok_or_else(|| StoreError::Invalid(format!("no decision at {public_id}")))?;
        let id: i64 = row.get(0);
        let (Some(court), Some(date), Some(keys)) = (
            row.get::<_, Option<String>>(1),
            row.get::<_, Option<NaiveDate>>(2),
            row.get::<_, Option<Vec<String>>>(3),
        ) else {
            return Err(StoreError::Invalid(format!(
                "{public_id}: no court, date or docket to hold"
            )));
        };
        self.conn
            .execute(
                "INSERT INTO decision_withdrawal \
                   (public_id, jurisdiction_code, date_lecture, docket_keys, reason) \
                 VALUES ($1, $2, $3, $4, $5) ON CONFLICT (public_id) DO NOTHING",
                &[&public_id, &court, &date, &keys, &reason],
            )
            .await?;
        let uids: Vec<String> = self
            .conn
            .query(
                "SELECT source_uid FROM decision_sources \
                 WHERE decision_id = $1 AND deleted_at IS NULL ORDER BY source_uid",
                &[&id],
            )
            .await?
            .into_iter()
            .map(|r| r.get(0))
            .collect();
        for uid in &uids {
            self.delete(uid).await?;
        }
        Ok(uids)
    }

    /// The decision comes back at the next read of its source.
    #[tracing::instrument(name = "db.lift_withdrawal", skip(self), fields(db.system = "postgresql"))]
    pub async fn lift_withdrawal(&self, public_id: &str) -> Result<bool> {
        let n = self
            .conn
            .execute(
                "DELETE FROM decision_withdrawal WHERE public_id = $1",
                &[&public_id],
            )
            .await?;
        Ok(n == 1)
    }
}
