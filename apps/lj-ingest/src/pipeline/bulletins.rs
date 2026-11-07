//! Ministerial bulletins → every act a `text_publication` of its bulletin, the
//! doctrine and the regulatory acts also a `legal_text`, identity
//! `bo_<site>_<id>`, body from the PDF by `pdftotext`, Mistral OCR when scanned.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{anyhow, Result};
use chrono::{Datelike, NaiveDate};

use lj_sources::bulletins::{self, BulletinClient, BulletinNotice, BulletinSite};
use lj_store::repository::{DecisionRepository, LegalTextRow, PublicationRow};

use super::circulaires::body_from_text_layer;
use crate::config::Settings;
use crate::ocr::Ocr;

/// The bulletins are read from this day, when publication on its ministry's
/// bulletin made a circular applicable (art. R. 312-7 CRPA).
fn first_day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2019, 1, 1).expect("valid date")
}

/// A scanned notice the OCR missed waits this long before a daily run asks
/// again; `--full` asks at once.
const OCR_RETRY_DAYS: i64 = 7;

#[derive(Debug, Default)]
struct Counts {
    listed: usize,
    todo: usize,
    earlier: usize,
    held_elsewhere: usize,
    norms: usize,
    publications: usize,
    ocr: usize,
    ocr_missed: usize,
    ocr_later: usize,
    no_body: usize,
    failed: usize,
}

enum Body {
    Text {
        text: String,
        ocr: bool,
    },
    /// Scanned, and no OCR key answered: left unvisited for the next run.
    OcrMissed,
    /// No document, a dead link or an unreadable PDF.
    Missing,
}

pub async fn sync_bulletins(site: Option<BulletinSite>, full: bool, dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let client = BulletinClient::new(
        (!settings.page_gateway.is_empty()).then(|| settings.page_gateway.clone()),
        settings.cache_dir(),
    );
    // The year before is read only while it is under 60 days old.
    let since = if full {
        first_day().year()
    } else {
        (chrono::Utc::now().date_naive() - chrono::Days::new(60)).year()
    };
    let sites = site.map_or_else(|| BulletinSite::ALL.to_vec(), |s| vec![s]);
    if dry_run {
        for site in sites {
            dry_run_site(&client, site, since).await?;
        }
        return Ok(());
    }

    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let ocr = Ocr::from_pool(&repo).await?;
    let mut failed_sites = Vec::new();
    for site in sites {
        let dir = bulletins::site_dir(&settings.cache_dir(), site);
        match sync_site(&repo, &client, ocr.as_ref(), &dir, site, since, full).await {
            Ok(c) => tracing::info!(
                site = site.slug(),
                listed = c.listed,
                todo = c.todo,
                earlier = c.earlier,
                held_elsewhere = c.held_elsewhere,
                norms = c.norms,
                publications = c.publications,
                ocr = c.ocr,
                ocr_missed = c.ocr_missed,
                ocr_later = c.ocr_later,
                no_body = c.no_body,
                failed = c.failed,
                "sync_bulletins site"
            ),
            Err(e) => {
                tracing::error!(site = site.slug(), error = %e, "sync_bulletins site failed");
                failed_sites.push(site.slug());
            }
        }
    }
    let slugged = super::slugs::assign_text_slugs(&repo).await?;
    tracing::info!(slugged, "sync_bulletins slugs");
    if !failed_sites.is_empty() {
        anyhow::bail!("sync_bulletins: {} failed", failed_sites.join(", "));
    }
    Ok(())
}

async fn sync_site(
    repo: &DecisionRepository<'_>,
    client: &BulletinClient,
    ocr: Option<&Ocr>,
    dir: &Path,
    site: BulletinSite,
    since: i32,
    full: bool,
) -> Result<Counts> {
    let mut published = bulletins::load_published(dir)?;
    let mut missed = bulletins::load_ocr_missed(dir)?;
    let today = chrono::Utc::now().date_naive();
    let notices = client.list(site, since).await?;
    let first = first_day();
    let (todo, later): (Vec<BulletinNotice>, Vec<BulletinNotice>) = notices
        .iter()
        .filter(|n| !before(n, first) && !published.contains(&n.native_id))
        .cloned()
        .partition(|n| {
            full || missed
                .get(&n.native_id)
                .is_none_or(|&d| (today - d).num_days() >= OCR_RETRY_DAYS)
        });
    let mut c = Counts {
        listed: notices.len(),
        todo: todo.len(),
        ocr_later: later.len(),
        ..Counts::default()
    };
    for mut n in todo {
        let uid = n.text_uid();
        if let Err(e) = client.enrich(&mut n).await {
            tracing::warn!(uid = %uid, error = %e, "bulletin: notice page failed, retried next run");
            c.failed += 1;
            continue;
        }
        if before(&n, first) {
            c.earlier += 1;
            published.insert(n.native_id);
            continue;
        }
        let body = if n.legifrance_uid.is_some() {
            None
        } else {
            match fetch_body(client, ocr, dir, &n).await {
                Ok(Body::Text { text, ocr }) => {
                    c.ocr += usize::from(ocr);
                    Some(text)
                }
                Ok(Body::OcrMissed) => {
                    c.ocr_missed += 1;
                    missed.insert(n.native_id, today);
                    save_ocr_missed(dir, &missed).await?;
                    continue;
                }
                Ok(Body::Missing) => {
                    c.no_body += 1;
                    None
                }
                Err(e) => {
                    tracing::warn!(uid = %uid, error = %e, "bulletin: body fetch failed, retried next run");
                    c.failed += 1;
                    continue;
                }
            }
        };
        if n.nor.is_none() {
            n.nor = body.as_deref().and_then(bulletins::nor_in_text);
        }
        let mut nature = norm_nature(&n);
        if nature.is_some() && held_elsewhere(repo, &n, &uid).await? {
            c.held_elsewhere += 1;
            nature = None;
        }
        let body = match nature {
            Some(nature) => {
                repo.upsert_legal_text(&row(&n, nature, body))
                    .await
                    .map_err(|e| anyhow!("upsert_legal_text {uid}: {e}"))?;
                c.norms += 1;
                None
            }
            None => body,
        };
        repo.upsert_publications(&[publication(&n, body)])
            .await
            .map_err(|e| anyhow!("upsert_publications {uid}: {e}"))?;
        c.publications += 1;
        missed.remove(&n.native_id);
        published.insert(n.native_id);
        if c.publications.is_multiple_of(25) {
            save_published(dir, &published).await?;
        }
    }
    save_published(dir, &published).await?;
    save_ocr_missed(dir, &missed).await?;
    Ok(c)
}

/// Published before the first day, the signature standing in for a
/// publication date the listing does not give; a notice whose dates only its
/// own page gives is decided once enriched.
fn before(n: &BulletinNotice, first: NaiveDate) -> bool {
    n.published.or(n.signed).is_some_and(|d| d < first)
}

/// The norms hold the doctrine, and the arrêtés and décisions that do not
/// name their addressees.
fn norm_nature(n: &BulletinNotice) -> Option<&'static str> {
    match n.nature()? {
        "CIRCULAIRE" => Some("CIRCULAIRE"),
        nature @ ("ARRETE" | "DECISION")
            if !lj_norm::individual::is_individual_measure(&n.title) =>
        {
            Some(nature)
        }
        _ => None,
    }
}

/// Another row of the referential already carries the notice's NOR.
async fn held_elsewhere(
    repo: &DecisionRepository<'_>,
    n: &BulletinNotice,
    uid: &str,
) -> Result<bool> {
    let Some(nor) = &n.nor else {
        return Ok(false);
    };
    let held = repo
        .legal_text_uids_by_nor(std::slice::from_ref(nor))
        .await?;
    Ok(held.get(nor).is_some_and(|other| other != uid))
}

async fn dry_run_site(client: &BulletinClient, site: BulletinSite, since: i32) -> Result<()> {
    let notices = client.list(site, since).await?;
    let first = first_day();
    let read: Vec<&BulletinNotice> = notices.iter().filter(|n| !before(n, first)).collect();
    let norms: Vec<&BulletinNotice> = read
        .iter()
        .copied()
        .filter(|n| norm_nature(n).is_some())
        .collect();
    tracing::info!(
        site = site.slug(),
        listed = notices.len(),
        read = read.len(),
        norms = norms.len(),
        doctrine = norms.iter().filter(|n| n.doctrine).count(),
        with_nor = read.iter().filter(|n| n.nor.is_some()).count(),
        with_body = read
            .iter()
            .filter(|n| n.body_url.is_some() || n.body.is_some())
            .count(),
        on_legifrance = read.iter().filter(|n| n.legifrance_uid.is_some()).count(),
        "sync_bulletins dry run"
    );
    for n in read.iter().take(3) {
        tracing::info!(uid = %n.text_uid(), nor = ?n.nor, kind = %n.kind, nature = ?n.nature(), norm = ?norm_nature(n), published = ?n.published, title = %n.title, body = ?n.body_url, "sync_bulletins sample");
    }
    Ok(())
}

fn row(n: &BulletinNotice, nature: &str, body: Option<String>) -> LegalTextRow {
    let title = n.title.to_lowercase();
    let status = if n.abrogated || title.contains("[abrogé") || title.contains("(abrogé") {
        "ABROGE"
    } else {
        "VIGUEUR"
    };
    LegalTextRow {
        text_uid: n.text_uid(),
        jurisdiction: "FR".to_string(),
        title: n.title.clone(),
        title_key: lj_extract::extract::normalize_instrument(&n.title),
        nature: nature.to_string(),
        last_modified: n.published.or(n.signed),
        date_texte: n.signed,
        date_publi: n.published,
        eli: None,
        nor: n.nor.clone(),
        instrument_key: None,
        body,
        status: Some(status.to_string()),
    }
}

fn publication(n: &BulletinNotice, body: Option<String>) -> PublicationRow {
    let uid = n.text_uid();
    PublicationRow {
        journal_code: lj_norm::applicability::journal_code(&uid, "FR"),
        text_uid: uid,
        nature: n.nature().map(str::to_string),
        nor: n.nor.clone(),
        title: n.title.clone(),
        date_texte: n.signed,
        date_publi: n.published,
        url: n.notice_url.clone(),
        body,
    }
}

async fn fetch_body(
    client: &BulletinClient,
    ocr: Option<&Ocr>,
    dir: &Path,
    n: &BulletinNotice,
) -> Result<Body> {
    if let Some(text) = &n.body {
        return Ok(Body::Text {
            text: text.clone(),
            ocr: false,
        });
    }
    let Some(url) = &n.body_url else {
        return Ok(Body::Missing);
    };
    let Some(bytes) = client.get_bytes(url).await? else {
        tracing::warn!(uid = %n.text_uid(), url = %url, "bulletin: document gone");
        return Ok(Body::Missing);
    };
    let pdf = bytes.clone();
    let layer = tokio::task::spawn_blocking(move || lj_sources::pdf::pdftotext_extract(&pdf))
        .await
        .map_err(|e| anyhow!("pdftotext task: {e}"))?;
    let layer = match layer {
        Ok(text) => text,
        Err(e) => {
            tracing::warn!(uid = %n.text_uid(), error = %e, "bulletin: unreadable pdf");
            return Ok(Body::Missing);
        }
    };
    if let Some(text) = body_from_text_layer(&layer) {
        return Ok(Body::Text { text, ocr: false });
    }
    if let Some(md) = bulletins::load_cached_ocr(dir, &n.native_id)? {
        return Ok(Body::Text {
            text: lj_parse::clean_ocr_markdown(&md),
            ocr: true,
        });
    }
    let Some(ocr) = ocr else {
        return Ok(Body::OcrMissed);
    };
    match ocr.document(&bytes, &n.text_uid()).await {
        Ok(md) => {
            bulletins::save_cached_ocr(dir, &n.native_id, &md)?;
            Ok(Body::Text {
                text: lj_parse::clean_ocr_markdown(&md),
                ocr: true,
            })
        }
        Err(e) => {
            tracing::warn!(uid = %n.text_uid(), error = %e, "bulletin: scanned OCR fallback failed");
            Ok(Body::OcrMissed)
        }
    }
}

async fn save_ocr_missed(dir: &Path, missed: &BTreeMap<String, NaiveDate>) -> Result<()> {
    let (dir, missed) = (dir.to_path_buf(), missed.clone());
    tokio::task::spawn_blocking(move || bulletins::save_ocr_missed(&dir, &missed))
        .await
        .map_err(|e| anyhow!("ocr_missed task: {e}"))?
        .map_err(|e| anyhow!("save_ocr_missed: {e}"))
}

async fn save_published(dir: &Path, published: &BTreeSet<String>) -> Result<()> {
    let (dir, published) = (dir.to_path_buf(), published.clone());
    tokio::task::spawn_blocking(move || bulletins::save_published(&dir, &published))
        .await
        .map_err(|e| anyhow!("published task: {e}"))?
        .map_err(|e| anyhow!("save_published: {e}"))
}
