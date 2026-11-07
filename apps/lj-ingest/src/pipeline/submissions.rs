//! `submissions`: the review of the files accounts submit at `/decision/contribute`.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, bail, Result};
use lj_core::decision::Decision;
use lj_parse::{
    clean_ocr_markdown, contribution_source_fields, parse_cnda, reflow_cnda_pdf_text,
    reflow_page_text,
};
use lj_store::repository::{canonical_jurisdiction_code, DecisionRepository, JurisdictionRow};
use serde_json::{json, Value};

use crate::config::Settings;
use crate::ocr::Ocr;

use super::batch::drain_batch;
use super::cnda::{cnda_candidate, pdf_has_text_layer};
use super::europe::europe_pool;
use super::{
    content_checksum, extract_ctx, generate_public_id, Candidate, IngestCounts, IngestMode,
    WriteMode, BATCH_SIZE,
};

/// What the reviewer read in the decision's text.
pub struct Approval {
    pub court: String,
    pub number: String,
    pub date: Option<String>,
    pub name: Option<String>,
    pub code: Option<String>,
}

pub async fn list() -> Result<()> {
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    for s in DecisionRepository::new(&conn).pending_submissions().await? {
        println!(
            "{}\t{}\t{} bytes\t{}",
            s.id,
            s.created_at.format("%Y-%m-%d %H:%M"),
            s.bytes,
            s.account
        );
    }
    Ok(())
}

/// The file's text, then what the workbench's detector finds in it.
pub async fn show(id: i64) -> Result<()> {
    let (_, file) = fetch(id).await?;
    let (text, _) = text_of(id, &file, false).await?;
    println!("{text}\n");
    let feed = lj_redact_core::office::plain(&text)?;
    for found in lj_redact_core::suggest(&feed) {
        println!("{:?}\t{}\t{}", found.kind, found.count, found.text);
    }
    Ok(())
}

pub async fn reject(id: i64) -> Result<()> {
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    if !DecisionRepository::new(&conn).delete_submission(id).await? {
        bail!("no submission {id}");
    }
    tracing::info!(id, "submission rejected");
    Ok(())
}

pub async fn approve(id: i64, approval: Approval) -> Result<()> {
    let (account, file) = fetch(id).await?;
    let (text, payload_format) = text_of(id, &file, approval.court == "CNDA").await?;
    let uid = format!("contribution/{id}");

    let candidate = if approval.court == "CNDA" {
        let mut parsed = parse_cnda(&text, &json!({}), &approval.number)?;
        parsed.decision.source_uid = uid;
        if let (Some(fields), Value::Object(read_back)) = (
            parsed.source_fields.as_object_mut(),
            contribution_source_fields(&parsed.decision),
        ) {
            fields.extend(read_back);
        }
        parsed.source_fields["account"] = json!(account);
        cnda_candidate(parsed, &file, payload_format)
    } else {
        let decision = decision_of(&uid, text, &approval);
        let mut fields = contribution_source_fields(&decision);
        fields["account"] = json!(account);
        Candidate {
            decision_id: None,
            public_id: generate_public_id(),
            content_checksum: content_checksum(&file),
            prebuilt_source_fields: Some(fields),
            prebuilt_extracted: None,
            decision,
            raw_payload: Vec::new(),
            payload_format: payload_format.to_string(),
            write_mode: WriteMode::Full,
            passages: true,
            distinct: false,
        }
    };

    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let mut counts = IngestCounts::default();
    drain_batch(&conn, vec![candidate], IngestMode::MissingHash, &mut counts).await?;
    if counts.errors > 0 || counts.skipped > 0 {
        bail!("submission {id} not written: {counts:?}");
    }
    DecisionRepository::new(&conn).delete_submission(id).await?;
    tracing::info!(id, ?counts, "submission approved");
    Ok(())
}

/// One line of the `decisions.jsonl` that `load` reads.
#[derive(serde::Deserialize)]
struct Delivered {
    uid: String,
    jurisdiction_code: String,
    formation: Option<String>,
    date: Option<String>,
    numbers: Vec<String>,
}

/// Refuses a court the extraction does not place, since every re-extraction places it again.
pub async fn load(dir: &Path, dry_run: bool, mode: IngestMode) -> Result<()> {
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let courts: HashMap<String, JurisdictionRow> = DecisionRepository::new(&conn)
        .load_jurisdictions()
        .await?
        .into_iter()
        .map(|j| (j.code.clone(), j))
        .collect();
    let ctx = extract_ctx(&conn).await?;
    let mut candidates = Vec::new();
    let mut unplaced = Vec::new();
    for line in std::fs::read_to_string(dir.join("decisions.jsonl"))?.lines() {
        let d: Delivered = serde_json::from_str(line)?;
        let court = courts
            .get(&d.jurisdiction_code)
            .ok_or_else(|| anyhow!("{}: no court {}", d.uid, d.jurisdiction_code))?;
        let text = std::fs::read_to_string(dir.join("texte").join(format!("{}.txt", d.uid)))?;
        let uid = format!("contribution/{}", content_checksum(d.uid.as_bytes()));
        let judicial = matches!(
            court.jurisdiction_type.as_str(),
            "CA" | "TJ" | "TCOM" | "CPH" | "CC"
        );
        let checksum = content_checksum(text.as_bytes());
        let approval = Approval {
            court: court.jurisdiction_type.clone(),
            number: d.numbers.first().cloned().unwrap_or_default(),
            date: d.date,
            name: Some(court.label.clone()),
            code: judicial.then(|| court.source_code.clone()),
        };
        let mut decision = decision_of(&uid, text, &approval);
        decision.numero_dossier = d.numbers.first().cloned();
        decision.numero_dossiers = (!d.numbers.is_empty()).then_some(d.numbers);
        decision.formation = d.formation;
        let placed = lj_extract::extract::jurisdiction_scanned(&decision, None, &[]).map(|j| {
            ctx.jur_codes.get(&j.code).cloned().unwrap_or_else(|| {
                canonical_jurisdiction_code(&j.jurisdiction_type, j.city.as_deref(), &j.code)
            })
        });
        if placed.as_deref() != Some(court.code.as_str()) {
            unplaced.push(format!("{} ({})", d.uid, court.code));
            continue;
        }
        candidates.push(Candidate {
            decision_id: None,
            public_id: generate_public_id(),
            content_checksum: checksum,
            prebuilt_source_fields: Some(contribution_source_fields(&decision)),
            prebuilt_extracted: None,
            decision,
            raw_payload: Vec::new(),
            payload_format: "pdf".to_string(),
            write_mode: WriteMode::Full,
            passages: true,
            distinct: false,
        });
    }
    if !unplaced.is_empty() {
        bail!(
            "the extraction does not place {} courts: {}",
            unplaced.len(),
            unplaced.join(", ")
        );
    }
    tracing::info!(decisions = candidates.len(), "contributions built");
    if dry_run {
        return Ok(());
    }

    let mut counts = IngestCounts::default();
    let mut rest = candidates.into_iter().peekable();
    while rest.peek().is_some() {
        drain_batch(
            &conn,
            rest.by_ref().take(BATCH_SIZE).collect(),
            mode,
            &mut counts,
        )
        .await?;
    }
    if counts.errors > 0 {
        bail!("contributions not all written: {counts:?}");
    }
    tracing::info!(?counts, "contributions loaded");
    Ok(())
}

async fn fetch(id: i64) -> Result<(String, Vec<u8>)> {
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    DecisionRepository::new(&conn)
        .submission(id)
        .await?
        .ok_or_else(|| anyhow!("no submission {id}"))
}

/// A PDF without a text layer is read by Mistral OCR, kept under the cache so that `show` and
/// `approve` pay it once.
async fn text_of(id: i64, file: &[u8], cnda: bool) -> Result<(String, &'static str)> {
    if lj_sources::docx::is_zip_container(file) {
        let text = lj_sources::docx::extract_docx_text(file)?;
        return Ok((clean_ocr_markdown(&text), "docx"));
    }
    let mut text = lj_sources::pdf::pdftotext_extract(file)?;
    if !pdf_has_text_layer(&text) {
        text = scanned_text(id, file).await?;
    }
    let text = if cnda {
        reflow_cnda_pdf_text(&text)
    } else {
        reflow_page_text(&text)
    };
    Ok((text, "pdf"))
}

async fn scanned_text(id: i64, file: &[u8]) -> Result<String> {
    let path = Settings::from_env()?
        .cache_dir()
        .join("submissions")
        .join(format!("{id}.md"));
    if let Ok(markdown) = std::fs::read_to_string(&path) {
        return Ok(markdown);
    }
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let Some(ocr) = Ocr::from_pool(&DecisionRepository::new(&conn)).await? else {
        bail!("the PDF has no text layer, and the mistral-vibe pool holds no key to read it");
    };
    let markdown = ocr.document(file, &format!("submission {id}")).await?;
    std::fs::create_dir_all(path.parent().expect("a cache file has a folder"))?;
    std::fs::write(&path, &markdown)?;
    tracing::info!(id, chars = markdown.len(), "submission read by OCR");
    Ok(markdown)
}

/// A decision of an administrative court is placed by its registry code (`TA75`), a judicial one
/// by its Judilibre location (`ca_paris`).
fn decision_of(uid: &str, text: String, approval: &Approval) -> Decision {
    let judicial = matches!(approval.court.as_str(), "CA" | "TJ" | "TCOM" | "CPH" | "CC");
    Decision {
        source_uid: uid.to_string(),
        member_name: approval.number.clone(),
        ecli: None,
        jurisdiction_source_code: approval.code.clone().filter(|_| !judicial),
        chamber: None,
        nac: None,
        jurisdiction_name: approval.name.clone(),
        jurisdiction_type: Some(approval.court.clone()),
        jurisdiction_location: approval.code.clone().filter(|_| judicial),
        numero_dossier: Some(approval.number.clone()),
        numero_dossiers: Some(vec![approval.number.clone()]),
        numero_role: None,
        date_lecture: approval.date.clone(),
        date_audience: None,
        date_mise_jour: None,
        formation: None,
        type_decision: None,
        type_recours: None,
        solution: None,
        publication_codes: Vec::new(),
        avocat_requerant: None,
        listed_counsel: Vec::new(),
        themes: Vec::new(),
        attacked: None,
        texte_integral_raw: text.clone(),
        texte_integral_clean: text,
        sections: Vec::new(),
        metadata_header: String::new(),
        parse_warnings: Vec::new(),
    }
}
