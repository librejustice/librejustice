//! Ministerial bulletins → every act a `text_publication` of its bulletin, the
//! doctrine and the regulatory acts also a `legal_text`, identity
//! `bo_<site>_<id>`, body from the PDF by `pdftotext`, Mistral OCR when scanned,
//! the PDF kept in the bucket, where a later read takes it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{anyhow, Result};
use chrono::NaiveDate;

use lj_bucket::{key, Bucket};
use lj_sources::bulletins::{self, BulletinClient, BulletinNotice, BulletinSite};
use lj_store::repository::{BodyWrite, DecisionRepository, Edition, LegalTextRow, PublicationRow};
use serde_json::Value;

use super::circulaires::body_from_text_layer;
use crate::config::Settings;
use crate::ocr::{self, Ocr};

/// `legal_article.source` of a bulletin norm's body.
const SOURCE: &str = "bulletin";

/// From this day publication on its ministry's bulletin makes a circular
/// applicable (art. R. 312-7 CRPA); an older one is a publication alone.
fn applicable_from() -> NaiveDate {
    NaiveDate::from_ymd_opt(2019, 1, 1).expect("valid date")
}

/// The ACPR's collections that set a rule or state its doctrine; its decisions, opinions and lists are publications.
const ACPR_NORMS: [&str; 6] = [
    "Instruction",
    "Recommandation",
    "Position",
    "Notice",
    "Ligne directrice",
    "Principe d'application sectoriel",
];

/// A scanned notice the OCR missed waits this long before a daily run asks
/// again; `--full` asks at once.
const OCR_RETRY_DAYS: i64 = 7;
const BULLETIN: Edition = Edition {
    language: "fr",
    status: "authentic",
};

#[derive(Debug, Default)]
struct Counts {
    listed: usize,
    todo: usize,
    held_elsewhere: usize,
    norms: usize,
    publications: usize,
    ocr: usize,
    ocr_missed: usize,
    ocr_later: usize,
    no_body: usize,
    failed: usize,
    filed: usize,
    file_failed: usize,
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
        (!settings.egress_proxy.is_empty()).then(|| settings.egress_proxy.clone()),
        settings.cache_dir(),
    )?;
    // The year before is read only while it is under 60 days old, the sites
    // listed by date only these 60 days.
    let since = |site: BulletinSite| {
        if full {
            NaiveDate::from_ymd_opt(site.first_year(), 1, 1).expect("valid date")
        } else {
            chrono::Utc::now().date_naive() - chrono::Days::new(60)
        }
    };
    let sites = site.map_or_else(|| BulletinSite::ALL.to_vec(), |s| vec![s]);
    if dry_run {
        for site in sites {
            dry_run_site(&client, site, since(site)).await?;
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
    let bucket = settings.bucket()?;
    let mut failed_sites = Vec::new();
    for &site in &sites {
        let dir = bulletins::site_dir(&settings.cache_dir(), site);
        match sync_site(
            &repo,
            &client,
            ocr.as_ref(),
            &bucket,
            &dir,
            site,
            since(site),
            full,
        )
        .await
        {
            Ok(c) => tracing::info!(
                site = site.slug(),
                listed = c.listed,
                todo = c.todo,
                held_elsewhere = c.held_elsewhere,
                norms = c.norms,
                publications = c.publications,
                ocr = c.ocr,
                ocr_missed = c.ocr_missed,
                ocr_later = c.ocr_later,
                no_body = c.no_body,
                failed = c.failed,
                filed = c.filed,
                file_failed = c.file_failed,
                "sync_bulletins site"
            ),
            Err(e) => {
                tracing::warn!(site = site.slug(), error = %e, "sync_bulletins site failed, read again next run");
                failed_sites.push(site.slug());
            }
        }
    }
    let slugged = super::slugs::assign_text_slugs(&repo).await?;
    tracing::info!(slugged, "sync_bulletins slugs");
    // One ministry's site down leaves the others' texts written; the step fails
    // only when no site answered.
    if failed_sites.len() == sites.len() {
        anyhow::bail!("sync_bulletins: {} failed", failed_sites.join(", "));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn sync_site(
    repo: &DecisionRepository<'_>,
    client: &BulletinClient,
    ocr: Option<&Ocr>,
    bucket: &Bucket,
    dir: &Path,
    site: BulletinSite,
    since: NaiveDate,
    full: bool,
) -> Result<Counts> {
    let mut published = bulletins::load_published(dir)?;
    let mut missed = bulletins::load_ocr_missed(dir)?;
    let mut filed = bulletins::load_filed(dir)?;
    let today = chrono::Utc::now().date_naive();
    let notices = client.list(site, since).await?;
    let (todo, later): (Vec<BulletinNotice>, Vec<BulletinNotice>) = notices
        .iter()
        .filter(|n| !published.contains(&n.native_id))
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
    let mut written = Vec::new();
    for mut n in todo {
        let uid = n.text_uid();
        if let Err(e) = client.enrich(&mut n).await {
            tracing::warn!(uid = %uid, error = %e, "bulletin: notice page failed, retried next run");
            c.failed += 1;
            continue;
        }
        let body = if n.legifrance_uid.is_some() {
            None
        } else {
            match fetch_body(client, ocr, bucket, dir, &n).await {
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
        if let Some(title) = body
            .as_deref()
            .and_then(|b| bulletins::complete_title(&n.title, b))
        {
            n.title = title;
        }
        let mut nature = norm_nature(&n);
        if nature.is_some() && held_elsewhere(repo, &n, &uid).await? {
            c.held_elsewhere += 1;
            nature = None;
        }
        if let Some(nature) = nature {
            repo.upsert_legal_text(&row(&n, nature))
                .await
                .map_err(|e| anyhow!("upsert_legal_text {uid}: {e}"))?;
            if let Some(body) = &body {
                repo.write_text_bodies(SOURCE, &[(uid.clone(), body.clone())], BodyWrite::Replace)
                    .await
                    .map_err(|e| anyhow!("write_text_bodies {uid}: {e}"))?;
            }
            c.norms += 1;
        }
        repo.upsert_publications(BULLETIN, &[publication(&n, body)])
            .await
            .map_err(|e| anyhow!("upsert_publications {uid}: {e}"))?;
        c.publications += 1;
        missed.remove(&n.native_id);
        published.insert(n.native_id.clone());
        if n.body_url.is_some() || n.body.is_none() {
            filed.insert(n.native_id.clone());
        } else {
            written.push(n);
        }
        if c.publications.is_multiple_of(25) {
            save_published(dir, &published).await?;
        }
    }
    save_published(dir, &published).await?;
    save_ocr_missed(dir, &missed).await?;

    // A daily run files what it wrote; `--full` also what was written before
    // the files were kept.
    let unfiled: Vec<BulletinNotice> = if full {
        notices
            .into_iter()
            .filter(|n| published.contains(&n.native_id) && !filed.contains(&n.native_id))
            .collect()
    } else {
        written
    };
    let mut issues = BTreeSet::new();
    for mut n in unfiled {
        match file(bucket, client, &mut n, &mut issues).await {
            Ok(()) => {
                filed.insert(n.native_id);
                c.filed += 1;
                if c.filed.is_multiple_of(25) {
                    save_filed(dir, &filed).await?;
                }
            }
            Err(e) => {
                tracing::warn!(uid = %n.text_uid(), error = %e, "bulletin: file not kept, retried next full run");
                c.file_failed += 1;
            }
        }
    }
    save_filed(dir, &filed).await?;
    Ok(c)
}

/// Keeps a written notice's PDF in the bucket: the issue it was cut from, else
/// its own document. A notice with neither, or whose file the site lost, has
/// nothing to keep.
async fn file(
    bucket: &Bucket,
    client: &BulletinClient,
    n: &mut BulletinNotice,
    issues: &mut BTreeSet<String>,
) -> Result<()> {
    let site = n.site.slug();
    if n.body.is_some() {
        let Some((issue, _)) = n.notice_url.split_once("#page=") else {
            return Ok(());
        };
        let key = key::bulletin_issue(site, xxhash_rust::xxh3::xxh3_64(issue.as_bytes()));
        if issues.insert(key.clone()) && !bucket.contains(&key).await? {
            if let Some(bytes) = client.get_bytes(issue).await? {
                put_pdf(bucket, &key, bytes).await?;
            }
        }
        return Ok(());
    }
    if n.body_url.is_none() && n.legifrance_uid.is_none() {
        client.enrich(n).await?;
    }
    let Some(url) = &n.body_url else {
        return Ok(());
    };
    let key = key::bulletin(site, &n.native_id);
    if !bucket.contains(&key).await? {
        if let Some(bytes) = client.get_bytes(url).await? {
            put_pdf(bucket, &key, bytes).await?;
        }
    }
    Ok(())
}

/// A body link that serves something other than a PDF keeps nothing.
async fn put_pdf(bucket: &Bucket, key: &str, bytes: Vec<u8>) -> Result<()> {
    if bytes.starts_with(b"%PDF") {
        bucket.put(key, bytes).await?;
    }
    Ok(())
}

/// The norms hold the doctrine applicable by its publication, and the arrêtés
/// and décisions that do not name their addressees.
fn norm_nature(n: &BulletinNotice) -> Option<&'static str> {
    if n.site == BulletinSite::Acpr {
        return ACPR_NORMS.contains(&n.kind.as_str()).then_some("ACPR");
    }
    match n.nature()? {
        "CIRCULAIRE"
            if n.published
                .or(n.signed)
                .is_none_or(|d| d >= applicable_from()) =>
        {
            Some("CIRCULAIRE")
        }
        "CIRCULAIRE" => None,
        nature @ ("ARRETE" | "DECISION")
            if !lj_norm::individual::is_individual_measure(&n.title) =>
        {
            Some(nature)
        }
        _ => None,
    }
}

/// The notice links to a DILA circular, or another row carries its NOR.
async fn held_elsewhere(
    repo: &DecisionRepository<'_>,
    n: &BulletinNotice,
    uid: &str,
) -> Result<bool> {
    if n.legifrance_uid.is_some() {
        return Ok(true);
    }
    let Some(nor) = &n.nor else {
        return Ok(false);
    };
    let held = repo
        .legal_text_uids_by_nor(std::slice::from_ref(nor))
        .await?;
    Ok(held.get(nor).is_some_and(|other| other != uid))
}

async fn dry_run_site(client: &BulletinClient, site: BulletinSite, since: NaiveDate) -> Result<()> {
    let notices = client.list(site, since).await?;
    let read: Vec<&BulletinNotice> = notices.iter().collect();
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

fn row(n: &BulletinNotice, nature: &str) -> LegalTextRow {
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
    bucket: &Bucket,
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
    let key = key::bulletin(n.site.slug(), &n.native_id);
    let bytes = match bucket.get(&key).await? {
        Some(bytes) => bytes.to_vec(),
        None => {
            let Some(bytes) = client.get_bytes(url).await? else {
                tracing::warn!(uid = %n.text_uid(), url = %url, "bulletin: document gone");
                return Ok(Body::Missing);
            };
            put_pdf(bucket, &key, bytes.clone()).await?;
            bytes
        }
    };
    // The agriculture site serves its instructions of 2011 as Word's HTML.
    if let Some(text) = std::str::from_utf8(&bytes)
        .ok()
        .filter(|s| s.trim_start().starts_with('<'))
        .and_then(|s| body_from_text_layer(&lj_sources::html_strip::strip_html(s)))
    {
        return Ok(Body::Text { text, ocr: false });
    }
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
    let answer = key::ocr(&key);
    if let Some(pages) = ocr::kept::<Vec<Value>>(bucket, &answer).await? {
        return Ok(Body::Text {
            text: lj_parse::clean_ocr_markdown(&ocr::text(&pages)),
            ocr: true,
        });
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
    match ocr
        .document_kept(bucket, &answer, &bytes, &n.text_uid())
        .await
    {
        Ok(reading) => Ok(Body::Text {
            text: lj_parse::clean_ocr_markdown(&reading.text()),
            ocr: true,
        }),
        Err(e) => {
            tracing::warn!(uid = %n.text_uid(), error = %e, "bulletin: scanned OCR fallback failed");
            Ok(Body::OcrMissed)
        }
    }
}

/// Rewrites the body of every bulletin notice whose OCR answer the bucket keeps, from that
/// answer: its publication's, and its norm's when it is one.
pub async fn rebuild_bulletins_ocr() -> Result<()> {
    let settings = Settings::from_env()?;
    let bucket = settings.bucket()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let (mut answers, mut publications, mut norms) = (0usize, 0usize, 0usize);
    for site in BulletinSite::ALL {
        let prefix = format!("bo/{}/", site.slug());
        for answer in bucket.keys(&prefix).await? {
            let Some(native_id) = answer
                .strip_prefix(&prefix)
                .and_then(|k| k.strip_suffix(".pdf.ocr.json.zst"))
            else {
                continue;
            };
            answers += 1;
            let pages: Vec<Value> = ocr::kept(&bucket, &answer)
                .await?
                .ok_or_else(|| anyhow!("{answer}: gone from the bucket"))?;
            let body = lj_parse::clean_ocr_markdown(&ocr::text(&pages));
            let uid = format!("bo_{}_{native_id}", site.slug());
            publications += usize::from(repo.set_publication_body(BULLETIN, &uid, &body).await?);
            norms += repo
                .write_text_bodies(SOURCE, &[(uid.clone(), body)], BodyWrite::Replace)
                .await
                .map_err(|e| anyhow!("write_text_bodies {uid}: {e}"))?
                .len();
        }
    }
    tracing::info!(
        answers,
        publications,
        norms,
        "bulletins: bodies rebuilt from the OCR answers"
    );
    Ok(())
}

/// Empties the pages of the bulletins' kept answers their PDF leaves blank, redrawing only the
/// [`ocr::suspect`] ones, and rewrites the bodies from them.
pub async fn blank_bulletins_ocr(dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let bucket = settings.bucket()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let mut tally = ocr::Emptied::default();
    for site in BulletinSite::ALL {
        let prefix = format!("bo/{}/", site.slug());
        let answers: BTreeSet<String> = bucket
            .keys(&prefix)
            .await?
            .into_iter()
            .filter_map(|k| match k.split_once(".pdf.ocr/") {
                Some((pdf, _)) => Some(format!("{pdf}.pdf.ocr.json.zst")),
                None => k.ends_with(".pdf.ocr.json.zst").then_some(k),
            })
            .collect();
        for answer in answers {
            let Some(native_id) = answer
                .strip_prefix(&prefix)
                .and_then(|k| k.strip_suffix(".pdf.ocr.json.zst"))
            else {
                continue;
            };
            tally.docs_read += 1;
            let mut kept = ocr::kept_answer(&bucket, &answer).await?;
            let suspects: Vec<u32> = kept
                .0
                .iter()
                .filter(|(_, page)| ocr::suspect(&ocr::markdown(page)))
                .map(|(&n, _)| n)
                .collect();
            if suspects.is_empty() {
                continue;
            }
            let file = key::bulletin(site.slug(), native_id);
            let Some(blank) = ocr::redraw(&bucket, &file, &suspects, &mut tally).await? else {
                continue;
            };
            if blank.is_empty() {
                continue;
            }
            for n in &blank {
                let page = kept
                    .0
                    .get_mut(n)
                    .ok_or_else(|| anyhow!("{answer}: page {n} not kept"))?;
                tally.empty(&ocr::markdown(page));
                *page = ocr::emptied(page);
            }
            tally.docs += 1;
            if dry_run {
                continue;
            }
            ocr::keep_answer(&bucket, &answer, &kept, &blank).await?;
            let pages: Vec<Value> = kept.0.into_values().collect();
            let body = lj_parse::clean_ocr_markdown(&ocr::text(&pages));
            let uid = format!("bo_{}_{native_id}", site.slug());
            repo.set_publication_body(BULLETIN, &uid, &body).await?;
            repo.write_text_bodies(SOURCE, &[(uid.clone(), body)], BodyWrite::Replace)
                .await
                .map_err(|e| anyhow!("write_text_bodies {uid}: {e}"))?;
        }
    }
    tally.log("bulletins", dry_run);
    Ok(())
}

async fn save_ocr_missed(dir: &Path, missed: &BTreeMap<String, NaiveDate>) -> Result<()> {
    let (dir, missed) = (dir.to_path_buf(), missed.clone());
    tokio::task::spawn_blocking(move || bulletins::save_ocr_missed(&dir, &missed))
        .await
        .map_err(|e| anyhow!("ocr_missed task: {e}"))?
        .map_err(|e| anyhow!("save_ocr_missed: {e}"))
}

async fn save_filed(dir: &Path, filed: &BTreeSet<String>) -> Result<()> {
    let (dir, filed) = (dir.to_path_buf(), filed.clone());
    tokio::task::spawn_blocking(move || bulletins::save_filed(&dir, &filed))
        .await
        .map_err(|e| anyhow!("filed task: {e}"))?
        .map_err(|e| anyhow!("save_filed: {e}"))
}

async fn save_published(dir: &Path, published: &BTreeSet<String>) -> Result<()> {
    let (dir, published) = (dir.to_path_buf(), published.clone());
    tokio::task::spawn_blocking(move || bulletins::save_published(&dir, &published))
        .await
        .map_err(|e| anyhow!("published task: {e}"))?
        .map_err(|e| anyhow!("save_published: {e}"))
}
