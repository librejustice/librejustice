//! Text identity curation from a JSONL dataset: the `rename-texts` command. Each line
//! `{from, to?, jurisdiction?, collapse?, version?, delete?, redirect?}` applies one operation: a uid
//! rename cascaded through its rows ([`rename_legal_text`]), a collapse into a twin that
//! carries the same content ([`collapse_legal_text`]), a merge into the official reading of
//! the same act as one of its versions ([`merge_legal_text_version`]), or the deletion of a
//! text with no object ([`delete_legal_text`]). The reading ranks are refreshed once for the
//! batch, and the decisions' citations follow with `reextract-fields --field
//! legal_references`. Resumable: a text already handled is skipped.
//!
//! [`rename_legal_text`]: DecisionRepository::rename_legal_text
//! [`collapse_legal_text`]: DecisionRepository::collapse_legal_text
//! [`merge_legal_text_version`]: DecisionRepository::merge_legal_text_version
//! [`delete_legal_text`]: DecisionRepository::delete_legal_text

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

use lj_norm::text_aliases;
use lj_store::repository::{DecisionRepository, TextVersionMerge};

use crate::config::Settings;

/// One line of the dataset.
#[derive(Debug, Deserialize)]
struct RenameRecord {
    from: String,
    /// The target: absent for a deletion, required otherwise.
    to: Option<String>,
    /// Replaces the `jurisdiction` column of a renamed text when given and different (a
    /// bilateral instrument misfiled outside `INTL`).
    jurisdiction: Option<String>,
    /// `to` carries the same content: `from`'s content is dropped, its incoming references
    /// move.
    #[serde(default)]
    collapse: bool,
    /// `from` is another reading of the act `to` carries: its articles become closed
    /// versions of `to`.
    #[serde(default)]
    version: bool,
    /// The text has no object and no twin: it goes.
    #[serde(default)]
    delete: bool,
    /// An old slug redirecting to `from` that goes, for the text that bore it to take it back
    /// at its next load: a merge undone.
    redirect: Option<String>,
}

pub async fn rename_texts(dataset: &Path, dry_run: bool) -> Result<()> {
    let raw = std::fs::read_to_string(dataset)
        .with_context(|| format!("reading {}", dataset.display()))?;
    let records: Vec<RenameRecord> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).with_context(|| format!("invalid line: {l}")))
        .collect::<Result<_>>()?;

    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let (mut renamed, mut collapsed, mut merged, mut deleted, mut skipped) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut unredirected = 0u64;
    let mut articles = 0u64;
    for rec in &records {
        if let Some(slug) = &rec.redirect {
            if dry_run {
                println!("{}: the redirect {slug:?} goes", rec.from);
                continue;
            }
            match repo.drop_text_slug_redirect(&rec.from, slug).await? {
                0 => skipped += 1,
                _ => {
                    unredirected += 1;
                    tracing::info!(text_uid = rec.from, slug, "slug redirect dropped");
                }
            }
            continue;
        }
        let target = || -> Result<&str> {
            rec.to
                .as_deref()
                .ok_or_else(|| anyhow!("{}: `to` is required outside a deletion", rec.from))
        };
        if rec.version {
            let to = target()?;
            let Some(plan) = repo.text_version_merge(&rec.from, to).await? else {
                skipped += 1;
                continue;
            };
            let problems = merge_problems(&rec.from, &plan);
            if dry_run {
                println!("{}", merge_line(&rec.from, to, &plan, &problems));
                continue;
            }
            if !problems.is_empty() {
                bail!("{} into {to}: {}", rec.from, problems.join("; "));
            }
        } else if dry_run {
            continue;
        }
        let (done, verb) = if rec.delete {
            (repo.delete_legal_text(&rec.from).await?, "deleted")
        } else if rec.collapse {
            (
                repo.collapse_legal_text(&rec.from, target()?).await?,
                "collapsed",
            )
        } else if rec.version {
            (
                repo.merge_legal_text_version(&rec.from, target()?, &text_aliases::of(target()?))
                    .await?,
                "merged as a version",
            )
        } else {
            (
                repo.rename_legal_text(&rec.from, target()?, rec.jurisdiction.as_deref())
                    .await?,
                "renamed",
            )
        };
        match done {
            Some(counts) => {
                if rec.delete {
                    deleted += 1;
                } else if rec.collapse {
                    collapsed += 1;
                } else if rec.version {
                    merged += 1;
                    articles += counts.articles;
                } else {
                    renamed += 1;
                    articles += counts.articles;
                }
                tracing::info!(from = rec.from, to = rec.to, ?counts, "text {verb}");
            }
            None => skipped += 1,
        }
    }
    if dry_run {
        return Ok(());
    }
    let (ranked_texts, _) = repo.refresh_reading_ranks().await?;
    tracing::info!(
        renamed,
        collapsed,
        merged,
        deleted,
        unredirected,
        skipped,
        articles,
        ranked_texts,
        "rename-texts"
    );
    println!(
        "rename-texts: {renamed} renamed and {merged} merged as versions ({articles} articles \
         moved), {collapsed} collapsed, {deleted} deleted, {unredirected} redirects dropped, \
         {skipped} done already.\n\
         Next: reextract-fields --field legal_references for the decisions citing them."
    );
    Ok(())
}

/// What keeps `from` from becoming a version of its target. The embedded names count: one
/// still keyed by `from` would name nothing once it is gone.
fn merge_problems(from: &str, plan: &TextVersionMerge) -> Vec<String> {
    let mut out = Vec::new();
    if plan.into_articles == 0 {
        out.push("the target has no article".to_string());
    } else if plan.empty_windows > 0 {
        out.push(format!(
            "{} rows would close on or before they open",
            plan.empty_windows
        ));
    }
    if plan.collisions > 0 {
        out.push(format!(
            "{} rows hold a key the target holds",
            plan.collisions
        ));
    }
    let aliased = lj_extract::data::LINK_ALIASES_TSV
        .lines()
        .filter(|l| l.split('\t').nth(2) == Some(from))
        .count();
    if aliased > 0 {
        out.push(format!("link_aliases.tsv names it on {aliased} lines"));
    }
    if !text_aliases::of(from).is_empty() {
        out.push("text_aliases.tsv names it".to_string());
    }
    out
}

fn merge_line(from: &str, to: &str, plan: &TextVersionMerge, problems: &[String]) -> String {
    format!(
        "{from} -> {to}: {} articles join {} ({} keys shared, {} only in {from}), closed at {}, \
         {} owned links, slug {} -> {}, title {:?}, target names {:?}; {}",
        plan.from_articles,
        plan.into_articles,
        plan.shared_keys,
        plan.from_only_keys,
        plan.close.map_or("-".to_string(), |d| d.to_string()),
        plan.owned_links,
        plan.from_slug.as_deref().unwrap_or("-"),
        plan.into_slug.as_deref().unwrap_or("-"),
        plan.from_title,
        text_aliases::of(to)
            .iter()
            .map(|a| format!("{} {} {}", a.kind.as_str(), a.lang, a.value))
            .collect::<Vec<_>>(),
        if problems.is_empty() {
            "ready".to_string()
        } else {
            problems.join("; ")
        }
    )
}
