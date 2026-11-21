//! The provenances whose source names a person until a date, withdrawn at that date.

use chrono::NaiveDate;
use serde_json::Value;

use super::DecisionRepository;
use crate::error::Result;

impl DecisionRepository<'_> {
    /// The active provenances of `sources` named until `until` at the latest.
    #[tracing::instrument(name = "db.named_until_due", skip(self), fields(db.system = "postgresql"))]
    pub async fn named_until_due(
        &self,
        sources: &[&str],
        until: NaiveDate,
    ) -> Result<Vec<(String, Value)>> {
        let rows = self
            .conn
            .query(
                "SELECT source_uid, source_fields FROM decision_sources \
                 WHERE source = ANY($1) AND deleted_at IS NULL \
                   AND source_fields->>'named_until' <= $2 \
                 ORDER BY source_uid",
                &[&sources, &until.to_string()],
            )
            .await?;
        Ok(rows.into_iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Tombstones the provenance, which empties its decision, and holds its checksum.
    #[tracing::instrument(name = "db.expire_provenance", skip(self), fields(db.system = "postgresql"))]
    pub async fn expire_provenance(&self, source_uid: &str) -> Result<()> {
        self.conn.batch_execute("BEGIN").await?;
        let res = async {
            self.delete_locked(source_uid).await?;
            self.conn
                .execute(
                    "UPDATE decision_sources SET expired_checksum = content_checksum \
                     WHERE source_uid = $1",
                    &[&source_uid],
                )
                .await?;
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

    /// The provenance's version at this checksum has expired.
    #[tracing::instrument(name = "db.is_expired", skip(self), fields(db.system = "postgresql"))]
    pub async fn is_expired(&self, source_uid: &str, content_checksum: &str) -> Result<bool> {
        let row = self
            .conn
            .query_opt(
                "SELECT 1 FROM decision_sources \
                 WHERE source_uid = $1 AND expired_checksum = $2",
                &[&source_uid, &content_checksum],
            )
            .await?;
        Ok(row.is_some())
    }
}
