//! `treaty-parties`: the states each INTL text binds, written to `legal_text.parties`.

use anyhow::{anyhow, Result};
use chrono::Datelike;
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

pub async fn treaty_parties() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let treaties = repo.treaty_titles().await?;
    let rows: Vec<(String, Option<Vec<&str>>)> = treaties
        .iter()
        .map(|t| {
            let date = t.date.map(|d| {
                jiff::civil::Date::new(d.year() as i16, d.month() as i8, d.day() as i8)
                    .expect("valid date")
            });
            let parties = lj_norm::treaty_parties::parties(&t.title, &t.keywords, date);
            (t.text_uid.clone(), parties)
        })
        .collect();
    let listed = rows.iter().filter(|(_, p)| p.is_some()).count();
    let written = repo.write_treaty_parties(&rows).await?;
    tracing::info!(
        treaties = treaties.len(),
        listed,
        written,
        "treaty-parties: lists written"
    );
    Ok(())
}
