//! The CJUE's and the CEDH's own summaries, pasted whole on each text of their decision
//! in its language. A text that carries one is not asked again, nor a summary fetched twice,
//! but a summary read by OCR is read again from its pages at every sweep.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use anyhow::{anyhow, Result};
use chrono::Datelike;
use futures::StreamExt;
use serde_json::{json, Value};

use lj_parse::{
    cedh_legal_summary, cedh_legal_summary_pending, cjue_case_number, cjue_court_summary,
    cjue_court_summary_ocr, cjue_summary_without_head, OcrBlock, OcrPage,
};
use lj_sources::cedh::{parse_results, CedhClient, PAGE_SIZE};
use lj_sources::cjue::{CjueClient, THROTTLE};
use lj_store::repository::DecisionRepository;
use lj_summary::{COURT_SUMMARY_ENGINE, COURT_SUMMARY_GENERATION};
use lj_vocab::languages::eu_of_iso_639_1;

use crate::config::Settings;
use crate::ocr::{Invented, Ocr};

use super::europe::europe_pool;

/// First year CELLAR dates a summary in.
const FIRST_YEAR: i32 = 1954;

const FETCH_CONCURRENCY: usize = 4;

#[derive(Debug, Default)]
struct Counts {
    pasted: usize,
    held: usize,
    absent: usize,
    unserved: usize,
    titrage: usize,
    unread: usize,
    pending: usize,
    failed: usize,
}

/// The OCR of the PDF summaries, off for the rest of the run once it fails.
struct Reader {
    ocr: Ocr,
    off: AtomicBool,
    pages: AtomicUsize,
}

/// Pastes the summaries of the CJUE and of the CEDH dated from `from_year` to `to_year`,
/// every year by default.
pub async fn sync_court_summaries(from_year: Option<i32>, to_year: Option<i32>) -> Result<()> {
    let from_year = from_year.unwrap_or(FIRST_YEAR).max(FIRST_YEAR);
    let to_year = to_year.unwrap_or_else(|| chrono::Utc::now().year());
    let settings = Settings::from_env()?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    cjue(&settings, &repo, from_year, to_year).await?;
    cedh(&settings, &repo, from_year, to_year).await
}

/// Pastes the CJUE's summaries dated from `from_year`.
pub(super) async fn sync_cjue_summaries(from_year: i32) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    cjue(
        &settings,
        &repo,
        from_year.max(FIRST_YEAR),
        chrono::Utc::now().year(),
    )
    .await
}

/// Pastes the CEDH's legal summaries dated from `from_year`.
pub(super) async fn sync_cedh_summaries(from_year: i32) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    cedh(
        &settings,
        &repo,
        from_year.max(FIRST_YEAR),
        chrono::Utc::now().year(),
    )
    .await
}

/// `_SUM` sums up a judgment of the Recueil, `_RES` digests it, `_INF` informs on a
/// decision left out of the Recueil.
fn summary_rank(celex: &str) -> usize {
    ["_SUM", "_RES", "_INF"]
        .iter()
        .position(|suffix| celex.ends_with(suffix))
        .unwrap_or(3)
}

async fn cjue(
    settings: &Settings,
    repo: &DecisionRepository<'_>,
    from_year: i32,
    to_year: i32,
) -> Result<()> {
    let client = CjueClient::new();
    let cache = settings.paths().cjue().join("summaries");
    std::fs::create_dir_all(&cache)?;
    let ocr = Ocr::from_pool(repo).await?.map(|ocr| Reader {
        ocr,
        off: AtomicBool::new(false),
        pages: AtomicUsize::new(0),
    });
    if ocr.is_none() {
        tracing::warn!("cjue summaries: the mistral-vibe pool is empty, the PDF summaries wait");
    }
    let decisions = repo.cjue_decisions_by_celex().await?;
    let texts = repo
        .court_summary_texts("CJUE", COURT_SUMMARY_ENGINE)
        .await?;
    let mut languages: HashMap<i64, Vec<(String, bool)>> = HashMap::new();
    for ((decision_id, lang), pasted) in texts {
        languages
            .entry(decision_id)
            .or_default()
            .push((lang, pasted));
    }

    let mut chosen: HashMap<String, String> = HashMap::new();
    for year in from_year..=to_year {
        for (summary, celex) in client
            .summaries_of_year(year)
            .await
            .map_err(|e| anyhow!("cjue summaries {year}: {e}"))?
        {
            let better = chosen
                .get(&celex)
                .is_none_or(|held| summary_rank(&summary) < summary_rank(held));
            if better {
                chosen.insert(celex, summary);
            }
        }
    }

    let mut counts = Counts::default();
    let mut work = Vec::new();
    for (celex, summary) in &chosen {
        let Some(decision_id) = decisions.get(celex) else {
            counts.absent += 1;
            continue;
        };
        for (lang, pasted) in languages.get(decision_id).into_iter().flatten() {
            let eu = eu_of_iso_639_1(lang).expect("a CJUE text is in an official language");
            let read = cache.join(format!("{summary}.{eu}.ocr.json")).exists();
            if *pasted && !read {
                counts.held += 1;
            } else {
                work.push((*decision_id, lang.as_str(), eu, summary.as_str()));
            }
        }
    }

    let mut outcomes =
        futures::stream::iter(work.into_iter().map(|(decision_id, lang, eu, summary)| {
            let (client, cache, ocr) = (&client, &cache, ocr.as_ref());
            async move {
                let html = match cjue_summary_html(client, cache, summary, eu).await {
                    Ok(Some(html)) => html,
                    Ok(None) => {
                        let read = match cjue_summary_ocr(client, ocr, cache, summary, eu).await {
                            Ok(Some(pages)) => read_ocr(&pages, summary),
                            Ok(None) => return Outcome::Unserved,
                            Err(error) => Err(error),
                        };
                        return match read {
                            Ok(Some(text)) => Outcome::Text(
                                decision_id,
                                lang,
                                cjue_summary_without_head(&text).to_string(),
                            ),
                            Ok(None) => Outcome::Unread,
                            Err(error) => {
                                tracing::warn!(summary, lang, %error, "cjue summary PDF unread");
                                Outcome::Failed
                            }
                        };
                    }
                    Err(error) => {
                        tracing::warn!(summary, lang, %error, "cjue summary unavailable");
                        return Outcome::Failed;
                    }
                };
                match cjue_court_summary(&html) {
                    Some(text) => Outcome::Text(
                        decision_id,
                        lang,
                        cjue_summary_without_head(&text).to_string(),
                    ),
                    None => Outcome::Titrage,
                }
            }
        }))
        .buffer_unordered(FETCH_CONCURRENCY);
    while let Some(outcome) = outcomes.next().await {
        paste(repo, outcome, &mut counts).await?;
    }
    tracing::info!(
        from_year,
        to_year,
        summaries = chosen.len(),
        ?counts,
        ocr_pages = ocr.as_ref().map_or(0, |o| o.pages.load(Ordering::Relaxed)),
        "court_summaries_cjue"
    );
    Ok(())
}

enum Outcome<'a> {
    Text(i64, &'a str, String),
    Unserved,
    Titrage,
    Unread,
    Pending,
    Failed,
}

async fn paste(
    repo: &DecisionRepository<'_>,
    outcome: Outcome<'_>,
    counts: &mut Counts,
) -> Result<()> {
    match outcome {
        Outcome::Text(decision_id, lang, text) => {
            if repo
                .paste_court_summary(
                    decision_id,
                    lang,
                    &text,
                    COURT_SUMMARY_ENGINE,
                    COURT_SUMMARY_GENERATION,
                )
                .await?
            {
                counts.pasted += 1;
            } else {
                counts.held += 1;
            }
        }
        Outcome::Unserved => counts.unserved += 1,
        Outcome::Titrage => counts.titrage += 1,
        Outcome::Unread => counts.unread += 1,
        Outcome::Pending => counts.pending += 1,
        Outcome::Failed => counts.failed += 1,
    }
    Ok(())
}

/// The summary as CELLAR served it, read from the cache when it was asked before: a
/// rendition is kept, its absence marked.
async fn cjue_summary_html(
    client: &CjueClient,
    cache: &Path,
    summary: &str,
    eu: &str,
) -> Result<Option<String>> {
    let html = cache.join(format!("{summary}.{eu}.html"));
    let none = cache.join(format!("{summary}.{eu}.none"));
    if html.exists() {
        return Ok(Some(std::fs::read_to_string(&html)?));
    }
    if none.exists() {
        return Ok(None);
    }
    tokio::time::sleep(THROTTLE).await;
    let served = client
        .summary_html_in(summary, eu)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    let (path, body): (PathBuf, &str) = match &served {
        Some(body) => (html, body),
        None => (none, ""),
    };
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(served)
}

/// The OCR'd pages of a summary CELLAR serves as a PDF alone, read from the cache when it
/// was read before: the pages are kept, the absence of a PDF marked.
async fn cjue_summary_ocr(
    client: &CjueClient,
    ocr: Option<&Reader>,
    cache: &Path,
    summary: &str,
    eu: &str,
) -> Result<Option<Vec<Value>>> {
    let layout = cache.join(format!("{summary}.{eu}.ocr.json"));
    let none = cache.join(format!("{summary}.{eu}.nopdf"));
    if layout.exists() {
        return Ok(Some(serde_json::from_slice(&std::fs::read(&layout)?)?));
    }
    let Some(ocr) = ocr.filter(|o| !none.exists() && !o.off.load(Ordering::Relaxed)) else {
        return Ok(None);
    };
    tokio::time::sleep(THROTTLE).await;
    let Some(pdf) = client
        .summary_pdf_in(summary, eu)
        .await
        .map_err(|e| anyhow!("{e}"))?
    else {
        std::fs::write(&none, "")?;
        return Ok(None);
    };
    let read = ocr.ocr.document_layout(&pdf, summary).await;
    let read = read.inspect_err(|e| {
        if !e.is::<Invented>() {
            ocr.off.store(true, Ordering::Relaxed)
        }
    })?;
    ocr.pages.fetch_add(read.len(), Ordering::Relaxed);
    let pages: Vec<Value> = read
        .into_iter()
        .map(|page| json!({ "dimensions": page["dimensions"], "blocks": page["blocks"] }))
        .collect();
    let tmp = layout.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(&pages)?)?;
    std::fs::rename(&tmp, &layout)?;
    Ok(Some(pages))
}

/// The summary its OCR'd pages hold, `None` when they hold none.
fn read_ocr(pages: &[Value], summary: &str) -> Result<Option<String>> {
    let case = cjue_case_number(summary).ok_or_else(|| anyhow!("no case number in {summary}"))?;
    let pages = pages
        .iter()
        .map(|page| {
            let blocks = page["blocks"]
                .as_array()
                .ok_or_else(|| anyhow!("an OCR page without blocks"))?
                .iter()
                .map(|block| {
                    let at = |key: &str| {
                        block[key]
                            .as_f64()
                            .ok_or_else(|| anyhow!("an OCR block without {key}"))
                    };
                    Ok(OcrBlock {
                        kind: block["type"].as_str().unwrap_or_default().to_string(),
                        x0: at("top_left_x")?,
                        y0: at("top_left_y")?,
                        x1: at("bottom_right_x")?,
                        y1: at("bottom_right_y")?,
                        content: block["content"].as_str().unwrap_or_default().to_string(),
                    })
                })
                .collect::<Result<_>>()?;
            Ok(OcrPage {
                width: page["dimensions"]["width"]
                    .as_f64()
                    .ok_or_else(|| anyhow!("an OCR page without width"))?,
                blocks,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(cjue_court_summary_ocr(&pages, &case))
}

/// The ISO 639-1 code of a HUDOC language.
fn hudoc_iso(raw: &str) -> Result<&'static str> {
    match raw {
        "FRE" => Ok("fr"),
        "ENG" => Ok("en"),
        other => Err(anyhow!("HUDOC language {other}")),
    }
}

async fn cedh(
    settings: &Settings,
    repo: &DecisionRepository<'_>,
    from_year: i32,
    to_year: i32,
) -> Result<()> {
    let client = CedhClient::new(&settings.egress_proxy)?.with_body_cache(settings.cache_dir());
    let mut decisions: HashMap<(String, String), Vec<i64>> = HashMap::new();
    for (docket, date, id) in repo.docket_date_index("CEDH").await? {
        decisions.entry((docket, date)).or_default().push(id);
    }
    let texts = repo
        .court_summary_texts("CEDH", COURT_SUMMARY_ENGINE)
        .await?;

    let mut counts = Counts::default();
    let mut ambiguous = 0usize;
    let mut work = Vec::new();
    for year in from_year..=to_year {
        let mut start = 0;
        loop {
            let page = client
                .legal_summaries_page(year, start)
                .await
                .map_err(|e| anyhow!("cedh legal summaries {year} start={start}: {e}"))?;
            let (resultcount, rows) = parse_results(&page);
            for row in rows {
                let date = row.columns["kpdate"].as_str().unwrap_or_default();
                let date = date.get(..10).unwrap_or(date).to_string();
                let mut ids: Vec<i64> = row.columns["appno"]
                    .as_str()
                    .unwrap_or_default()
                    .split(';')
                    .filter_map(|appno| decisions.get(&(appno.trim().to_string(), date.clone())))
                    .flatten()
                    .copied()
                    .collect();
                ids.sort_unstable();
                ids.dedup();
                let decision_id = match ids[..] {
                    [] => {
                        counts.absent += 1;
                        continue;
                    }
                    [id] => id,
                    _ => {
                        ambiguous += 1;
                        continue;
                    }
                };
                let lang = hudoc_iso(&row.language)?;
                match texts.get(&(decision_id, lang.to_string())) {
                    None => counts.absent += 1,
                    Some(true) => counts.held += 1,
                    Some(false) => work.push((decision_id, lang, row.itemid)),
                }
            }
            start += PAGE_SIZE;
            if start as u64 >= resultcount {
                break;
            }
        }
    }

    let mut outcomes =
        futures::stream::iter(work.into_iter().map(|(decision_id, lang, itemid)| {
            let client = &client;
            async move {
                match client.body_text(&itemid).await {
                    Ok(body) if body.trim().is_empty() => Outcome::Unserved,
                    Ok(body) => match cedh_legal_summary(&body) {
                        Some(text) => Outcome::Text(decision_id, lang, text),
                        None if cedh_legal_summary_pending(&body) => {
                            match client.forget_body(&itemid) {
                                Ok(()) => Outcome::Pending,
                                Err(error) => {
                                    tracing::warn!(itemid, %error, "cedh pending summary kept in cache");
                                    Outcome::Failed
                                }
                            }
                        }
                        None => {
                            tracing::warn!(itemid, "cedh legal summary of an unknown layout");
                            Outcome::Titrage
                        }
                    },
                    Err(error) => {
                        tracing::warn!(itemid, %error, "cedh legal summary unavailable");
                        Outcome::Failed
                    }
                }
            }
        }))
        .buffer_unordered(FETCH_CONCURRENCY);
    while let Some(outcome) = outcomes.next().await {
        paste(repo, outcome, &mut counts).await?;
    }
    tracing::info!(
        from_year,
        to_year,
        ambiguous,
        ?counts,
        "court_summaries_cedh"
    );
    Ok(())
}
