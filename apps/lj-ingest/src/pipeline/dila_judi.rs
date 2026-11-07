//! Where a record of the three judicial funds goes, decided before its text is
//! cut: the verdict of the reading when it has one, its address otherwise.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use anyhow::{anyhow, Result};
use lj_pg::db::Connection;
use lj_store::repository::{DecisionRepository, Identity};

use super::dila::Fond;
use super::{Candidate, IngestCounts};

const DILA_JUDICIAL_RANK: i16 = 30;

enum Verdict {
    Attach(&'static str),
    Copy,
    Create,
    Hold,
}

static VERDICTS: LazyLock<HashMap<(&'static str, Option<u32>), Verdict>> = LazyLock::new(|| {
    include_str!("../../data/dila_juri_verdicts.tsv")
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let cols: Vec<&'static str> = l.split('\t').collect();
            let [juritext, section, verdict, target] = cols[..] else {
                panic!("dila_juri_verdicts.tsv: malformed line {l:?}");
            };
            let section = (!section.is_empty()).then(|| {
                section
                    .parse()
                    .unwrap_or_else(|_| panic!("dila_juri_verdicts.tsv: section {section:?}"))
            });
            let verdict = match verdict {
                "attach" => Verdict::Attach(target),
                "copy" => Verdict::Copy,
                "create" => Verdict::Create,
                "hold" => Verdict::Hold,
                other => panic!("dila_juri_verdicts.tsv: verdict {other:?}"),
            };
            ((juritext, section), verdict)
        })
        .collect()
});

/// What reading decided for a record, its target found in the base.
pub(super) enum Reading {
    Unread,
    Attach(i64),
    Copy,
    Create,
    Hold,
}

pub(super) async fn reading(repo: &DecisionRepository<'_>, uid: &str) -> Result<Reading> {
    Ok(match VERDICTS.get(&record_key(uid)) {
        None => Reading::Unread,
        Some(Verdict::Copy) => Reading::Copy,
        Some(Verdict::Create) => Reading::Create,
        Some(Verdict::Hold) => Reading::Hold,
        Some(Verdict::Attach(public_id)) => match repo.decision_id_by_public_id(public_id).await? {
            Some(id) => Reading::Attach(id),
            None => {
                tracing::warn!(%uid, public_id, "verdict target gone, resolving the address");
                Reading::Unread
            }
        },
    })
}

/// `(JURITEXT, section)` of `dila-<fund>/<JURITEXT>[/<k>]`.
fn record_key(source_uid: &str) -> (&str, Option<u32>) {
    let mut parts = source_uid.split('/').skip(1);
    let juritext = parts.next().unwrap_or_default();
    (juritext, parts.next().and_then(|k| k.parse().ok()))
}

/// Resolves the records the base does not know yet. An attached record is cut
/// only when its text becomes the one served; copies and held records leave.
pub(super) async fn route(
    conn: &Connection,
    fond: Fond,
    batch: Vec<Candidate>,
    counts: &mut IngestCounts,
) -> Result<Vec<Candidate>> {
    let repo = DecisionRepository::new(conn);
    let ctx = super::extract_ctx(conn).await?;
    let uids: Vec<String> = batch
        .iter()
        .map(|c| c.decision.source_uid.clone())
        .collect();
    let known = repo.known_source_uids(&uids).await?;
    let mut out = Vec::with_capacity(batch.len());
    let mut attached = HashSet::new();
    for mut cand in batch {
        let uid = cand.decision.source_uid.clone();
        if known.contains(&uid) {
            out.push(cand);
            continue;
        }
        let (juritext, section) = record_key(&uid);
        if fond == Fond::Inca && held_by_cass(&repo, juritext, section).await? {
            counts.skipped += 1;
            continue;
        }
        let identity = match reading(&repo, &uid).await? {
            Reading::Copy => {
                counts.copies += 1;
                continue;
            }
            Reading::Hold => {
                tracing::warn!(%uid, "record held by the reading");
                counts.held += 1;
                continue;
            }
            Reading::Create => {
                cand.distinct = true;
                out.push(cand);
                continue;
            }
            Reading::Attach(id) => {
                attached.insert(id);
                Identity::Attach(id)
            }
            Reading::Unread => match resolve(&repo, ctx, &cand).await? {
                // A second record of the batch on one decision resolves at its
                // write, which sees the first.
                Identity::Attach(id) if !attached.insert(id) => {
                    out.push(cand);
                    continue;
                }
                identity => identity,
            },
        };
        match identity {
            Identity::Attach(id) => {
                cand.passages = repo
                    .top_source_rank(id)
                    .await?
                    .is_none_or(|rank| rank <= DILA_JUDICIAL_RANK);
                cand.decision_id = Some(id);
                out.push(cand);
            }
            Identity::Create => out.push(cand),
            Identity::Copy(_) => counts.copies += 1,
            Identity::Hold(id) => {
                tracing::warn!(%uid, decision_id = id, "record held: neither the copy of that decision nor another one");
                counts.held += 1;
            }
        }
    }
    Ok(out)
}

async fn resolve(
    repo: &DecisionRepository<'_>,
    ctx: &super::ExtractCtx,
    cand: &Candidate,
) -> Result<Identity> {
    let extracted = lj_extract_rows::extracted_fields(&cand.decision, &ctx.snapshots())
        .map_err(|e| anyhow!("extract {}: {e}", cand.decision.source_uid))?;
    Ok(repo
        .identity_of(&cand.decision, Some(&extracted), &serde_json::Value::Null)
        .await?)
}

/// A file both funds deliver is one record, the Bulletin's.
async fn held_by_cass(
    repo: &DecisionRepository<'_>,
    juritext: &str,
    section: Option<u32>,
) -> Result<bool> {
    let uids: Vec<String> = match section {
        Some(k) => vec![format!("dila-cass/{juritext}/{k}")],
        None => vec![
            format!("dila-cass/{juritext}"),
            format!("dila-cass/{juritext}/1"),
        ],
    };
    Ok(!repo.known_source_uids(&uids).await?.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verdict_table_reads() {
        assert!(!VERDICTS.is_empty());
        assert!(VERDICTS
            .keys()
            .all(|(juritext, _)| juritext.starts_with("JURITEXT")));
    }
}
