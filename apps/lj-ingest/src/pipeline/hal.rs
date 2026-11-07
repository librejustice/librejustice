//! HAL -> web doctrine commentaries: a deposit citing a docket number becomes
//! an outbound `kind: "note"` link on the decision it comments.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use clap::ValueEnum;

use lj_parse::{build_hal_source_fields, extract_dockets, extract_law_numbers};
use lj_sources::hal;
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// How far back a scan reads HAL's submission dates.
#[derive(Copy, Clone, Debug, Default, ValueEnum)]
pub enum HalScan {
    /// 30 days: the daily pass.
    #[default]
    Daily,
    /// 180 days: the idempotent net, which warns about what the daily pass missed
    /// and normally repairs nothing.
    Repair,
    /// The whole law fonds: initial load, and the only scan that retires the
    /// links it no longer gives.
    Full,
}

impl HalScan {
    fn window_days(self) -> Option<u32> {
        match self {
            HalScan::Daily => Some(30),
            HalScan::Repair => Some(180),
            HalScan::Full => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            HalScan::Daily => "daily",
            HalScan::Repair => "repair",
            HalScan::Full => "full",
        }
    }
}

#[derive(Default)]
struct Stats {
    pages: usize,
    deposits: usize,
    citing: usize,
    upserted: usize,
    unchanged: usize,
    orphans: usize,
    citing_law: usize,
    law_upserted: usize,
    law_unchanged: usize,
    law_orphans: usize,
    kept: Vec<String>,
    kept_laws: Vec<String>,
}

/// Reads HAL's recent law deposits and attaches those citing a docket number
/// to the decisions already in the stock.
pub async fn sync_hal(scan: HalScan) -> Result<()> {
    let settings = Settings::from_env()?;

    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let existing = repo.web_commentary_checksums("hal").await?;
    let existing_laws = repo.article_commentaire_checksums("hal").await?;

    let client = hal::http_client();
    let mut stats = Stats::default();
    let mut cursor = hal::CURSOR_START.to_string();
    loop {
        let page = hal::scan_page(&client, scan.window_days(), &cursor).await?;
        if stats.pages == 0 {
            tracing::info!(
                window = scan.label(),
                deposits = page.total,
                "sync_hal started"
            );
        }
        stats.pages += 1;
        stats.deposits += page.deposits.len();
        for deposit in &page.deposits {
            if !deposit.is_law() || deposit.link().is_none() {
                continue;
            }
            let dockets = extract_dockets(&deposit.haystack());
            if dockets.is_empty() {
                continue;
            }
            stats.citing += 1;
            attach(&repo, deposit, &dockets, &existing, &mut stats).await?;
        }
        for deposit in &page.deposits {
            if !deposit.is_law() || deposit.link().is_none() {
                continue;
            }
            let laws = extract_law_numbers(deposit);
            if laws.is_empty() {
                continue;
            }
            stats.citing_law += 1;
            attach_law(&repo, deposit, &laws, &existing_laws, &mut stats).await?;
        }
        if page.next_cursor == cursor || page.deposits.is_empty() {
            break;
        }
        cursor = page.next_cursor;
        tokio::time::sleep(hal::THROTTLE).await;
    }

    let (retired, laws_retired) = match scan {
        HalScan::Full => (
            repo.tombstone_absent_web_commentaries("hal", &stats.kept)
                .await?,
            repo.tombstone_absent_article_commentaires("hal", &stats.kept_laws)
                .await?,
        ),
        HalScan::Daily | HalScan::Repair => (0, 0),
    };
    if matches!(scan, HalScan::Repair) && stats.upserted > 0 {
        tracing::warn!(
            repaired = stats.upserted,
            "the HAL net caught up what the daily pass missed"
        );
    }
    tracing::info!(
        pages = stats.pages,
        deposits = stats.deposits,
        citing_a_docket = stats.citing,
        upserted = stats.upserted,
        unchanged = stats.unchanged,
        orphans = stats.orphans,
        citing_a_law = stats.citing_law,
        laws_upserted = stats.law_upserted,
        laws_unchanged = stats.law_unchanged,
        laws_orphans = stats.law_orphans,
        retired,
        laws_retired,
        "sync_hal finished"
    );
    Ok(())
}

async fn attach(
    repo: &DecisionRepository<'_>,
    deposit: &lj_parse::HalDeposit,
    dockets: &[String],
    existing: &HashMap<String, String>,
    stats: &mut Stats,
) -> Result<()> {
    let matches = repo.cassation_decisions_by_dockets(dockets).await?;
    if matches.is_empty() {
        stats.orphans += 1;
        return Ok(());
    }
    let source_fields = build_hal_source_fields(deposit);
    let payload = serde_json::to_vec(&source_fields)?;
    let checksum = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(&payload));
    let mut seen: HashSet<i64> = HashSet::new();
    for (id, public_id, _) in matches {
        if !seen.insert(id) {
            continue;
        }
        let source_uid = format!("hal/{}#{public_id}", deposit.hal_id);
        stats.kept.push(source_uid.clone());
        if existing.get(&source_uid) == Some(&checksum) {
            stats.unchanged += 1;
            continue;
        }
        repo.upsert_decision_source(id, &source_uid, &checksum, "json", &source_fields)
            .await?;
        stats.upserted += 1;
    }
    Ok(())
}

/// Attaches a deposit to the laws its title names, as a commentary of the
/// whole text (`num_key` NULL).
async fn attach_law(
    repo: &DecisionRepository<'_>,
    deposit: &lj_parse::HalDeposit,
    laws: &[String],
    existing: &HashMap<String, String>,
    stats: &mut Stats,
) -> Result<()> {
    let slugs = repo.law_slugs_by_numbers(laws).await?;
    if slugs.is_empty() {
        stats.law_orphans += 1;
        return Ok(());
    }
    let source_fields = build_hal_source_fields(deposit);
    let payload = serde_json::to_vec(&source_fields)?;
    let checksum = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(&payload));
    let mut seen: HashSet<String> = HashSet::new();
    for (_, slug) in slugs {
        if !seen.insert(slug.clone()) {
            continue;
        }
        let source_uid = format!("hal/{}#{slug}", deposit.hal_id);
        stats.kept_laws.push(source_uid.clone());
        if existing.get(&source_uid) == Some(&checksum) {
            stats.law_unchanged += 1;
            continue;
        }
        repo.upsert_article_commentaire(&slug, None, "hal", &source_uid, &checksum, &source_fields)
            .await?;
        stats.law_upserted += 1;
    }
    Ok(())
}
