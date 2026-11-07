//! Law journals and blogs over OAI-PMH -> outbound `kind: "note"` links on the
//! cassation decisions they comment, on the `sync-hal` model.

use std::collections::{HashMap, HashSet};
use std::ops::AddAssign;

use anyhow::{anyhow, bail, Result};
use clap::ValueEnum;

use lj_parse::{build_doctrine_source_fields, cited_decisions, same_note, DoctrineNote};
use lj_sources::oai::{Harvester, Journal, JOURNALS};
use lj_store::repository::{DecisionRepository, WebNoteHead};

use crate::config::Settings;

const SOURCE: &str = "doctrine-oai";

/// How far back a scan reads the OAI datestamps.
#[derive(Copy, Clone, Debug, Default, ValueEnum)]
pub enum DoctrineScan {
    /// 30 days: the daily pass.
    #[default]
    Daily,
    /// 180 days: the idempotent net, which warns about what the daily pass
    /// missed and normally repairs nothing.
    Repair,
    /// Every record: initial load.
    Full,
}

impl DoctrineScan {
    fn window_days(self) -> Option<u64> {
        match self {
            DoctrineScan::Daily => Some(30),
            DoctrineScan::Repair => Some(180),
            DoctrineScan::Full => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            DoctrineScan::Daily => "daily",
            DoctrineScan::Repair => "repair",
            DoctrineScan::Full => "full",
        }
    }
}

#[derive(Default, Clone, Copy)]
struct Stats {
    records: usize,
    notes: usize,
    citing: usize,
    candidates: usize,
    day_refused: usize,
    hal_duplicates: usize,
    orphans: usize,
    attached: usize,
    upserted: usize,
    unchanged: usize,
}

impl AddAssign for Stats {
    fn add_assign(&mut self, o: Self) {
        self.records += o.records;
        self.notes += o.notes;
        self.citing += o.citing;
        self.candidates += o.candidates;
        self.day_refused += o.day_refused;
        self.hal_duplicates += o.hal_duplicates;
        self.orphans += o.orphans;
        self.attached += o.attached;
        self.upserted += o.upserted;
        self.unchanged += o.unchanged;
    }
}

type HalNotes = HashMap<i64, Vec<WebNoteHead>>;

/// Attaches the notes writing a cassation docket to the decisions in the
/// stock; `dry_run` writes nothing and logs each link it would write.
pub async fn sync_doctrine_oai(scan: DoctrineScan, dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;

    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let existing = repo.web_commentary_checksums(SOURCE).await?;
    let mut hal: HalNotes = HashMap::new();
    for head in repo.web_note_heads("hal").await? {
        hal.entry(head.decision_id).or_default().push(head);
    }

    let from = scan
        .window_days()
        .map(|d| (chrono::Utc::now().date_naive() - chrono::Days::new(d)).to_string());
    tracing::info!(
        window = scan.label(),
        from = from.as_deref().unwrap_or("-"),
        dry_run,
        "sync_doctrine_oai started"
    );
    let mut harvester = Harvester::new(&settings.egress_proxy)?;
    let mut total = Stats::default();
    let mut failed: Vec<&str> = Vec::new();
    for journal in &JOURNALS {
        let harvest = match harvester.harvest(journal, from.as_deref()).await {
            Ok(h) => h,
            Err(e) => {
                tracing::error!(journal = journal.key, error = %e, "journal harvest failed");
                failed.push(journal.key);
                continue;
            }
        };
        let mut stats = Stats {
            records: harvest.records,
            notes: harvest.notes.len(),
            ..Stats::default()
        };
        for note in &harvest.notes {
            attach(&repo, journal, note, &existing, &hal, dry_run, &mut stats).await?;
        }
        tracing::info!(
            journal = journal.key,
            records = stats.records,
            notes = stats.notes,
            citing_a_docket = stats.citing,
            candidate_links = stats.candidates,
            refused_by_the_day = stats.day_refused,
            already_on_hal = stats.hal_duplicates,
            orphans = stats.orphans,
            attached = stats.attached,
            upserted = stats.upserted,
            unchanged = stats.unchanged,
            "doctrine journal read"
        );
        total += stats;
    }

    if matches!(scan, DoctrineScan::Repair) && total.upserted > 0 {
        tracing::warn!(
            repaired = total.upserted,
            "the doctrine net caught up what the daily pass missed"
        );
    }
    tracing::info!(
        records = total.records,
        notes = total.notes,
        citing_a_docket = total.citing,
        candidate_links = total.candidates,
        refused_by_the_day = total.day_refused,
        already_on_hal = total.hal_duplicates,
        orphans = total.orphans,
        attached = total.attached,
        upserted = total.upserted,
        unchanged = total.unchanged,
        dry_run,
        "sync_doctrine_oai finished"
    );
    if !failed.is_empty() {
        bail!("journals unread: {}", failed.join(", "));
    }
    Ok(())
}

async fn attach(
    repo: &DecisionRepository<'_>,
    journal: &Journal,
    note: &DoctrineNote,
    existing: &HashMap<String, String>,
    hal: &HalNotes,
    dry_run: bool,
    stats: &mut Stats,
) -> Result<()> {
    let citation = cited_decisions(&note.cites);
    if citation.dockets.is_empty() {
        return Ok(());
    }
    stats.citing += 1;
    let matches = repo
        .cassation_decisions_by_dockets(&citation.dockets)
        .await?;
    if matches.is_empty() {
        stats.orphans += 1;
        return Ok(());
    }
    let source_fields = build_doctrine_source_fields(note);
    let payload = serde_json::to_vec(&source_fields)?;
    let checksum = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(&payload));
    let mut seen: HashSet<i64> = HashSet::new();
    for (id, public_id, decided) in matches {
        if !seen.insert(id) {
            continue;
        }
        stats.candidates += 1;
        let decided = decided.map(|d| d.to_string());
        if !citation.admits(decided.as_deref()) {
            stats.day_refused += 1;
            continue;
        }
        let on_hal = hal.get(&id).is_some_and(|notes| {
            notes
                .iter()
                .any(|h| same_note(note, h.publisher.as_deref(), h.title.as_deref()))
        });
        if on_hal {
            stats.hal_duplicates += 1;
            continue;
        }
        stats.attached += 1;
        let source_uid = format!("{SOURCE}/{}#{public_id}", note.uid);
        if existing.get(&source_uid) == Some(&checksum) {
            stats.unchanged += 1;
            continue;
        }
        if dry_run {
            tracing::info!(
                journal = journal.key,
                note = %note.url,
                title = %note.title,
                decision = %public_id,
                decided = decided.as_deref().unwrap_or("-"),
                "doctrine link"
            );
        } else {
            repo.upsert_decision_source(id, &source_uid, &checksum, "json", &source_fields)
                .await?;
        }
        stats.upserted += 1;
    }
    Ok(())
}
