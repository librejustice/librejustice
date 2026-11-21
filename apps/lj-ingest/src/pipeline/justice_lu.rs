//! data.public.lu: the decisions of Luxembourg's judicial order, one archive per dataset and year,
//! each archive the portal uploaded again since its last load read whole and loaded by the
//! standard upsert path.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use chrono::NaiveDate;

use lj_core::chamber::Seat;
use lj_parse::{lu_court, parse_justice_lu, JusticeLuParsed, LuMember, LU_PLACE};
use lj_sources::justice_lu::{html_text, members, JusticeLuClient};
use lj_store::repository::{DecisionRepository, ExtractedFields, JurisdictionRow};

use crate::config::Settings;

use super::batch::run_stream;
use super::cnda::pdf_has_text_layer;
use super::europe::europe_pool;
use super::{content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode, WriteMode};

/// The daily run starts no archive past this, the rest waiting for the next run.
const BUDGET: Duration = Duration::from_secs(1_200);
/// Summaries of selected decisions, which the decisions themselves carry whole.
const LEFT_OUT: [&str; 1] = ["extraits-de-decisions-judiciaires-fiches-judoc"];

/// The URL of each archive loaded whole, by resource id: the portal uploads a changed archive
/// under a new URL.
fn loaded_path(settings: &Settings) -> PathBuf {
    settings.state_dir.join("ingest/justice-lu/loaded.json")
}

fn read_loaded(path: &Path) -> Result<BTreeMap<String, String>> {
    match std::fs::read(path) {
        Ok(raw) => Ok(serde_json::from_slice(&raw)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e.into()),
    }
}

fn write_loaded(path: &Path, loaded: &BTreeMap<String, String>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let part = path.with_extension("part");
    std::fs::write(&part, serde_json::to_vec_pretty(loaded)?)?;
    std::fs::rename(&part, path)?;
    Ok(())
}

fn candidate(parsed: JusticeLuParsed, raw: &[u8], format: &str) -> Candidate {
    let court = parsed.court;
    let seat = Seat {
        jurisdiction_type: court.jurisdiction_type.to_string(),
        jurisdiction_code: Some(court.code.to_string()),
        ..Default::default()
    };
    let extracted = ExtractedFields {
        date_lecture: parsed
            .decision
            .date_lecture
            .as_deref()
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()),
        docket_numbers: parsed.decision.numero_dossiers.clone().unwrap_or_default(),
        seat_paths: seat.paths(),
        seat: Some(seat),
        jurisdiction: Some(JurisdictionRow {
            code: court.code.to_string(),
            source_code: court.code.to_string(),
            jurisdiction_type: court.jurisdiction_type.to_string(),
            city: None,
            label: court.label.to_string(),
            place: Some(LU_PLACE.to_string()),
        }),
        ..Default::default()
    };
    Candidate {
        decision_id: None,
        public_id: generate_public_id(),
        content_checksum: content_checksum(raw),
        prebuilt_source_fields: Some(parsed.source_fields),
        prebuilt_extracted: Some(extracted),
        decision: parsed.decision,
        raw_payload: Vec::new(),
        payload_format: format.to_string(),
        write_mode: WriteMode::Full,
        passages: true,
        distinct: false,
    }
}

#[derive(Default)]
struct Refused {
    no_text: usize,
    unread: usize,
}

/// The decisions of one archive: a PDF read by `pdftotext`, an HTML by its reading blocks.
fn archive_candidates(
    dataset: &str,
    dataset_title: &str,
    archive: &str,
    bytes: &[u8],
) -> Result<(Vec<Candidate>, Refused)> {
    let mut out = Vec::new();
    let mut refused = Refused::default();
    for member in members(bytes)? {
        let text = match member.format {
            "pdf" => lj_sources::pdf::pdftotext_extract(&member.bytes)
                .ok()
                .filter(|t| pdf_has_text_layer(t)),
            _ => html_text(&String::from_utf8_lossy(&member.bytes)),
        };
        let Some(text) = text else {
            tracing::debug!(dataset, archive, file = %member.name, "justice-lu file without text");
            refused.no_text += 1;
            continue;
        };
        let lu_member = LuMember {
            dataset,
            dataset_title,
            archive,
            name: &member.name,
            format: member.format,
        };
        match parse_justice_lu(&lu_member, &text) {
            Ok(parsed) => out.push(candidate(parsed, &member.bytes, member.format)),
            Err(e) => {
                tracing::debug!(error = %e, "justice-lu file refused");
                refused.unread += 1;
            }
        }
    }
    Ok((out, refused))
}

/// Loads every archive the portal uploaded since its last load, dataset by dataset. `full` reads
/// every archive with no time budget; `datasets` takes only those; `list` counts the archives of
/// each dataset and loads nothing.
pub async fn sync_justice_lu(full: bool, datasets: &[String], list: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let client = JusticeLuClient::new(&settings.egress_proxy)?;
    let mut taken = Vec::new();
    for dataset in client.datasets().await? {
        if lu_court(&dataset.slug).is_some() {
            taken.push(dataset);
        } else if !LEFT_OUT.contains(&dataset.slug.as_str()) {
            tracing::warn!(dataset = %dataset.slug, title = %dataset.title, "justice-lu: a dataset no court holds is left out");
        }
    }
    if let Some(unknown) = datasets
        .iter()
        .find(|d| !taken.iter().any(|t| &t.slug == *d))
    {
        bail!("justice-lu: no dataset {unknown}; `sync-justice-lu --list` names them");
    }
    if !datasets.is_empty() {
        taken.retain(|t| datasets.contains(&t.slug));
    }
    if list {
        for dataset in &taken {
            let bytes: u64 = dataset.resources.iter().filter_map(|r| r.filesize).sum();
            println!(
                "{}\t{}\t{}\t{}",
                dataset.resources.len(),
                bytes / 1_000_000,
                lu_court(&dataset.slug).expect("a taken dataset").code,
                dataset.slug
            );
        }
        return Ok(());
    }

    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let write_conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    if !DecisionRepository::new(&conn).justice_lu_lock().await? {
        tracing::info!("justice-lu sync: another run holds the lock");
        return Ok(());
    }
    let loaded_path = loaded_path(&settings);
    let mut loaded = read_loaded(&loaded_path)?;
    let started = Instant::now();
    let (mut read, mut no_text, mut unread) = (0usize, 0usize, 0usize);
    let mut totals = IngestCounts::default();
    for dataset in &taken {
        if !full && started.elapsed() > BUDGET {
            break;
        }
        let due: Vec<_> = dataset
            .resources
            .iter()
            .filter(|r| r.format == "zip" && (full || loaded.get(&r.id) != Some(&r.url)))
            .collect();
        if due.is_empty() {
            continue;
        }
        let mut done: Vec<(String, String)> = Vec::new();
        let mut refused = Refused::default();
        let (client, done_ref, refused_ref) = (&client, &mut done, &mut refused);
        let counts = run_stream(
            &conn,
            &write_conn,
            IngestMode::MissingHash,
            |tx| async move {
                for resource in due {
                    if !full && started.elapsed() > BUDGET {
                        break;
                    }
                    let bytes = client.archive(resource).await?;
                    let (slug, title, archive) = (
                        dataset.slug.clone(),
                        dataset.title.clone(),
                        resource.title.clone(),
                    );
                    let (candidates, archive_refused) = tokio::task::spawn_blocking(move || {
                        archive_candidates(&slug, &title, &archive, &bytes)
                    })
                    .await??;
                    refused_ref.no_text += archive_refused.no_text;
                    refused_ref.unread += archive_refused.unread;
                    for c in candidates {
                        if tx.send(c).await.is_err() {
                            return Ok(IngestCounts::default());
                        }
                    }
                    done_ref.push((resource.id.clone(), resource.url.clone()));
                }
                Ok(IngestCounts::default())
            },
        )
        .await?;
        read += done.len();
        no_text += refused.no_text;
        unread += refused.unread;
        loaded.extend(done);
        write_loaded(&loaded_path, &loaded)?;
        totals.merge(&counts);
        tracing::info!(
            dataset = %dataset.slug,
            created = counts.created,
            updated = counts.updated,
            skipped = counts.skipped,
            "justice-lu sync"
        );
    }
    let left = taken
        .iter()
        .flat_map(|d| &d.resources)
        .filter(|r| r.format == "zip" && loaded.get(&r.id) != Some(&r.url))
        .count();
    if left > 0 {
        tracing::warn!(
            left,
            "justice-lu sync: budget spent, the archives left wait for the next run"
        );
    }
    tracing::warn!(
        archives = read,
        no_text,
        unread,
        created = totals.created,
        updated = totals.updated,
        skipped = totals.skipped,
        errors = totals.errors,
        "justice-lu sync done"
    );
    Ok(())
}
