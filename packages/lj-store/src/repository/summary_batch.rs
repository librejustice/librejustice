//! `summary_batch`, the summary batches a provider still holds.

use super::DecisionRepository;
use crate::error::Result;

pub struct SummaryBatchRow {
    pub id: String,
    pub engine: i16,
    pub effort: String,
    pub decision_ids: Vec<i64>,
}

impl DecisionRepository<'_> {
    #[tracing::instrument(name = "db.open_summary_batches", skip(self), fields(db.system = "postgresql"))]
    pub async fn open_summary_batches(&self) -> Result<Vec<SummaryBatchRow>> {
        let rows = self
            .conn
            .query(
                "SELECT id, engine, effort, decision_ids FROM summary_batch ORDER BY submitted_at",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| SummaryBatchRow {
                id: r.get(0),
                engine: r.get(1),
                effort: r.get(2),
                decision_ids: r.get(3),
            })
            .collect())
    }

    #[tracing::instrument(name = "db.insert_summary_batch", skip(self, decision_ids), fields(db.system = "postgresql"))]
    pub async fn insert_summary_batch(
        &self,
        id: &str,
        engine: i16,
        effort: &str,
        decision_ids: &[i64],
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO summary_batch (id, engine, effort, decision_ids) VALUES ($1, $2, $3, $4)",
                &[&id, &engine, &effort, &decision_ids],
            )
            .await?;
        Ok(())
    }

    #[tracing::instrument(name = "db.delete_summary_batch", skip(self), fields(db.system = "postgresql"))]
    pub async fn delete_summary_batch(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM summary_batch WHERE id = $1", &[&id])
            .await?;
        Ok(())
    }
}
