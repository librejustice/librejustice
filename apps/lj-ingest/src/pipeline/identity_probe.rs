//! Each provenance ingested lately, resolved as if it arrived now into the base
//! without it, by the reading's verdict when it has one and its address otherwise;
//! a resolution that differs from where it sits goes to the report, and with
//! `apply` a provenance alone on its decision moves to where it resolves.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use lj_core::decision::Decision;
use lj_parse::FromSourceFields;
use lj_store::repository::{source_from_source_uid, DecisionRepository, Identity};
use serde_json::json;

use super::dila_judi::Reading;
use crate::config::Settings;

const PAGE: usize = 500;

#[derive(Default, Debug)]
struct Tally {
    resolved: usize,
    unchanged: usize,
    /// Attached today, created by the address.
    split: usize,
    /// Attached today, attached elsewhere by the address.
    moved: usize,
    /// Alone today, attached by the address.
    merged: usize,
    copy: usize,
    hold: usize,
    errors: usize,
}

/// With `apply`, sequentially, each resolution reading the base the previous
/// ones left: a copy or a held record is deleted, and a provenance alone on its
/// decision that belongs to another merges into it, and a shared one moves where
/// reading put it. A shared one the address alone would move or split is only
/// reported.
pub async fn probe_identity(days: i32, sources: &[String], out: &Path, apply: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0").await?;
    let ctx = super::extract_ctx(&conn).await?;
    let repo = DecisionRepository::new(&conn);

    let uids = repo.recent_provenance_uids(days, sources).await?;
    tracing::info!(
        provenances = uids.len(),
        days,
        ?sources,
        "identity probe: worklist"
    );
    let mut report = std::io::BufWriter::new(
        std::fs::File::create(out).with_context(|| format!("create {}", out.display()))?,
    );
    let mut tallies: BTreeMap<&'static str, Tally> = BTreeMap::new();
    let mut to_fuse = Vec::new();
    for (page, chunk) in uids.chunks(PAGE).enumerate() {
        for p in repo.provenances_to_resolve(chunk).await? {
            let (decision_id, uid, source_fields, shared) =
                (p.decision_id, p.source_uid, p.source_fields, p.shared);
            let tally = tallies.entry(source_from_source_uid(&uid)).or_default();
            let decision = Decision::from_source_fields(&p.read_text, &source_fields, &uid);
            let extracted =
                match lj_extract_rows::reread_fields(&decision, &source_fields, &ctx.snapshots()) {
                    Ok(fields) => fields,
                    Err(e) => {
                        tracing::debug!(%uid, error = %e, "identity probe: extract");
                        tally.errors += 1;
                        continue;
                    }
                };
            let reading = super::dila_judi::reading(&repo, &uid).await?;
            let read = !matches!(reading, Reading::Unread);
            let now = match reading {
                Reading::Unread => {
                    repo.identity_of(&decision, Some(&extracted), &source_fields)
                        .await?
                }
                Reading::Attach(id) => Identity::Attach(id),
                Reading::Create => Identity::Create,
                // Reading names no target: the record's own decision stands in.
                Reading::Copy => Identity::Copy(decision_id),
                Reading::Hold => Identity::Hold(decision_id),
            };
            tally.resolved += 1;
            let kind = match (now, shared) {
                // Alone, a CJUE work still finds its own decision by its ECLI.
                (Identity::Attach(id), _) if id == decision_id => None,
                (Identity::Create, false) => None,
                (Identity::Create, true) => Some("split"),
                (Identity::Attach(_), true) => Some("moved"),
                (Identity::Attach(_), false) => Some("merged"),
                (Identity::Copy(_), _) => Some("copy"),
                (Identity::Hold(_), _) => Some("hold"),
            };
            let Some(kind) = kind else {
                tally.unchanged += 1;
                continue;
            };
            match kind {
                "split" => tally.split += 1,
                "moved" => tally.moved += 1,
                "merged" => tally.merged += 1,
                "copy" => tally.copy += 1,
                _ => tally.hold += 1,
            }
            let target = match now {
                Identity::Attach(id) | Identity::Copy(id) | Identity::Hold(id) => Some(id),
                Identity::Create => None,
            };
            let line = json!({
                "source_uid": uid,
                "decision_id": decision_id,
                "shared": shared,
                "kind": kind,
                "target_id": target,
                "court": extracted.jurisdiction.as_ref().map(|j| j.source_code.clone()),
                "date": extracted.date_lecture.map(|d| d.to_string()),
                "dockets": extracted.docket_numbers,
                "ecli": decision.ecli,
                "canonical_ref": lj_extract_rows::canonical_ref(&decision),
            });
            writeln!(report, "{line}")?;
            if apply {
                report.flush()?;
                match (now, shared) {
                    (Identity::Attach(target), false) => {
                        repo.merge_into(target, decision_id).await?;
                        to_fuse.push(target);
                    }
                    (Identity::Attach(target), true) if read && target != decision_id => {
                        let from = repo.move_provenance(&uid, target).await?;
                        to_fuse.extend([target, from]);
                    }
                    (Identity::Copy(_) | Identity::Hold(_), _) => {
                        repo.delete_provenance(&uid).await?;
                        if shared {
                            to_fuse.push(decision_id);
                        }
                    }
                    _ => {}
                }
            }
        }
        if page % 20 == 19 {
            tracing::info!(
                done = (page + 1) * PAGE,
                total = uids.len(),
                "identity probe progress"
            );
        }
    }
    report.flush()?;
    for (source, t) in &tallies {
        tracing::info!(source, ?t, "identity probe");
    }
    if apply {
        for chunk in to_fuse.chunks(500) {
            super::reextract::fuse_written(&repo, chunk).await?;
        }
        tracing::warn!(fused = to_fuse.len(), "identity probe applied");
    }
    Ok(())
}
