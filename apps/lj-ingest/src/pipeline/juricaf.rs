//! JuriCAF: the decision pages of the fonds no primary source serves, fetched under
//! `<state_dir>/ingest/juricaf-probe/html/` and loaded by the standard upsert path, each court
//! placed and typed from the URN:LEX of its page. A page with no decision text takes it from an
//! archived copy in the bucket, else its decision is withdrawn.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use chrono::{Datelike, Days, NaiveDate, Utc};

use lj_bucket::{key, Bucket};
use lj_core::chamber::Seat;
use lj_parse::{
    archived_text, clean_ocr_markdown, juricaf_article, ohadata_decision, parse_juricaf,
    reflow_page_text, Recovered,
};
use lj_pg::db::Connection;
use lj_sources::juricaf::{Court, JuricafClient, Search};
use lj_store::repository::{DecisionRepository, ExtractedFields, Identity, JurisdictionRow};

use crate::config::Settings;
use crate::ocr::{self, Ocr};

use super::batch::drain_batch;
use super::cnda::pdf_has_text_layer;
use super::europe::europe_pool;
use super::{content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode, WriteMode};

/// The countries and courts whose primary source serves their decisions, as JuriCAF names them,
/// `None` for every court of the country: the sync takes every other one.
const SERVED_ELSEWHERE: [(&str, Option<&str>); 9] = [
    ("France", None),
    ("Suisse", None),
    ("Belgique", None),
    ("Luxembourg", Some("Cour de cassation")),
    ("Luxembourg", Some("Courdecassation")),
    ("Monaco", None),
    ("Canada", None),
    ("CJUE", None),
    ("CEDH", None),
];
/// The courts JuriCAF publishes under two slug families, the decision of a `dup` slug being the
/// one of the `kept` slug of the same day and number: (court, dup, kept).
const TWIN_SLUGS: [(&str, &str, &str); 1] = [(
    "mg_cour_constitutionnelle",
    "MADAGASCAR-LAHAUTECOURCONSTITUTIONELLEDUMADAGASCAR-",
    "MADAGASCAR-HAUTECOURCONSTITUTIONELLE-",
)];

/// The courts whose own site is the reference: a JuriCAF page of theirs is written only onto a
/// decision the base already holds at its address, one page a decision, and never relabels it.
const ATTACH_ONLY: [&str; 1] = ["unionafricaine_cour_droits_homme"];

/// A slug family whose pages JuriCAF also posts with their day DDMMYYYY rather than YYYYMMDD.
const DAY_FLIPPED: &str = "CADHP-COURAFRICAINEDESDROITSDELHOMMEETDESPEUPLES-";

/// The slug a page of a twin family is the copy of: the page is skipped while that one is held.
fn twin_of(slug: &str) -> Option<String> {
    TWIN_SLUGS
        .iter()
        .find_map(|(_, dup, kept)| slug.strip_prefix(dup).map(|rest| format!("{kept}{rest}")))
        .or_else(|| day_flipped(slug))
}

/// The other slug of a [`DAY_FLIPPED`] page: no day of 19xx or 20xx reads both ways.
fn day_flipped(slug: &str) -> Option<String> {
    let (day, rest) = slug.strip_prefix(DAY_FLIPPED)?.split_once('-')?;
    let (date, flipped) = match NaiveDate::parse_from_str(day, "%Y%m%d") {
        Ok(date) => (date, "%d%m%Y"),
        Err(_) => (NaiveDate::parse_from_str(day, "%d%m%Y").ok()?, "%Y%m%d"),
    };
    Some(format!("{DAY_FLIPPED}{}-{rest}", date.format(flipped)))
}

const BATCH: usize = 100;
/// The daily sync starts no batch past this, the pages left waiting for the next run.
const BUDGET: Duration = Duration::from_secs(1_200);
/// A listing costs the site about 30 ms a decision, 21 s for 686: a slice lists 500 at most.
const SLICE: usize = 500;
/// The imports page counts the 30 days before the day it is asked for, that day included.
const WINDOW: Days = Days::new(30);
const FIRST_DAY: NaiveDate = NaiveDate::from_ymd_opt(1, 1, 1).expect("a day");
const LAST_DAY: NaiveDate = NaiveDate::from_ymd_opt(9999, 12, 31).expect("a day");

fn pages_dir(settings: &Settings) -> PathBuf {
    settings.state_dir.join("ingest/juricaf-probe/html")
}

/// The day of the last run that fetched every page the imports listed, read again by the next.
fn cursor_path(settings: &Settings) -> PathBuf {
    settings
        .state_dir
        .join("ingest/juricaf-probe/imports-cursor")
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
/// `juricaf/{slug}`, checksum of the raw bytes. `dry_run` writes nothing and counts where each
/// page would go. `creates` and `attaches` place pages of an attach-only court by hand.
pub async fn juricaf_probe(
    dir: Option<PathBuf>,
    mode: IngestMode,
    dry_run: bool,
    creates: Vec<String>,
    attaches: Vec<(String, String)>,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = dir.unwrap_or_else(|| pages_dir(&settings));
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let paths = html_pages(&dir)?;
    let mut loader = Loader::new(&settings, &conn).await?;
    loader.dry_run = dry_run;
    let repo = DecisionRepository::new(&conn);
    let mut moved = Vec::new();
    for (slug, public_id) in attaches {
        let id = repo
            .decision_id_by_public_id(&public_id)
            .await?
            .ok_or_else(|| anyhow!("--attach {slug}: no live decision {public_id}"))?;
        let uid = format!("juricaf/{slug}");
        match repo
            .provenance_decisions(std::slice::from_ref(&uid))
            .await?
            .get(&uid)
        {
            None => {
                loader.placed.insert(slug, Placed::Onto(id));
            }
            Some(&(from, _)) if from != id => {
                tracing::warn!(slug = %slug, from, to = id, dry_run, "juricaf page moved by hand");
                if !dry_run {
                    repo.move_provenance(&uid, id).await?;
                    moved.extend([from, id]);
                }
            }
            Some(_) => {}
        }
    }
    super::reextract::fuse_written(&repo, &moved).await?;
    for slug in creates {
        loader.placed.insert(slug, Placed::New);
    }
    for chunk in paths.chunks(BATCH) {
        let pages = chunk
            .iter()
            .map(|p| Ok((slug_of(p)?, std::fs::read(p)?)))
            .collect::<Result<Vec<_>>>()?;
        loader.load(&conn, &pages, mode).await?;
    }
    loader.report(paths.len());
    loader.keep_set_aside(&settings, &dir)?;
    Ok(())
}

/// Where a page of an attach-only court goes when placed by hand.
#[derive(Clone, Copy)]
enum Placed {
    New,
    Onto(i64),
}

struct Loader {
    bucket: Bucket,
    ocr: Option<Ocr>,
    places: HashMap<String, bool>,
    counts: IngestCounts,
    withdrawn: usize,
    dry_run: bool,
    /// The decisions an attach-only page of this run went to.
    claimed: HashSet<i64>,
    /// Where each page went, counted when it is resolved before its batch.
    outcomes: BTreeMap<&'static str, usize>,
    /// The attach-only pages left out, with why.
    set_aside: Vec<(String, &'static str)>,
    placed: HashMap<String, Placed>,
}

impl Loader {
    async fn new(settings: &Settings, conn: &Connection) -> Result<Self> {
        Ok(Self {
            bucket: settings.bucket()?,
            ocr: Ocr::from_pool(&DecisionRepository::new(conn)).await?,
            places: HashMap::new(),
            counts: IngestCounts::default(),
            withdrawn: 0,
            dry_run: false,
            claimed: HashSet::new(),
            outcomes: BTreeMap::new(),
            set_aside: Vec::new(),
            placed: HashMap::new(),
        })
    }

    /// The text of the archived copy of what a page links: the decision an archived page holds,
    /// or a PDF's text layer, else its OCR, Mistral's answer kept beside it, an OHADATA file cut
    /// to its decision.
    async fn recovered(&self, slug: &str) -> Result<Option<Recovered>> {
        let Some(url) = self.bucket.get(&key::juricaf(slug, "url")).await? else {
            return Ok(None);
        };
        let url = String::from_utf8(url.to_vec())?.trim().to_string();
        let text = match self.bucket.get(&key::juricaf(slug, "html")).await? {
            Some(page) => archived_text(&page),
            None => {
                let pdf = self
                    .bucket
                    .get(&key::juricaf(slug, "pdf"))
                    .await?
                    .ok_or_else(|| anyhow!("juricaf {slug}: {url} has no copy beside it"))?;
                self.pdf_text(slug, &pdf, url.contains("ohada.com/"))
                    .await?
            }
        };
        Ok(text.map(|text| Recovered { text, url }))
    }

    async fn pdf_text(&self, slug: &str, pdf: &[u8], ohadata: bool) -> Result<Option<String>> {
        let decision = |text: String| {
            if ohadata {
                ohadata_decision(&text)
            } else {
                Some(text)
            }
        };
        if let Some(raw) = lj_sources::pdf::pdftotext_extract(pdf)
            .ok()
            .filter(|raw| pdf_has_text_layer(raw))
        {
            return Ok(decision(raw).map(|text| reflow_page_text(&text)));
        }
        let answer = key::ocr(&key::juricaf(slug, "pdf"));
        let cached = key::juricaf(slug, "ocr.md");
        let markdown = if let Some(pages) =
            ocr::kept::<Vec<serde_json::Value>>(&self.bucket, &answer).await?
        {
            ocr::text(&pages)
        } else if let Some(markdown) = self.bucket.get(&cached).await? {
            String::from_utf8(markdown.to_vec())?
        } else {
            let Some(ocr) = &self.ocr else {
                tracing::warn!(slug, "juricaf: a scan waits for a Mistral key");
                return Ok(None);
            };
            match ocr.document_kept(&self.bucket, &answer, pdf, slug).await {
                Ok(reading) => reading.text(),
                Err(e) => {
                    tracing::warn!(slug, error = %e, "juricaf: the OCR of a scan failed");
                    return Ok(None);
                }
            }
        };
        Ok(decision(clean_ocr_markdown(&markdown)))
    }

    async fn load(
        &mut self,
        conn: &Connection,
        pages: &[(String, Vec<u8>)],
        mode: IngestMode,
    ) -> Result<()> {
        let repo = DecisionRepository::new(conn);
        let mut candidates = Vec::new();
        let mut names = BTreeMap::new();
        let mut without_text = Vec::new();
        for (slug, raw) in pages {
            if let Some(twin) = twin_of(slug) {
                let twin = format!("juricaf/{twin}");
                if !repo
                    .provenance_decisions(std::slice::from_ref(&twin))
                    .await?
                    .is_empty()
                {
                    tracing::info!(slug = %slug, twin = %twin, "juricaf page of a decision held under its twin slug, skip");
                    self.counts.skipped += 1;
                    continue;
                }
            }
            let recovered = match juricaf_article(raw) {
                Some(_) => None,
                None if self.dry_run => {
                    *self.outcomes.entry("no text on its page").or_default() += 1;
                    continue;
                }
                None => match self.recovered(slug).await? {
                    Some(recovered) => Some(recovered),
                    None => {
                        without_text.push(format!("juricaf/{slug}"));
                        continue;
                    }
                },
            };
            let checksum = match &recovered {
                Some(r) => content_checksum(&[raw.as_slice(), r.text.as_bytes()].concat()),
                None => content_checksum(raw),
            };
            let parsed = match parse_juricaf(slug, raw, recovered) {
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
            let attach_only = ATTACH_ONLY.contains(&parsed.jurisdiction_code.as_str());
            let placed = self.placed.get(slug).copied();
            if placed.is_some() {
                *self.outcomes.entry("placed by hand").or_default() += 1;
                if self.dry_run {
                    continue;
                }
            } else if attach_only || self.dry_run {
                let uid = parsed.decision.source_uid.clone();
                let held = repo
                    .provenance_decisions(std::slice::from_ref(&uid))
                    .await?;
                let outcome = match held.get(&uid) {
                    Some(&(id, _)) => {
                        self.claimed.insert(id);
                        "held"
                    }
                    None => match repo
                        .identity_of(&parsed.decision, Some(&extracted), &parsed.source_fields)
                        .await?
                    {
                        Identity::Attach(id) if !attach_only || self.claimed.insert(id) => "attach",
                        Identity::Attach(_) => "twin of a page attached before",
                        Identity::Create => "create",
                        Identity::Copy(_) => "copy",
                        Identity::Hold(_) => "hold",
                    },
                };
                *self.outcomes.entry(outcome).or_default() += 1;
                if attach_only && !matches!(outcome, "held" | "attach") {
                    tracing::info!(slug = %slug, outcome, "juricaf page of an attach-only court set aside");
                    self.set_aside.push((slug.clone(), outcome));
                    self.counts.skipped += 1;
                    continue;
                }
                if self.dry_run {
                    continue;
                }
            }
            if !attach_only {
                names
                    .entry(parsed.jurisdiction_code.clone())
                    .or_insert_with(|| parsed.jurisdiction_label.clone());
            }
            candidates.push(Candidate {
                decision_id: match placed {
                    Some(Placed::Onto(id)) => Some(id),
                    _ => None,
                },
                public_id: generate_public_id(),
                content_checksum: checksum,
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
        if self.dry_run {
            return Ok(());
        }
        if !candidates.is_empty() {
            drain_batch(conn, candidates, mode, &mut self.counts).await?;
        }
        self.counts.skipped += without_text.len();
        for uid in &without_text {
            if repo.delete(uid).await? {
                self.withdrawn += 1;
            }
        }
        let codes: Vec<String> = names.keys().cloned().collect();
        let stored = repo.jurisdiction_labels(&codes).await?;
        let (codes, labels): (Vec<String>, Vec<String>) = names
            .into_iter()
            .filter(|(code, label)| {
                stored
                    .get(code)
                    .is_some_and(|s| lj_fold::fold(s) != lj_fold::fold(label))
            })
            .unzip();
        if !codes.is_empty() {
            repo.relabel_jurisdictions(&codes, &labels).await?;
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
            withdrawn = self.withdrawn,
            dry_run = self.dry_run,
            outcomes = ?self.outcomes,
            set_aside = self.set_aside.len(),
            "juricaf load done"
        );
    }

    /// The attach-only pages the run left out, one `slug<TAB>why` line each, in a file named
    /// after the pages' folder.
    fn keep_set_aside(&self, settings: &Settings, dir: &Path) -> Result<()> {
        let folder = dir
            .file_name()
            .and_then(|f| f.to_str())
            .ok_or_else(|| anyhow!("{} names no folder", dir.display()))?;
        let path = settings
            .state_dir
            .join(format!("ingest/juricaf-probe/{folder}-set-aside.tsv"));
        let lines: String = self
            .set_aside
            .iter()
            .map(|(slug, why)| format!("{slug}\t{why}\n"))
            .collect();
        std::fs::write(&path, lines)?;
        tracing::info!(path = %path.display(), pages = self.set_aside.len(), "attach-only pages set aside");
        Ok(())
    }
}

/// A page neither fetched nor found gone, or fetched no later than the day the site imported it
/// again. A 404 leaves an empty `{slug}.gone` beside the pages.
fn due(dir: &Path, slug: &str, imported: Option<NaiveDate>) -> Result<bool> {
    for kind in ["html", "gone"] {
        match std::fs::metadata(dir.join(format!("{slug}.{kind}"))) {
            Ok(meta) => {
                let fetched = chrono::DateTime::<Utc>::from(meta.modified()?).date_naive();
                return Ok(imported.is_some_and(|day| fetched <= day));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(true)
}

/// Splits a range of days in two, at a year's end when it spans several years.
fn halves(from: NaiveDate, to: NaiveDate) -> Option<[(NaiveDate, NaiveDate); 2]> {
    if from >= to {
        return None;
    }
    let end = if from.year() < to.year() {
        let year = from.year() + (to.year() - from.year() - 1) / 2;
        NaiveDate::from_ymd_opt(year, 12, 31).expect("a year's last day")
    } else {
        from + Days::new(((to - from).num_days() / 2) as u64)
    };
    Some([
        (from, end),
        (end.succ_opt().expect("a day before `to`"), to),
    ])
}

/// The searches a country is read by: none, the country whole, or each of its courts no primary
/// source serves. `--country` reads a country a primary source serves whole, never a court one
/// serves.
fn searches<'a>(country: &str, courts: &'a [Court], given: &[String]) -> Vec<Option<&'a str>> {
    let read = match given {
        [] => !SERVED_ELSEWHERE.contains(&(country, None)),
        given => given.iter().any(|c| c == country),
    };
    if !read {
        return Vec::new();
    }
    if !SERVED_ELSEWHERE
        .iter()
        .any(|(c, court)| *c == country && court.is_some())
    {
        return vec![None];
    }
    courts
        .iter()
        .filter(|c| {
            c.country == country && !SERVED_ELSEWHERE.contains(&(country, Some(c.court.as_str())))
        })
        .map(|c| Some(c.court.as_str()))
        .collect()
}

/// The slugs of the decisions of `country`, or of its `court`, a search matches, imported on
/// `imported` when given, listed by slices of their dates small enough for one listing each.
async fn listed(
    client: &JuricafClient,
    country: &str,
    court: Option<&str>,
    imported: Option<NaiveDate>,
) -> Result<Vec<String>> {
    let total = client
        .count(&Search {
            country,
            court,
            imported,
            decided: None,
        })
        .await?;
    let mut slugs = Vec::new();
    let mut reached = 0usize;
    let mut slices = vec![(None, Some(total))];
    while let Some((decided, known)) = slices.pop() {
        let search = Search {
            country,
            court,
            imported,
            decided,
        };
        let count = match known {
            Some(count) => count,
            None => client.count(&search).await?,
        };
        if count == 0 {
            continue;
        }
        if count <= SLICE {
            let listing = client.listing(&search).await?;
            if listing.returned == listing.total {
                reached += listing.total;
                slugs.extend(listing.slugs);
                continue;
            }
        }
        let (from, to) = decided.unwrap_or((FIRST_DAY, LAST_DAY));
        let Some(halves) = halves(from, to) else {
            bail!("juricaf {country}: {count} decisions dated {from}, more than one listing holds");
        };
        slices.extend(halves.map(|half| (Some(half), None)));
    }
    if reached < total {
        bail!(
            "juricaf {country}: its date slices reach {reached} of the {total} decisions it holds"
        );
    }
    Ok(slugs)
}

/// The pages the site imported since `cursor`, that day included, else over the 30 days its
/// imports page shows.
async fn imported_since(
    client: &JuricafClient,
    dir: &Path,
    courts: &[Court],
    given: &[String],
    cursor: Option<NaiveDate>,
    today: NaiveDate,
) -> Result<BTreeSet<String>> {
    let mut imports = Vec::new();
    let mut until = today;
    loop {
        imports.extend(client.imports(until).await?);
        let from = until - WINDOW;
        match cursor {
            Some(cursor) if cursor < from => until = from.pred_opt().expect("a day before"),
            _ => break,
        }
    }
    let mut wanted = BTreeSet::new();
    for import in imports.iter().filter(|i| cursor.is_none_or(|c| i.day >= c)) {
        for court in searches(&import.country, courts, given) {
            for slug in listed(client, &import.country, court, Some(import.day)).await? {
                if due(dir, &slug, Some(import.day))? {
                    wanted.insert(slug);
                }
            }
        }
    }
    Ok(wanted)
}

/// The pages of each country's or court's stock the folder lacks, each total checked against the
/// statistics page.
async fn stock(
    client: &JuricafClient,
    dir: &Path,
    courts: &[Court],
    given: &[String],
) -> Result<BTreeSet<String>> {
    let mut by_country: BTreeMap<&str, usize> = BTreeMap::new();
    for court in courts {
        *by_country.entry(court.country.as_str()).or_default() += court.count;
    }
    if let Some(unknown) = given.iter().find(|c| !by_country.contains_key(c.as_str())) {
        bail!("juricaf names no country {unknown}: `sync-juricaf --list` gives their names");
    }
    let mut wanted = BTreeSet::new();
    for (country, whole) in &by_country {
        for court in searches(country, courts, given) {
            let stated = match court {
                None => *whole,
                Some(court) => courts
                    .iter()
                    .filter(|c| c.country == *country && c.court == court)
                    .map(|c| c.count)
                    .sum(),
            };
            let slugs = listed(client, country, court, None).await?;
            if slugs.len() != stated {
                tracing::warn!(
                    country,
                    court,
                    listed = slugs.len(),
                    stated,
                    "juricaf: the search and the statistics page count a stock apart"
                );
            }
            for slug in slugs {
                if due(dir, &slug, None)? {
                    wanted.insert(slug);
                }
            }
        }
    }
    Ok(wanted)
}

/// Fetches the pages the site imported since the last run, then loads them. `full` reads instead
/// the stock of each country by slices of dates, with no time budget; `countries` takes only
/// those; `list` counts the decisions by country and court.
pub async fn sync_juricaf(countries: &[String], full: bool, list: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = pages_dir(&settings);
    let client = JuricafClient::new(&settings.egress_proxy)?;
    let courts = client.courts().await?;
    if list {
        for court in &courts {
            println!("{}\t{}\t{}", court.count, court.country, court.court);
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
    let today = Utc::now().date_naive();
    let cursor_path = cursor_path(&settings);
    let wanted = if full {
        stock(&client, &dir, &courts, countries).await?
    } else {
        let cursor = match std::fs::read_to_string(&cursor_path) {
            Ok(day) => Some(NaiveDate::parse_from_str(day.trim(), "%Y-%m-%d")?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!("juricaf sync: no cursor yet, the imports page's 30 days are read");
                None
            }
            Err(e) => return Err(e.into()),
        };
        imported_since(&client, &dir, &courts, countries, cursor, today).await?
    };
    let wanted: Vec<String> = wanted.into_iter().collect();
    tracing::info!(full, wanted = wanted.len(), dir = %dir.display(), "juricaf sync: pages to fetch");

    let mut loader = Loader::new(&settings, &conn).await?;
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
    } else if !full && countries.is_empty() {
        std::fs::write(&cursor_path, today.to_string())?;
    }
    if gone > 0 {
        tracing::warn!(gone, "juricaf sync: pages a listing names answer 404");
    }
    loader.report(fetched - gone);
    Ok(())
}

/// Merges the decisions JuriCAF holds under two slug families of one court: the twin's into the
/// keeper's, every provenance kept, then the decisions the court's own site wrote beside the two
/// for want of a single one to attach to. Without `apply`, counts them and changes nothing.
pub async fn merge_juricaf_twins(apply: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let mut keepers = Vec::new();
    for (court, dup, kept) in TWIN_SLUGS {
        let pairs = repo.juricaf_twin_pairs(court, dup, kept).await?;
        let keeper_of: HashMap<i64, i64> = pairs.iter().map(|&(k, t)| (t, k)).collect();
        let mut copies = Vec::new();
        if let Some(site) = lj_parse::SITE_COURTS.iter().find(|c| c.code == court) {
            for (copy, beside) in repo.single_source_twins(court, site.source).await? {
                let onto: BTreeSet<i64> = beside
                    .iter()
                    .map(|b| keeper_of.get(b).copied().unwrap_or(*b))
                    .collect();
                if let ([_, _], [keeper]) = (&beside[..], &onto.into_iter().collect::<Vec<_>>()[..])
                {
                    copies.push((copy, *keeper));
                }
            }
        }
        let moved = copies.len();
        tracing::warn!(
            court,
            pairs = pairs.len(),
            site_copies = moved,
            apply,
            "merge-juricaf-twins"
        );
        if !apply {
            continue;
        }
        for (keeper, twin) in pairs {
            repo.absorb_into(twin, keeper).await?;
            keepers.push(keeper);
        }
        for (copy, keeper) in copies {
            repo.absorb_into(copy, keeper).await?;
            keepers.push(keeper);
        }
    }
    keepers.sort_unstable();
    keepers.dedup();
    super::reextract::fuse_written(&repo, &keepers).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_country_a_primary_source_serves_in_part_is_read_court_by_court() {
        let court = |country: &str, court: &str| Court {
            country: country.into(),
            court: court.into(),
            count: 1,
        };
        let courts = [
            court("Luxembourg", "Cour de cassation"),
            court("Luxembourg", "Tribunal administratif"),
            court("Bénin", "Cour suprême"),
            court("France", "Conseil d'État"),
        ];
        let given = |c: &str| vec![c.to_string()];
        assert_eq!(
            searches("Luxembourg", &courts, &[]),
            vec![Some("Tribunal administratif")]
        );
        assert_eq!(
            searches("Luxembourg", &courts, &given("Luxembourg")),
            vec![Some("Tribunal administratif")]
        );
        assert_eq!(searches("Bénin", &courts, &[]), vec![None]);
        assert!(searches("Bénin", &courts, &given("Luxembourg")).is_empty());
        assert!(searches("France", &courts, &[]).is_empty());
        assert_eq!(searches("France", &courts, &given("France")), vec![None]);
    }

    fn day(d: &str) -> NaiveDate {
        NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap()
    }

    /// Splits until every slice is a single year, then a single day, as a crowded country would.
    fn leaves(
        from: NaiveDate,
        to: NaiveDate,
        stop: &dyn Fn(NaiveDate, NaiveDate) -> bool,
    ) -> Vec<(NaiveDate, NaiveDate)> {
        match halves(from, to).filter(|_| !stop(from, to)) {
            None => vec![(from, to)],
            Some([a, b]) => [leaves(a.0, a.1, stop), leaves(b.0, b.1, stop)].concat(),
        }
    }

    #[test]
    fn date_slices_split_at_years_ends_then_inside_a_year_and_cover_the_range_once() {
        assert_eq!(
            halves(FIRST_DAY, LAST_DAY),
            Some([
                (FIRST_DAY, day("4999-12-31")),
                (day("5000-01-01"), LAST_DAY)
            ])
        );
        assert_eq!(
            halves(day("2020-01-01"), day("2021-12-31")),
            Some([
                (day("2020-01-01"), day("2020-12-31")),
                (day("2021-01-01"), day("2021-12-31"))
            ])
        );
        assert_eq!(
            halves(day("2024-01-01"), day("2024-12-31")),
            Some([
                (day("2024-01-01"), day("2024-07-01")),
                (day("2024-07-02"), day("2024-12-31"))
            ])
        );
        assert_eq!(halves(day("2024-03-01"), day("2024-03-01")), None);

        let years = leaves(day("1833-01-01"), day("2026-12-31"), &|from, to| {
            from.year() == to.year()
        });
        assert_eq!(years.len(), 2026 - 1833 + 1);
        assert!(years
            .iter()
            .all(|(from, to)| from.ordinal() == 1 && to.month() == 12 && to.day() == 31));
        let days = leaves(day("2024-01-01"), day("2024-12-31"), &|_, _| false);
        assert_eq!(days.len(), 366);
        assert!(days.windows(2).all(|w| w[0].1.succ_opt() == Some(w[1].0)));
    }

    #[test]
    fn an_african_court_page_posted_under_both_day_orders_is_the_twin_of_the_other() {
        let slug = |day: &str| format!("{DAY_FLIPPED}{day}-0222015");
        assert_eq!(day_flipped(&slug("11052018")), Some(slug("20180511")));
        assert_eq!(day_flipped(&slug("20180511")), Some(slug("11052018")));
        assert_eq!(twin_of(&slug("20180511")), Some(slug("11052018")));
        assert_eq!(
            day_flipped(&format!("{DAY_FLIPPED}20160603-RANDOM105121363")),
            Some(format!("{DAY_FLIPPED}03062016-RANDOM105121363"))
        );
        assert_eq!(day_flipped("AFRIQUE-CADHP-20190320-0042018"), None);
        assert_eq!(day_flipped(&slug("32132018")), None);
    }
}
