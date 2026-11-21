//! The decisions whose source stops naming the persons they sanction, withdrawn before that day.

use anyhow::{anyhow, Result};
use chrono::{Days, Local};
use lj_store::repository::DecisionRepository;
use serde_json::Value;

use crate::config::Settings;

use super::europe::europe_pool;

/// The sources whose connector writes `named_until`.
const NAMING_SOURCES: &[&str] = &["amf"];
/// A decision page lives seven days at the edge, and one more stale.
const EDGE_DAYS: u64 = 8;

pub async fn expire_named() -> Result<()> {
    let settings = Settings::from_env()?;
    let bucket = settings.bucket()?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let until = Local::now().date_naive() + Days::new(EDGE_DAYS);
    let due = repo.named_until_due(NAMING_SOURCES, until).await?;
    for (source_uid, fields) in &due {
        if let Some(file) = fields.get("file").and_then(Value::as_str) {
            bucket.delete(file).await?;
        }
        repo.expire_provenance(source_uid).await?;
        let named_until = fields.get("named_until").and_then(|v| v.as_str());
        tracing::info!(
            source_uid = %source_uid,
            named_until,
            "a decision withdrawn as its source stops naming"
        );
    }
    tracing::info!(expired = due.len(), "expire_named");
    Ok(())
}
