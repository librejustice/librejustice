//! Re-derives `seat_paths` from the stored seat against the room vocabulary of
//! the moment, without parsing. The full re-extraction does the same at the
//! price of a parse; this one follows the pass that first writes the seats.
//! A dry run prints what the tree of nodes would become, node by node.

use std::collections::HashMap;
use std::time::Instant;

use anyhow::{anyhow, bail, Result};

use lj_extract::seat_link::{SeatRow, SeatVocabulary};
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

const PAGE: i64 = 20_000;
const WRITE_BATCH: usize = 1_000;
const DELTA_LINES: usize = 80;

pub async fn resolve_seats(dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0; SET synchronous_commit = off")
        .await?;
    let repo = DecisionRepository::new(&conn);

    let started = Instant::now();
    let vocabulary =
        SeatVocabulary::new(repo.seat_vocabulary().await?.into_iter().map(|s| SeatRow {
            seat: s.seat,
            chamber: s.chamber,
            formation: s.formation,
            span: s.span,
        }));
    if vocabulary.is_empty() {
        bail!("aucun siège en base : jouer reextract-fields d'abord");
    }
    tracing::info!(
        secondes = started.elapsed().as_secs(),
        "resolve-seats: room vocabulary built"
    );

    let mut after = 0i64;
    let (mut lues, mut changees, mut ecrites) = (0usize, 0usize, 0u64);
    let mut pending: Vec<(i64, Vec<String>)> = Vec::new();
    let mut nodes: HashMap<String, (i64, i64)> = HashMap::new();
    loop {
        let rows = repo.seat_rows_after(after, PAGE).await?;
        let Some((last, ..)) = rows.last() else {
            break;
        };
        after = *last;
        lues += rows.len();
        for (id, seat, chamber, formation, stored) in rows {
            let paths = vocabulary
                .resolve(&SeatRow {
                    seat,
                    chamber,
                    formation,
                    span: None,
                })
                .paths();
            if let Some(leaf) = stored.last() {
                nodes.entry(leaf.clone()).or_default().0 += 1;
            }
            if let Some(leaf) = paths.last() {
                nodes.entry(leaf.clone()).or_default().1 += 1;
            }
            if paths != stored {
                changees += 1;
                pending.push((id, paths));
            }
        }
        if pending.len() >= WRITE_BATCH {
            ecrites += flush(&repo, &mut pending, dry_run).await?;
        }
        if lues % 500_000 == 0 {
            tracing::info!(lues, changees, ecrites, "resolve-seats: running");
        }
    }
    ecrites += flush(&repo, &mut pending, dry_run).await?;
    tracing::info!(
        lues,
        changees,
        ecrites,
        dry_run,
        secondes = started.elapsed().as_secs(),
        "resolve-seats: done"
    );
    if dry_run {
        print_delta(&nodes);
    }
    Ok(())
}

/// The leaves of the tree before and after, and every leaf whose count moves.
fn print_delta(nodes: &HashMap<String, (i64, i64)>) {
    let before = nodes.values().filter(|(b, _)| *b > 0).count();
    let after = nodes.values().filter(|(_, a)| *a > 0).count();
    let mut moved: Vec<(&String, i64, i64)> = nodes
        .iter()
        .filter(|(_, (b, a))| b != a)
        .map(|(k, (b, a))| (k, *b, *a))
        .collect();
    moved.sort_by_key(|(k, b, a)| (-(b - a).abs(), (*k).clone()));
    println!(
        "leaves: {before} before, {after} after; {} leaves change count",
        moved.len()
    );
    for (leaf, b, a) in moved.iter().take(DELTA_LINES) {
        let mark = match (*b, *a) {
            (0, _) => "apparaît",
            (_, 0) => "disparaît",
            _ => "",
        };
        println!("{leaf}\t{b} → {a}\t{mark}");
    }
    if moved.len() > DELTA_LINES {
        println!("… and {} more", moved.len() - DELTA_LINES);
    }
}

async fn flush(
    repo: &DecisionRepository<'_>,
    pending: &mut Vec<(i64, Vec<String>)>,
    dry_run: bool,
) -> Result<u64> {
    let written = if dry_run || pending.is_empty() {
        0
    } else {
        repo.put_seat_paths(pending).await?
    };
    pending.clear();
    Ok(written)
}
