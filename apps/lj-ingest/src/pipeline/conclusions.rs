//! Conclusions, a row of `conclusions` each: the opinions of the CJUE's Advocates General,
//! pages of their own, and the rapporteur public's at the Conseil d'État, their text in the bucket.

use anyhow::{anyhow, Result};
use chrono::NaiveDate;

use lj_bucket::Bucket;
use lj_parse::parse_cjue;
use lj_store::repository::{ConclusionsWrite, DecisionRepository};
use lj_summary::SummaryEngine;
use lj_vocab::languages::eu_of_iso_639_1;

use crate::config::Settings;

use super::batch::drain_batch;
use super::{IngestCounts, IngestMode};

pub(super) const EUR_LEX: &str = "eur-lex";
const ARIANE_WEB: &str = "ariane-web";

const PAGE: i64 = 200;

/// The Advocate General a CELLAR French title names.
pub(super) fn advocate_general(title: &str) -> Option<String> {
    lj_parse::cjue_advocate_general(title).map(|(_, name)| name)
}

/// Puts each text in the bucket, and fails unless the object is there for the row to name.
pub(super) async fn put_texts(
    bucket: &Bucket,
    source: &str,
    uid: &str,
    texts: &[(&str, &str)],
) -> Result<()> {
    for (lang, text) in texts {
        let key = lj_bucket::key::conclusions_text(source, uid, lang);
        let packed = zstd::encode_all(text.as_bytes(), zstd::DEFAULT_COMPRESSION_LEVEL)?;
        bucket.put(&key, packed).await?;
        if !bucket.contains(&key).await? {
            return Err(anyhow!("{key}: absent from the bucket after its put"));
        }
    }
    Ok(())
}

pub(super) fn iso_date(raw: Option<&str>) -> Option<NaiveDate> {
    raw.and_then(|d| NaiveDate::parse_from_str(d.get(..10)?, "%Y-%m-%d").ok())
}

/// Writes each CJUE opinion without a live page back as a page of kind conclusions: its
/// retired row revived under its address, else a new one, each language's text read from
/// the bucket, its summary kept on its French text.
pub async fn page_cjue_conclusions(dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let bucket = settings.bucket()?;
    let pool = super::europe::europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let mut counts = IngestCounts::default();
    let (mut revived, mut created, mut texts, mut missing, mut summaries, mut pinned) =
        (0usize, 0usize, 0usize, 0usize, 0usize, 0u64);
    let mut after = 0i64;
    loop {
        let page = repo.cjue_conclusions_to_page(after, PAGE).await?;
        let Some(last) = page.last() else { break };
        after = last.id;
        let mut candidates = Vec::new();
        for opinion in &page {
            let languages: Vec<(String, serde_json::Value)> = match &opinion.retired {
                Some((_, provenances)) => provenances
                    .iter()
                    .map(|(uid, fields)| {
                        (
                            uid.rsplit('/').next().unwrap_or_default().to_string(),
                            fields.clone(),
                        )
                    })
                    .collect(),
                None => opinion
                    .langs
                    .iter()
                    .map(|iso| (iso.clone(), opinion.source_fields.clone()))
                    .collect(),
            };
            match opinion.retired {
                Some(_) => revived += 1,
                None => created += 1,
            }
            for (iso, mut fields) in languages {
                let eu = eu_of_iso_639_1(&iso)
                    .ok_or_else(|| anyhow!("{}: language {iso}", opinion.celex))?;
                let key = lj_bucket::key::conclusions_text(EUR_LEX, &opinion.celex, &iso);
                let Some(packed) = bucket.get(&key).await? else {
                    missing += 1;
                    tracing::warn!(celex = %opinion.celex, lang = %iso, "conclusions page: no text in the bucket");
                    continue;
                };
                let text = String::from_utf8(zstd::decode_all(&packed[..])?)?;
                texts += 1;
                if opinion.retired.is_none() {
                    fields["resource_obtained_language"] = serde_json::json!(eu);
                }
                let decision = parse_cjue(&text, &fields, &opinion.celex, &iso)
                    .map_err(|e| anyhow!("parse_cjue {}: {e}", opinion.celex))?;
                candidates.push(super::europe::html_candidate(decision, &fields, &text, eu));
            }
        }
        if !dry_run {
            drain_batch(&conn, candidates, IngestMode::MissingHash, &mut counts).await?;
            let celexes: Vec<String> = page.iter().map(|o| o.celex.clone()).collect();
            pinned += repo.pin_conclusions_pages(&celexes).await?;
            for opinion in &page {
                let (Some((id, _)), Some(summary), Some(engine)) =
                    (&opinion.retired, &opinion.summary, opinion.summary_engine)
                else {
                    continue;
                };
                if !opinion.langs.iter().any(|l| l == "fr") {
                    tracing::warn!(celex = %opinion.celex, "conclusions page: a summary without a French text, left out");
                    continue;
                }
                let prompt_version = SummaryEngine::from_code(engine)
                    .ok_or_else(|| anyhow!("{}: summary engine {engine}", opinion.celex))?
                    .prompt_version();
                repo.set_summary(*id, "fr", summary, None, None, prompt_version, engine)
                    .await?;
                summaries += 1;
            }
        }
        tracing::info!(
            revived,
            created,
            texts,
            missing,
            summaries,
            pinned,
            after,
            dry_run,
            "conclusions page progress"
        );
    }
    tracing::warn!(
        revived,
        created,
        texts,
        missing,
        summaries,
        pinned,
        written = counts.created + counts.updated,
        dry_run,
        "conclusions page done"
    );
    Ok(())
}

/// Reads again the Advocate General of each EUR-Lex conclusions from its title.
pub async fn conclusions_authors() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let rows = repo.conclusions_titles(EUR_LEX).await?;
    let mut changed = 0usize;
    for (id, title, author) in &rows {
        let read = advocate_general(title);
        if read != *author {
            repo.set_conclusions_author(*id, read.as_deref()).await?;
            changed += 1;
        }
    }
    tracing::info!(read = rows.len(), changed, "conclusions authors done");
    Ok(())
}

/// Fetches the rapporteur public's conclusions of the Conseil d'État decisions whose
/// ArianeWeb bundle names them, `limit` at most: PDF and text to the bucket, a row each.
pub async fn sync_ce_conclusions(limit: Option<usize>) -> Result<()> {
    let settings = Settings::from_env()?;
    let bucket = settings.bucket()?;
    let client = lj_sources::ariane::CrpClient::new(&settings.egress_proxy)?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let queue = repo
        .ce_conclusions_to_fetch(limit.map_or(i64::MAX, |l| l as i64))
        .await?;
    let (mut read, mut unserved, mut failed) = (0usize, 0usize, 0usize);
    for (dossier, date) in &queue {
        tokio::time::sleep(lj_sources::ariane::THROTTLE).await;
        let uid = format!("{dossier}@{date}");
        let pdf = match client.pdf(date, dossier).await {
            Ok(pdf) => pdf,
            Err(lj_sources::error::SourceError::Http(error))
                if error
                    .status()
                    .is_some_and(|s| s.as_u16() == 403 || s.as_u16() == 429) =>
            {
                tracing::warn!(%uid, %error, "ce conclusions: the site refuses, run stopped until the next");
                break;
            }
            Err(error) => {
                failed += 1;
                tracing::warn!(%uid, %error, "ce conclusions: fetch failed, asked again next run");
                continue;
            }
        };
        let text = pdf
            .as_deref()
            .and_then(|bytes| lj_sources::pdf::lj_pdf_extract(bytes).ok())
            .filter(|t| !t.trim().is_empty());
        if let Some(bytes) = pdf {
            bucket
                .put(&lj_bucket::key::conclusions_pdf(ARIANE_WEB, &uid), bytes)
                .await?;
        }
        let langs: Vec<String> = match &text {
            Some(text) => {
                put_texts(&bucket, ARIANE_WEB, &uid, &[("fr", text)]).await?;
                read += 1;
                vec!["fr".to_string()]
            }
            None => {
                unserved += 1;
                Vec::new()
            }
        };
        repo.upsert_conclusions(&ConclusionsWrite {
            source: ARIANE_WEB,
            source_uid: &uid,
            jurisdiction_code: "ce",
            case_numbers: std::slice::from_ref(dossier),
            ruled_on: iso_date(Some(date)),
            delivered_on: None,
            author: None,
            langs: &langs,
            source_fields: &serde_json::json!({}),
            public_id: None,
            summary: None,
            summary_engine: None,
        })
        .await?;
    }
    tracing::info!(
        queued = queue.len(),
        read,
        unserved,
        failed,
        "sync-ce-conclusions done"
    );
    Ok(())
}
