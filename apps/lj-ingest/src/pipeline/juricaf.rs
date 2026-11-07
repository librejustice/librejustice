//! JuriCAF: the decision pages of the fonds no primary source serves, fetched under
//! `<state_dir>/ingest/juricaf-probe/html/` and loaded by the standard upsert path, each court
//! placed and typed from the URN:LEX of its page.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use chrono::NaiveDate;

use lj_core::chamber::Seat;
use lj_parse::{juricaf_country, parse_juricaf};
use lj_pg::db::Connection;
use lj_sources::juricaf::{JuricafClient, SitemapEntry};
use lj_store::repository::{DecisionRepository, ExtractedFields, JurisdictionRow};

use crate::config::Settings;

use super::batch::drain_batch;
use super::europe::europe_pool;
use super::{content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode, WriteMode};

/// The countries and courts whose primary source serves their decisions, by the head of their
/// slugs: the sync takes every other one.
const SERVED_ELSEWHERE: [&str; 8] = [
    "FRANCE",
    "SUISSE",
    "LUXEMBOURG",
    "BELGIQUE",
    "MONACO",
    "CANADA",
    "CJUE",
    "CONSEILDELEUROPE",
];
const BATCH: usize = 100;
/// The daily sync starts no batch past this, the pages left waiting for the next run.
const BUDGET: Duration = Duration::from_secs(1_200);

fn pages_dir(settings: &Settings) -> PathBuf {
    settings.state_dir.join("ingest/juricaf-probe/html")
}

/// The `Last-Modified` each sitemap gave at its last read whose pages were all fetched.
fn sitemaps_seen(settings: &Settings) -> PathBuf {
    settings
        .state_dir
        .join("ingest/juricaf-probe/sitemaps.json")
}

fn head(slug: &str) -> &str {
    slug.split('-').next().unwrap_or(slug)
}

fn slug_of(path: &Path) -> Result<String> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("unreadable file name: {}", path.display()))
}

fn html_pages(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| anyhow!("read {}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "html"))
        .collect();
    paths.sort();
    Ok(paths)
}

/// Loads every `*.html` page of `dir`, by default the folder `sync-juricaf` fills. `source_uid`
/// `juricaf/{slug}`, checksum of the raw bytes.
pub async fn juricaf_probe(dir: Option<PathBuf>, mode: IngestMode) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = dir.unwrap_or_else(|| pages_dir(&settings));
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let paths = html_pages(&dir)?;
    let mut loader = Loader::default();
    for chunk in paths.chunks(BATCH) {
        let pages = chunk
            .iter()
            .map(|p| Ok((slug_of(p)?, std::fs::read(p)?)))
            .collect::<Result<Vec<_>>>()?;
        loader.load(&conn, &pages, mode).await?;
    }
    loader.report(paths.len());
    Ok(())
}

#[derive(Default)]
struct Loader {
    places: HashMap<String, bool>,
    counts: IngestCounts,
}

impl Loader {
    async fn load(
        &mut self,
        conn: &Connection,
        pages: &[(String, Vec<u8>)],
        mode: IngestMode,
    ) -> Result<()> {
        let repo = DecisionRepository::new(conn);
        let mut candidates = Vec::new();
        for (slug, raw) in pages {
            let parsed = match parse_juricaf(slug, raw) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(slug = %slug, error = %e, "juricaf page unparsed, skip");
                    self.counts.skipped += 1;
                    continue;
                }
            };
            let mut extracted = ExtractedFields {
                date_lecture: parsed
                    .decision
                    .date_lecture
                    .as_deref()
                    .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()),
                docket_numbers: parsed.decision.numero_dossiers.clone().unwrap_or_default(),
                ..Default::default()
            };
            let place = format!(
                "geo:{}",
                lj_vocab::courts::foreign_order(&parsed.jurisdiction_code)
            );
            if !self.places.contains_key(&place) {
                let known = !repo.place_path(&place).await?.is_empty();
                self.places.insert(place.clone(), known);
            }
            if !self.places[&place] {
                tracing::warn!(slug = %slug, place = %place, "juricaf court in a place the geo vocabulary lacks, skip");
                self.counts.skipped += 1;
                continue;
            }
            let jurisdiction_type = parsed
                .decision
                .jurisdiction_type
                .clone()
                .expect("parse_juricaf always sets the type");
            let seat = Seat {
                jurisdiction_type: jurisdiction_type.clone(),
                jurisdiction_code: Some(parsed.jurisdiction_code.clone()),
                ..Default::default()
            };
            extracted.seat_paths = seat.paths();
            extracted.seat = Some(seat);
            extracted.jurisdiction = Some(JurisdictionRow {
                code: parsed.jurisdiction_code.clone(),
                source_code: parsed.jurisdiction_code.clone(),
                jurisdiction_type,
                city: None,
                label: parsed.jurisdiction_label.clone(),
                place: Some(place),
            });
            candidates.push(Candidate {
                decision_id: None,
                public_id: generate_public_id(),
                content_checksum: content_checksum(raw),
                prebuilt_source_fields: Some(parsed.source_fields),
                prebuilt_extracted: Some(extracted),
                decision: parsed.decision,
                raw_payload: Vec::new(),
                payload_format: "html".to_string(),
                write_mode: WriteMode::Full,
                passages: true,
                distinct: false,
            });
        }
        if !candidates.is_empty() {
            drain_batch(conn, candidates, mode, &mut self.counts).await?;
        }
        Ok(())
    }

    fn report(&self, pages: usize) {
        tracing::info!(
            pages,
            created = self.counts.created,
            updated = self.counts.updated,
            skipped = self.counts.skipped,
            errors = self.counts.errors,
            dropped = self.counts.passages_dropped,
            "juricaf load done"
        );
    }
}

/// A page the folder lacks and no earlier run found gone, or older than the day its sitemap says it
/// was modified.
fn stale(dir: &Path, entry: &SitemapEntry) -> Result<bool> {
    let fetched = match std::fs::metadata(dir.join(format!("{}.html", entry.slug))) {
        Ok(meta) => chrono::DateTime::<chrono::Utc>::from(meta.modified()?).date_naive(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(!visited(dir, &entry.slug))
        }
        Err(e) => return Err(e.into()),
    };
    Ok(entry
        .lastmod
        .as_deref()
        .and_then(|d| NaiveDate::parse_from_str(d.get(..10)?, "%Y-%m-%d").ok())
        .is_some_and(|modified| modified > fetched))
}

/// A page fetched, or found gone: a 404 leaves an empty `{slug}.gone` beside the pages.
fn visited(dir: &Path, slug: &str) -> bool {
    dir.join(format!("{slug}.html")).exists() || dir.join(format!("{slug}.gone")).exists()
}

/// The countries of the folder, as their pages name them, one page read per slug head.
fn countries(dir: &Path) -> Result<BTreeSet<String>> {
    let mut first: BTreeMap<String, PathBuf> = BTreeMap::new();
    for path in html_pages(dir)? {
        let slug = slug_of(&path)?;
        first.entry(head(&slug).to_string()).or_insert(path);
    }
    let mut countries = BTreeSet::new();
    for path in first.values() {
        countries.extend(juricaf_country(&std::fs::read(path)?));
    }
    Ok(countries)
}

/// Fetches the pages that appeared or changed since the last run, then loads them: every page
/// newer than its copy in the sitemaps modified since their last read, and the latest decisions of
/// each country of the folder. `full` reads every sitemap whole and spends no budget; `prefixes`
/// takes only the slugs opening with one; `list` counts the slugs by country and court.
pub async fn sync_juricaf(prefixes: &[String], full: bool, list: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = pages_dir(&settings);
    let client = JuricafClient::new()?;
    if list {
        let mut courts: BTreeMap<String, usize> = BTreeMap::new();
        for url in client.sitemaps().await? {
            let entries = client.sitemap(&url, None).await?.map(|s| s.entries);
            for entry in entries.unwrap_or_default() {
                let court: Vec<&str> = entry.slug.splitn(3, '-').take(2).collect();
                *courts.entry(court.join("-")).or_default() += 1;
            }
        }
        for (court, n) in courts {
            println!("{n}\t{court}");
        }
        return Ok(());
    }
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    if !DecisionRepository::new(&conn).juricaf_lock().await? {
        tracing::info!("juricaf sync: another run holds the lock");
        return Ok(());
    }
    std::fs::create_dir_all(&dir)?;
    let taken = |slug: &str| match prefixes {
        [] => !SERVED_ELSEWHERE.contains(&head(slug)),
        given => given.iter().any(|p| slug.starts_with(p.as_str())),
    };
    let seen_path = sitemaps_seen(&settings);
    let mut seen: BTreeMap<String, String> = match std::fs::read(&seen_path) {
        Ok(raw) => serde_json::from_slice(&raw)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => return Err(e.into()),
    };

    let mut wanted: BTreeSet<String> = BTreeSet::new();
    let mut read: BTreeMap<String, String> = BTreeMap::new();
    for url in client.sitemaps().await? {
        let since = seen.get(&url).filter(|_| !full).map(String::as_str);
        let Some(sitemap) = client.sitemap(&url, since).await? else {
            continue;
        };
        for entry in sitemap.entries.iter().filter(|e| taken(&e.slug)) {
            if stale(&dir, entry)? {
                wanted.insert(entry.slug.clone());
            }
        }
        read.extend(sitemap.last_modified.map(|m| (url, m)));
    }
    let mut from_feeds = 0usize;
    for country in countries(&dir)? {
        for slug in client.feed(&country).await? {
            if taken(&slug) && !visited(&dir, &slug) && wanted.insert(slug) {
                from_feeds += 1;
            }
        }
    }
    let wanted: Vec<String> = wanted.into_iter().collect();
    tracing::info!(sitemaps_read = read.len(), wanted = wanted.len(), from_feeds, dir = %dir.display(), "juricaf sync: pages to fetch");

    let mut loader = Loader::default();
    let started = Instant::now();
    let (mut fetched, mut gone) = (0usize, 0usize);
    for chunk in wanted.chunks(BATCH) {
        if !full && started.elapsed() > BUDGET {
            break;
        }
        let mut pages = Vec::with_capacity(chunk.len());
        let mut missing = Vec::new();
        for slug in chunk {
            match client.page(slug).await? {
                Some(page) => pages.push((slug.clone(), page.into_bytes())),
                None => missing.push(slug),
            }
        }
        loader.load(&conn, &pages, IngestMode::MissingHash).await?;
        for (slug, raw) in &pages {
            let part = dir.join(format!("{slug}.part"));
            std::fs::write(&part, raw)?;
            std::fs::rename(&part, dir.join(format!("{slug}.html")))?;
        }
        for slug in &missing {
            std::fs::write(dir.join(format!("{slug}.gone")), b"")?;
        }
        fetched += chunk.len();
        gone += missing.len();
        tracing::info!(fetched, gone, of = wanted.len(), "juricaf sync");
    }
    if fetched < wanted.len() {
        tracing::warn!(
            left = wanted.len() - fetched,
            "juricaf sync: budget spent, the pages left wait for the next run"
        );
    } else if prefixes.is_empty() {
        seen.extend(read);
        std::fs::write(&seen_path, serde_json::to_vec_pretty(&seen)?)?;
    }
    if gone > 0 {
        tracing::warn!(gone, "juricaf sync: pages a sitemap names answer 404");
    }
    loader.report(fetched - gone);
    Ok(())
}
