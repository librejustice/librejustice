//! The JO texts that name their addressees, that are no norm or that print a code,
//! and the texts listed by the operator, leave the norms for their publication.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use flate2::write::GzEncoder;
use flate2::Compression;

use lj_norm::individual::Leaves;
use lj_store::repository::{DecisionRepository, JournalMoveCandidate, JournalMoveCounts};

use crate::config::Settings;

const LOT: usize = 200;

/// Which texts leave: those with neither body nor article that name their
/// addressees, else one of the other sets.
pub enum MoveSelection {
    Bare,
    /// The measures of role `individuel` that have a body or articles.
    Measures,
    /// The texts of a notice nature.
    Notices,
    /// The codes a codification act printed as its annex, LEGI holding them consolidated.
    Codes,
    /// The texts of every order that are no norm, a ruling the corpus holds leading to it.
    NoNorm,
    /// The texts listed in the file, one uid a line, whatever their journal.
    Listed(PathBuf),
}

fn group(c: &JournalMoveCandidate) -> &'static str {
    if lj_norm::individual::NOTICE_NATURES.contains(&c.nature.as_str()) {
        "notice"
    } else if c.nature == "CODE" {
        "code"
    } else if c.role.as_deref() == Some("individuel") {
        "role"
    } else if c.title.contains(lj_vocab::PROTECTED_ACCESS_MARKER) {
        "protected"
    } else if c.nature == "RESULTATS" {
        "results"
    } else {
        "formula"
    }
}

/// The texts of every order that are no norm, but those a source other than their journal
/// consolidates, and the rulings among them, `(text_uid, title)`.
async fn no_norm(
    repo: &DecisionRepository<'_>,
) -> Result<(Vec<JournalMoveCandidate>, Vec<(String, String)>)> {
    let read = repo
        .journal_move_no_norm(
            &lj_norm::individual::no_norm_natures(),
            &lj_norm::applicability::journal_article_sources(),
        )
        .await
        .map_err(|e| anyhow!("journal_move_no_norm: {e}"))?;
    let mut counts: BTreeMap<(String, String, &str), usize> = BTreeMap::new();
    let mut candidates = Vec::new();
    let mut rulings = Vec::new();
    for (c, jurisdiction, consolidated) in read {
        let Some(leaves) = lj_norm::individual::leaves_norms(&jurisdiction, &c.nature, &c.title)
        else {
            continue;
        };
        let kind = match (leaves, consolidated) {
            (_, true) => "consolidated",
            (Leaves::Journal, false) => "journal",
            (Leaves::Decision, false) => "ruling",
        };
        *counts
            .entry((jurisdiction, c.nature.to_uppercase(), kind))
            .or_default() += 1;
        if consolidated {
            continue;
        }
        if leaves == Leaves::Decision {
            rulings.push((c.text_uid.clone(), c.title.clone()));
        }
        candidates.push(c);
    }
    for ((jurisdiction, nature, kind), texts) in &counts {
        tracing::info!(
            jurisdiction,
            nature,
            kind,
            texts,
            "move-to-journal: no norm"
        );
    }
    Ok((candidates, rulings))
}

fn unconsolidated(measures: Vec<(JournalMoveCandidate, bool)>) -> Vec<JournalMoveCandidate> {
    tracing::info!(
        consolidated = measures.iter().filter(|(_, c)| *c).count(),
        "move-to-journal: texts holding a consolidated article stay"
    );
    measures
        .into_iter()
        .filter_map(|(m, consolidated)| (!consolidated).then_some(m))
        .collect()
}

/// Counts the candidates of `selection` by group; with `apply`, moves them by
/// lots, each lot traced before it is written.
pub async fn move_to_journal(selection: MoveSelection, apply: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Selecting the measures reads 260,000 texts and their articles, 90 s on 07/11.
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let mut rulings: HashMap<String, i64> = HashMap::new();
    let candidates: Vec<JournalMoveCandidate> = match selection {
        MoveSelection::Bare => repo
            .journal_move_candidates(
                lj_vocab::PROTECTED_ACCESS_MARKER,
                lj_norm::individual::INDIVIDUAL_MEASURE,
            )
            .await
            .map_err(|e| anyhow!("journal_move_candidates: {e}"))?,
        MoveSelection::Listed(path) => {
            let listed: Vec<String> = std::fs::read_to_string(&path)
                .map_err(|e| anyhow!("{}: {e}", path.display()))?
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect();
            let found = repo
                .journal_move_listed(&listed)
                .await
                .map_err(|e| anyhow!("journal_move_listed: {e}"))?;
            if found.len() != listed.len() {
                let found: Vec<&str> = found.iter().map(|c| c.text_uid.as_str()).collect();
                let missing: Vec<&String> = listed
                    .iter()
                    .filter(|u| !found.contains(&u.as_str()))
                    .collect();
                anyhow::bail!("move-to-journal: no text or no publication for {missing:?}");
            }
            found
        }
        MoveSelection::Measures => unconsolidated(
            repo.journal_move_measures()
                .await
                .map_err(|e| anyhow!("journal_move_measures: {e}"))?,
        ),
        MoveSelection::Notices => unconsolidated(
            repo.journal_move_notices(&lj_norm::individual::NOTICE_NATURES)
                .await
                .map_err(|e| anyhow!("journal_move_notices: {e}"))?,
        ),
        MoveSelection::Codes => unconsolidated(
            repo.journal_move_notices(&["CODE"])
                .await
                .map_err(|e| anyhow!("journal_move_notices: {e}"))?,
        ),
        MoveSelection::NoNorm => {
            let (candidates, printed) = no_norm(&repo).await?;
            let dockets: Vec<(String, &str, String)> = printed
                .iter()
                .filter_map(|(uid, title)| {
                    lj_norm::individual::printed_ruling(title)
                        .map(|(court, docket)| (uid.clone(), court, docket))
                })
                .collect();
            rulings = repo
                .printed_rulings(&dockets)
                .await
                .map_err(|e| anyhow!("printed_rulings: {e}"))?
                .into_iter()
                .collect();
            for (uid, title) in printed.iter().filter(|(uid, _)| !rulings.contains_key(uid)) {
                tracing::info!(
                    uid,
                    title,
                    "move-to-journal: a ruling the corpus lacks stays a notice"
                );
            }
            tracing::info!(
                rulings = printed.len(),
                docketed = dockets.len(),
                held = rulings.len(),
                "move-to-journal: the rulings the corpus holds lead to their decision"
            );
            candidates
        }
    };
    let mut groups: BTreeMap<&str, usize> = BTreeMap::new();
    for c in &candidates {
        *groups.entry(group(c)).or_default() += 1;
    }
    let count = |g: &str| groups.get(g).copied().unwrap_or(0);
    tracing::info!(
        candidates = candidates.len(),
        role = count("role"),
        protected = count("protected"),
        formula = count("formula"),
        results = count("results"),
        notice = count("notice"),
        code = count("code"),
        linked = candidates.iter().filter(|c| c.linked).count(),
        apply,
        "move-to-journal: candidates"
    );
    if !apply {
        return Ok(());
    }

    let bucket = settings.bucket()?;
    let name = format!(
        "move-to-journal-{}.jsonl.gz",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
    );
    let dir = settings.paths().root().join("backup");
    std::fs::create_dir_all(&dir).map_err(|e| anyhow!("{}: {e}", dir.display()))?;
    let path = dir.join(&name);
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .append(true)
        .open(&path)
        .map_err(|e| anyhow!("{}: {e}", path.display()))?;
    let uids: Vec<String> = candidates.into_iter().map(|c| c.text_uid).collect();
    let mut moved = JournalMoveCounts::default();
    for lot in uids.chunks(LOT) {
        let held: Vec<(String, i64)> = lot
            .iter()
            .filter_map(|uid| rulings.get(uid).map(|id| (uid.clone(), *id)))
            .collect();
        let lines = repo
            .journal_move_trace(lot)
            .await
            .map_err(|e| anyhow!("journal_move_trace: {e}"))?;
        let mut gz = GzEncoder::new(&mut file, Compression::default());
        for line in &lines {
            gz.write_all(line.as_bytes())?;
            gz.write_all(b"\n")?;
        }
        gz.finish()?;
        file.sync_data()?;
        moved += repo
            .move_to_journal(lot, &held)
            .await
            .map_err(|e| anyhow!("move_to_journal: {e}"))?;
    }
    repo.analyze_legal_text()
        .await
        .map_err(|e| anyhow!("analyze legal_text: {e}"))?;
    let bytes = std::fs::read(&path).map_err(|e| anyhow!("{}: {e}", path.display()))?;
    bucket
        .put(&format!("backup/{name}"), bytes)
        .await
        .map_err(|e| anyhow!("bucket put {name}: {e}"))?;
    tracing::info!(
        texts = moved.texts,
        slugs = moved.slugs,
        publications = moved.publications,
        links = moved.links,
        articles = moved.articles,
        bodies = moved.bodies,
        trace = %path.display(),
        "move-to-journal: moved"
    );
    Ok(())
}
