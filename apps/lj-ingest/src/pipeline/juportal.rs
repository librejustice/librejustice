//! Juportal: the French decisions of the Belgian courts, each page fetched once under
//! `<state_dir>/ingest/juportal/pages/{court}/` and loaded by the standard upsert path. The stock is
//! listed by the ECLI search engine year by year and by the portal's feeds since [`FEEDS_FROM`]; a
//! daily run reads the feeds since its last run.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use chrono::{Days, NaiveDate, Utc};
use tokio::signal::unix::{signal, Signal, SignalKind};

use lj_core::chamber::Seat;
use lj_parse::{
    ecli_court_and_kind, juportal_names, juportal_page_lang, juportal_taken, parse_juportal,
    BELGIAN_PLACE,
};
use lj_pg::db::Connection;
use lj_sources::error::SourceError;
use lj_sources::juportal::{EcliClient, JuportalClient, ECLI_PAGE, FEED_LIMIT};
use lj_store::repository::{DecisionRepository, ExtractedFields, JurisdictionRow};

use crate::config::Settings;

use super::batch::drain_batch;
use super::europe::europe_pool;
use super::{content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode, WriteMode};

const BATCH: usize = 100;
/// The daily run starts no batch past this, the pages left waiting for the next run.
const BUDGET: Duration = Duration::from_secs(1_200);
/// The ECLI search engine holds 695 French decisions of 2024 and 37 of 2025.
const FEEDS_FROM: NaiveDate = NaiveDate::from_ymd_opt(2024, 1, 1).expect("a day");
/// A daily run with no cursor reads the feeds of the 30 days before.
const FIRST_WINDOW: Days = Days::new(30);
/// The most days a feed is asked for: 30 days answer in under a second, and a single day of 2024
/// lists 733 French items.
const FEED_SPAN: Days = Days::new(31);
/// A feed range that ended this long before the run is kept under `feeds/` and not asked again.
const FEED_SETTLED: Days = Days::new(7);
/// The pages in a row whose transport fails that stop a stock run, the portal taken for down.
const FAILED_IN_A_ROW: usize = 3;

fn dir(settings: &Settings) -> PathBuf {
    settings.state_dir.join("ingest/juportal")
}

/// The language a page is asked in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lang {
    French,
    Dutch,
}

impl Lang {
    fn code(self) -> &'static str {
        match self {
            Lang::French => "FR",
            Lang::Dutch => "NL",
        }
    }

    /// The extension of its page under `pages/`, then of the mark of an ECLI the portal knows not.
    fn kinds(self) -> [&'static str; 2] {
        match self {
            Lang::French => ["html", "gone"],
            Lang::Dutch => ["nl.html", "nl.gone"],
        }
    }
}

fn page_path(dir: &Path, ecli: &str, kind: &str) -> PathBuf {
    let court = ecli_court_and_kind(ecli).map_or("other", |(court, _)| court);
    dir.join("pages").join(court).join(format!("{ecli}.{kind}"))
}

/// A page fetched in `lang`, or found gone.
fn visited(dir: &Path, ecli: &str, lang: Lang) -> bool {
    lang.kinds()
        .iter()
        .any(|kind| page_path(dir, ecli, kind).exists())
}

/// The page asked in French that the portal served in Dutch, the decision being held in Dutch alone.
fn cached_dutch(dir: &Path, ecli: &str) -> Result<Option<String>> {
    match std::fs::read_to_string(page_path(dir, ecli, "html")) {
        Ok(html) => Ok((juportal_page_lang(&html) == Some("nld")).then_some(html)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::create_dir_all(path.parent().expect("a page sits in a folder"))?;
    let part = path.with_extension("part");
    std::fs::write(&part, bytes)?;
    std::fs::rename(&part, path)?;
    Ok(())
}

/// The ECLIs the portal published in `lang` from `from` to `to`, asked [`FEED_SPAN`] days at most
/// at a time, a range whose feed reaches its limit read again in halves.
async fn feeds(
    client: &JuportalClient,
    dir: &Path,
    from: NaiveDate,
    to: NaiveDate,
    lang: Lang,
) -> Result<Vec<String>> {
    let cache = dir.join("feeds");
    let settled = to - FEED_SETTLED;
    let mut eclis = Vec::new();
    let mut ranges = Vec::new();
    let mut start = from;
    while start <= to {
        let end = (start + FEED_SPAN - Days::new(1)).min(to);
        ranges.push((start, end));
        start = end.succ_opt().expect("a day after the range");
    }
    while let Some((from, to)) = ranges.pop() {
        let path = match lang {
            Lang::French => cache.join(format!("{from}_{to}.txt")),
            Lang::Dutch => cache.join(format!("{from}_{to}.nl.txt")),
        };
        if to < settled {
            match std::fs::read_to_string(&path) {
                Ok(listed) => {
                    eclis.extend(listed.lines().map(str::to_string));
                    continue;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        let listed = client.feed(from, to, lang.code()).await?;
        if listed.len() < FEED_LIMIT {
            if to < settled {
                write_file(&path, listed.join("\n").as_bytes())?;
            }
            eclis.extend(listed);
            continue;
        }
        if from == to {
            bail!("juportal: the feed of {from} reaches its {FEED_LIMIT} items");
        }
        let mid = from + Days::new(((to - from).num_days() / 2) as u64);
        ranges.push((from, mid));
        ranges.push((mid.succ_opt().expect("a day before `to`"), to));
    }
    Ok(eclis)
}

/// Belgium's decisions the ECLI search engine lists, year by year: those held in French, from the
/// French listing kept under `ecli/{year}.txt`, then every one the Dutch listing holds, kept under
/// `ecli/{year}.nl.txt`, with whether it is held in French too. Each year's list is kept once read
/// whole.
async fn ecli_stock(
    client: &EcliClient,
    dir: &Path,
) -> Result<(Vec<String>, Vec<(String, Option<bool>)>)> {
    let cache = dir.join("ecli");
    std::fs::create_dir_all(&cache)?;
    let years = match client.years().await {
        Ok(years) => years,
        Err(e) => {
            let cached = cached_years(&cache)?;
            if cached.is_empty() {
                return Err(e.into());
            }
            tracing::warn!(error = %e, years = cached.len(), "juportal: the ECLI facets failed, the cached years are read");
            cached.into_iter().map(|year| (year, 0)).collect()
        }
    };
    let (mut eclis, mut dutch) = (Vec::new(), Vec::new());
    for (year, count) in years {
        let path = cache.join(format!("{year}.txt"));
        match std::fs::read_to_string(&path) {
            Ok(listed) => eclis.extend(listed.lines().map(str::to_string)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let listed: Vec<String> = ecli_year(client, year, "FR", count)
                    .await?
                    .into_iter()
                    .filter(|(_, languages)| held_in_french(languages))
                    .map(|(ecli, _)| ecli)
                    .collect();
                write_file(&path, listed.join("\n").as_bytes())?;
                eclis.extend(listed);
            }
            Err(e) => return Err(e.into()),
        }
        let path = cache.join(format!("{year}.nl.txt"));
        let listed = match std::fs::read_to_string(&path) {
            Ok(listed) => listed
                .lines()
                .map(|line| {
                    let (ecli, languages) = line.split_once('\t').unwrap_or((line, ""));
                    let languages = languages.split(',').map(str::to_string).collect();
                    (ecli.to_string(), languages)
                })
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let listed = ecli_year(client, year, "NL", count).await?;
                let lines: Vec<String> = listed
                    .iter()
                    .map(|(ecli, languages)| format!("{ecli}\t{}", languages.join(",")))
                    .collect();
                write_file(&path, lines.join("\n").as_bytes())?;
                listed
            }
            Err(e) => return Err(e.into()),
        };
        for (ecli, languages) in listed {
            let french = held_in_french(&languages);
            if french {
                eclis.push(ecli.clone());
            }
            dutch.push((ecli, Some(french)));
        }
    }
    Ok((eclis, dutch))
}

fn held_in_french(languages: &[String]) -> bool {
    languages.iter().any(|l| l == "fr")
}

/// Every decision of `year` the search engine lists under `language`, with its languages.
async fn ecli_year(
    client: &EcliClient,
    year: i32,
    language: &str,
    count: usize,
) -> Result<Vec<(String, Vec<String>)>> {
    let (mut listed, mut total) = (Vec::new(), 0usize);
    for number in 1.. {
        let page = client.page(year, language, number).await?;
        total = page.total;
        listed.extend(page.items);
        if listed.len() >= total || number * ECLI_PAGE >= total {
            break;
        }
    }
    if listed.len() < total {
        tracing::warn!(
            year,
            language,
            read = listed.len(),
            total,
            "juportal: an ECLI year lists fewer decisions than it counts"
        );
    }
    let french = listed
        .iter()
        .filter(|(_, languages)| held_in_french(languages))
        .count();
    tracing::info!(
        year,
        language,
        count,
        total,
        french,
        "juportal: ECLI year listed"
    );
    Ok(listed)
}

/// The years whose ECLI list `cache` holds.
fn cached_years(cache: &Path) -> Result<Vec<i32>> {
    let mut years = Vec::new();
    for entry in std::fs::read_dir(cache)? {
        let path = entry?.path();
        if path.extension().is_some_and(|x| x == "txt") {
            if let Some(year) = path.file_stem().and_then(|s| s.to_str()?.parse().ok()) {
                years.push(year);
            }
        }
    }
    years.sort_unstable();
    Ok(years)
}

/// The day of the last run that fetched every page its feeds listed, read again by the next.
fn cursor_path(dir: &Path) -> PathBuf {
    dir.join("feed-cursor")
}

fn read_cursor(dir: &Path) -> Result<Option<NaiveDate>> {
    match std::fs::read_to_string(cursor_path(dir)) {
        Ok(day) => Ok(Some(NaiveDate::parse_from_str(day.trim(), "%Y-%m-%d")?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[derive(Default)]
struct Loader {
    counts: IngestCounts,
    refused: usize,
}

impl Loader {
    fn candidate(html: &str) -> Result<Candidate> {
        let parsed = parse_juportal(html)?;
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
                place: Some(BELGIAN_PLACE.to_string()),
            }),
            ..Default::default()
        };
        let checksummed = [
            serde_json::to_vec(&parsed.source_fields)?,
            parsed.decision.texte_integral_clean.as_bytes().to_vec(),
        ]
        .concat();
        Ok(Candidate {
            decision_id: None,
            public_id: generate_public_id(),
            content_checksum: content_checksum(&checksummed),
            prebuilt_source_fields: Some(parsed.source_fields),
            prebuilt_extracted: Some(extracted),
            decision: parsed.decision,
            raw_payload: Vec::new(),
            payload_format: "html".to_string(),
            write_mode: WriteMode::Full,
            passages: true,
            distinct: false,
        })
    }

    /// Loads the pages a batch fetched, each under the ECLI it states: two ECLIs a newer one
    /// replaced give one decision.
    async fn load(&mut self, conn: &Connection, pages: &[(String, Lang, String)]) -> Result<()> {
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();
        for (ecli, _, html) in pages {
            match Self::candidate(html) {
                Ok(c) => {
                    if seen.insert(c.decision.source_uid.clone()) {
                        candidates.push(c);
                    }
                }
                Err(e) => {
                    tracing::debug!(ecli = %ecli, error = %e, "juportal page refused");
                    self.refused += 1;
                }
            }
        }
        if !candidates.is_empty() {
            drain_batch(conn, candidates, IngestMode::MissingHash, &mut self.counts).await?;
        }
        Ok(())
    }
}

/// The pages to fetch: the stock the ECLI search engine and the feeds since [`FEEDS_FROM`] list
/// when `full`, else the feeds since the last run, in French and in Dutch. Only those of a court and
/// kind the load takes and not visited yet, in this order: the Dutch pages the French walk already
/// holds, the French pages, the Dutch pages of the decisions held in Dutch alone, then those of
/// the decisions held in French too.
async fn wanted(
    settings: &Settings,
    client: &JuportalClient,
    dir: &Path,
    full: bool,
    today: NaiveDate,
) -> Result<Vec<(String, Lang)>> {
    let (french, dutch) = if full {
        let ecli = EcliClient::new(&settings.egress_proxy).await?;
        let (mut french, mut dutch) = ecli_stock(&ecli, dir).await?;
        match feeds(client, dir, FEEDS_FROM, today, Lang::French).await {
            Ok(fed) => french.extend(fed),
            Err(e) => {
                tracing::warn!(error = %e, "juportal: the French feeds failed, the stock walks without them");
            }
        }
        match feeds(client, dir, FEEDS_FROM, today, Lang::Dutch).await {
            Ok(fed) => dutch.extend(fed.into_iter().map(|ecli| (ecli, None))),
            Err(e) => {
                tracing::warn!(error = %e, "juportal: the Dutch feeds failed, the stock walks without them");
            }
        }
        (french, dutch)
    } else {
        let from = match read_cursor(dir)? {
            Some(day) => day,
            None => {
                tracing::info!(
                    "juportal sync: no cursor yet, the feeds of the 30 days before are read"
                );
                today - FIRST_WINDOW
            }
        };
        let french = feeds(client, dir, from, today, Lang::French).await?;
        let dutch = feeds(client, dir, from, today, Lang::Dutch)
            .await?
            .into_iter()
            .map(|ecli| (ecli, None))
            .collect();
        (french, dutch)
    };
    let mut others: BTreeMap<String, usize> = BTreeMap::new();
    let mut taken = |ecli: &str| {
        let taken = juportal_taken(ecli);
        if !taken {
            if let Some((court, kind)) = ecli_court_and_kind(ecli) {
                *others.entry(format!("{court}:{kind}")).or_default() += 1;
            }
        }
        taken
    };
    let held_in_french: HashSet<&str> = french.iter().map(String::as_str).collect();
    let mut stages: BTreeMap<(String, &'static str), (u8, Lang)> = BTreeMap::new();
    for ecli in &french {
        if taken(ecli) && !visited(dir, ecli, Lang::French) {
            stages.insert((ecli.clone(), "FR"), (1, Lang::French));
        }
    }
    for (ecli, french_too) in &dutch {
        if !taken(ecli) || visited(dir, ecli, Lang::Dutch) {
            continue;
        }
        let stage = match french_too {
            Some(false) => 2,
            Some(true) => 3,
            None if cached_dutch(dir, ecli)?.is_some() => 0,
            None if held_in_french.contains(ecli.as_str()) => 3,
            None => 2,
        };
        stages
            .entry((ecli.clone(), "NL"))
            .and_modify(|(s, _)| *s = (*s).min(stage))
            .or_insert((stage, Lang::Dutch));
    }
    if !others.is_empty() {
        tracing::info!(?others, "juportal sync: ECLIs of a court or kind not taken");
    }
    let mut wanted: Vec<(u8, String, Lang)> = stages
        .into_iter()
        .map(|((ecli, _), (stage, lang))| (stage, ecli, lang))
        .collect();
    wanted.sort_by_cached_key(|(stage, ecli, _)| (*stage, walk_order(ecli)));
    Ok(wanted
        .into_iter()
        .map(|(_, ecli, lang)| (ecli, lang))
        .collect())
}

/// The newest year first, and within a year the three highest courts before the others: the
/// portal answers a page in 13 to 23 s, so a stock walk lasts weeks.
fn walk_order(ecli: &str) -> (Reverse<String>, bool, String) {
    let mut parts = ecli.split(':').skip(2);
    let court = parts.next().unwrap_or_default();
    let year = parts.next().unwrap_or_default();
    (
        Reverse(year.to_string()),
        !matches!(court, "CASS" | "GHCC" | "RVSCE"),
        ecli.to_string(),
    )
}

/// Fetches the pages the feeds listed since the last run, then loads them. `full` lists the
/// stock instead and spends no time budget; `limit` stops after that many pages; `dry_run`
/// fetches and parses them, prints a line for each, and keeps no page.
pub async fn sync_juportal(full: bool, limit: Option<usize>, dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = dir(&settings);
    let client = JuportalClient::new(&settings.egress_proxy)?;
    let today = Utc::now().date_naive();
    if dry_run {
        return dry(&settings, &client, &dir, full, today, limit.unwrap_or(20)).await;
    }
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    if !DecisionRepository::new(&conn).juportal_lock().await? {
        tracing::info!("juportal sync: another run holds the lock");
        return Ok(());
    }
    let named = DecisionRepository::new(&conn)
        .write_jurisdiction_names(&juportal_names())
        .await?;
    tracing::info!(named, "juportal sync: court names written");
    let wanted = wanted(&settings, &client, &dir, full, today).await?;
    let wanted = &wanted[..limit.unwrap_or(wanted.len()).min(wanted.len())];
    tracing::info!(full, wanted = wanted.len(), dir = %dir.display(), "juportal sync: pages to fetch");

    let mut loader = Loader::default();
    let started = Instant::now();
    let (mut fetched, mut gone, mut left) = (0usize, 0usize, 0usize);
    let mut failed_in_a_row = 0;
    let mut stopped = None;
    let mut term = full.then(|| signal(SignalKind::terminate())).transpose()?;
    let mut terminated = false;
    for chunk in wanted.chunks(BATCH) {
        if !full && started.elapsed() > BUDGET {
            break;
        }
        let mut pages = Vec::with_capacity(chunk.len());
        let mut missing = Vec::new();
        for (ecli, lang) in chunk {
            let cached = match lang {
                Lang::Dutch => cached_dutch(&dir, ecli)?,
                Lang::French => None,
            };
            let answer = match cached {
                Some(html) => Ok(Some(html)),
                None => tokio::select! {
                    answer = client.page(ecli, lang.code()) => answer,
                    () = received(term.as_mut()) => {
                        terminated = true;
                        break;
                    }
                },
            };
            match answer {
                Ok(Some(html)) => {
                    failed_in_a_row = 0;
                    pages.push((ecli.clone(), *lang, html));
                }
                Ok(None) => {
                    failed_in_a_row = 0;
                    missing.push((ecli, *lang));
                }
                Err(SourceError::Http(e)) if full && failed_in_a_row + 1 < FAILED_IN_A_ROW => {
                    failed_in_a_row += 1;
                    left += 1;
                    tracing::warn!(%ecli, error = %e, "juportal: a page that does not answer is left for the next run");
                }
                Err(e) => {
                    stopped = Some(e);
                    break;
                }
            }
        }
        loader.load(&conn, &pages).await?;
        for (ecli, lang, html) in &pages {
            write_file(&page_path(&dir, ecli, lang.kinds()[0]), html.as_bytes())?;
            if *lang == Lang::French && juportal_page_lang(html) == Some("nld") {
                write_file(
                    &page_path(&dir, ecli, Lang::Dutch.kinds()[0]),
                    html.as_bytes(),
                )?;
            }
        }
        for (ecli, lang) in &missing {
            write_file(&page_path(&dir, ecli, lang.kinds()[1]), b"")?;
        }
        fetched += pages.len() + missing.len();
        gone += missing.len();
        tracing::info!(fetched, gone, of = wanted.len(), "juportal sync");
        if stopped.is_some() || terminated {
            break;
        }
    }
    tracing::warn!(
        fetched,
        gone,
        left,
        refused = loader.refused,
        created = loader.counts.created,
        updated = loader.counts.updated,
        skipped = loader.counts.skipped,
        errors = loader.counts.errors,
        "juportal sync done"
    );
    if let Some(e) = stopped {
        bail!("juportal sync stopped, the next run resumes: {e}");
    }
    if terminated {
        bail!("juportal sync stopped by SIGTERM, the next run resumes");
    }
    if fetched < wanted.len() {
        tracing::warn!(
            left = wanted.len() - fetched,
            "juportal sync: the pages left wait for the next run"
        );
    } else if limit.is_none() {
        write_file(&cursor_path(&dir), today.to_string().as_bytes())?;
    }
    Ok(())
}

/// Resolves on the SIGTERM `term` listens for, never without one.
async fn received(term: Option<&mut Signal>) {
    match term {
        Some(term) => {
            term.recv().await;
        }
        None => std::future::pending().await,
    }
}

/// Lists, fetches `limit` pages and parses them, a line on stdout for each: no database, no page
/// kept.
async fn dry(
    settings: &Settings,
    client: &JuportalClient,
    dir: &Path,
    full: bool,
    today: NaiveDate,
    limit: usize,
) -> Result<()> {
    let wanted = wanted(settings, client, dir, full, today).await?;
    tracing::info!(
        full,
        wanted = wanted.len(),
        "juportal dry run: pages listed"
    );
    for (ecli, lang) in wanted.iter().take(limit) {
        let Some(html) = client.page(ecli, lang.code()).await? else {
            println!("{ecli}\tgone");
            continue;
        };
        match parse_juportal(&html) {
            Ok(p) => println!(
                "{ecli}\t{}\t{}\t{}\t{}\t{} chars\t{} summaries",
                p.decision.ecli.as_deref().unwrap_or_default(),
                p.court.code,
                p.decision.date_lecture.as_deref().unwrap_or_default(),
                p.decision.numero_dossiers.unwrap_or_default().join(" | "),
                p.decision.texte_integral_clean.chars().count(),
                p.source_fields["summaries"].as_array().map_or(0, Vec::len),
            ),
            Err(e) => println!("{ecli}\trefused\t{e}"),
        }
    }
    Ok(())
}
