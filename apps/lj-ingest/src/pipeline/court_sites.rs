//! The decisions African courts publish on their own sites: each site's listing walked newest
//! first, each new decision's text read once, loaded by the standard upsert path, then kept under
//! `<state_dir>/ingest/court-sites/{source}/`. A PDF without a text layer is counted, its pages
//! with it, and left for an OCR, or read by it on a site whose scans are read, its PDF then kept
//! in the bucket; a PDF whose text layer is garbled is read by the OCR.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use chrono::NaiveDate;
use serde_json::{json, Value};

use lj_bucket::{key, Bucket};
use lj_core::chamber::Seat;
use lj_core::docket_key::docket_key;
use lj_core::identity::text_containment;
use lj_parse::{
    african_court_advisories, african_court_advisory_pdf, african_court_listing,
    cassation_ci_listing, cc_bj_listing, cc_ci_listing, cc_dj_listing, cc_ma_listing, cc_ma_page,
    ccja_listing, clean_ocr_markdown, cndj_ci_listing, garbled_layer, hcc_mg_listing,
    holds_a_decision, html_text, jurisprudence_mg_listing, jurisprudence_mg_page,
    parse_site_decision, SiteBody, SiteEntry, SiteListing,
};
use lj_pg::db::Connection;
use lj_sources::court_sites::{
    african_court_listing_url, cassation_ci_listing_url, cc_bj_listing_url, cc_ci_listing_url,
    cc_dj_media_url, cc_ma_listing_form, ccja_listing_url, cndj_ci_listing_url, hcc_mg_posts_url,
    jurisprudence_mg_listing_url, CourtSiteClient, AFRICAN_COURT_ADVISORY_URL,
    CASSATION_CI_MATTERS, CCJA_YEAR_CATEGORIES, CC_MA_LISTING_URL, CNDJ_CI_COLLECTIONS,
    JURISPRUDENCE_MG_NATURES,
};
use lj_store::repository::{
    DecisionRepository, ExtractedFields, JurisdictionRow, SOURCE_TWIN_CONTAINMENT,
};

use crate::config::Settings;
use crate::ocr::{self, Ocr};

use super::batch::drain_batch;
use super::europe::europe_pool;
use super::{content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode, WriteMode};

const BATCH: usize = 100;

/// A site, the least time between two of its requests and between two of its listing pages,
/// whether it still publishes, and whether its scans are read by the OCR.
struct Site {
    source: &'static str,
    pace: Duration,
    listing_gap: Duration,
    publishing: bool,
    reads_scans: bool,
}

/// How long a `--full` run of a site whose listing pages are spaced takes one back from its last
/// read rather than asking it again.
const LISTING_KEPT: Duration = Duration::from_secs(7 * 86_400);

const SITES: [Site; 10] = [
    Site {
        source: "african-court",
        pace: Duration::from_secs(5),
        listing_gap: Duration::ZERO,
        publishing: true,
        reads_scans: false,
    },
    Site {
        source: "cassation-ci",
        pace: Duration::from_secs(10),
        listing_gap: Duration::ZERO,
        publishing: true,
        reads_scans: false,
    },
    Site {
        source: "cc-bj",
        pace: Duration::from_secs(2),
        listing_gap: Duration::ZERO,
        publishing: true,
        reads_scans: false,
    },
    // Its nginx answers 429 at the 30 s its robots.txt asks, and to three listing pages in 2 min.
    Site {
        source: "ccja",
        pace: Duration::from_secs(60),
        listing_gap: Duration::from_secs(300),
        publishing: true,
        reads_scans: false,
    },
    // Its robots.txt asks 10 s between requests.
    Site {
        source: "cc-ci",
        pace: Duration::from_secs(10),
        listing_gap: Duration::ZERO,
        publishing: true,
        reads_scans: false,
    },
    Site {
        source: "hcc-mg",
        pace: Duration::from_secs(2),
        listing_gap: Duration::ZERO,
        publishing: true,
        reads_scans: false,
    },
    // Its robots.txt asks 10 s between requests; it publishes nothing after 2018.
    Site {
        source: "jurisprudence-mg",
        pace: Duration::from_secs(10),
        listing_gap: Duration::ZERO,
        publishing: false,
        reads_scans: false,
    },
    // Its robots.txt asks 20 s between requests; its rulings are scans.
    Site {
        source: "cc-dj",
        pace: Duration::from_secs(20),
        listing_gap: Duration::ZERO,
        publishing: true,
        reads_scans: true,
    },
    // Its free collections are scans, of judicial years 2017-2018 and 2018-2019.
    Site {
        source: "cndj-ci",
        pace: Duration::from_secs(3),
        listing_gap: Duration::ZERO,
        publishing: false,
        reads_scans: true,
    },
    Site {
        source: "cc-ma",
        pace: Duration::from_secs(2),
        listing_gap: Duration::ZERO,
        publishing: true,
        reads_scans: false,
    },
];

/// The listings a site is walked by, each paged from 1.
fn walks(source: &str) -> &'static [&'static str] {
    match source {
        "african-court" => &["", "advisory"],
        "jurisprudence-mg" => &JURISPRUDENCE_MG_NATURES,
        "ccja" => &CCJA_YEAR_CATEGORIES,
        "cassation-ci" => &CASSATION_CI_MATTERS,
        "cndj-ci" => &CNDJ_CI_COLLECTIONS,
        _ => &[""],
    }
}

/// The URL of a walk's page, `None` past its last.
fn listing_url(source: &str, walk: &str, page: usize) -> Option<String> {
    match (source, walk) {
        ("cc-ma", _) => Some(CC_MA_LISTING_URL.into()),
        ("african-court", "advisory") => (page == 1).then(|| AFRICAN_COURT_ADVISORY_URL.into()),
        ("african-court", _) => Some(african_court_listing_url(page)),
        ("cassation-ci", matter) => Some(cassation_ci_listing_url(matter, page)),
        ("cc-bj", _) => Some(cc_bj_listing_url(page)),
        ("cc-ci", _) => Some(cc_ci_listing_url(page)),
        ("cc-dj", _) => Some(cc_dj_media_url(page)),
        ("cndj-ci", collection) => Some(cndj_ci_listing_url(collection, page)),
        ("ccja", category) => Some(ccja_listing_url(category, page)),
        ("hcc-mg", _) => Some(hcc_mg_posts_url(page)),
        ("jurisprudence-mg", _) => Some(jurisprudence_mg_listing_url(walk, page)),
        (other, _) => unreachable!("no listing for {other}"),
    }
}

fn read_listing(source: &str, walk: &str, body: &str) -> Result<SiteListing> {
    Ok(match source {
        "african-court" if walk == "advisory" => african_court_advisories(body),
        "african-court" => african_court_listing(body)?,
        "cassation-ci" => cassation_ci_listing(body),
        "cc-bj" => cc_bj_listing(body),
        "cc-ci" => cc_ci_listing(body),
        "cc-dj" => cc_dj_listing(body)?,
        "cc-ma" => cc_ma_listing(body)?,
        "ccja" => ccja_listing(body),
        "cndj-ci" => cndj_ci_listing(body),
        "hcc-mg" => hcc_mg_listing(body)?,
        "jurisprudence-mg" => jurisprudence_mg_listing(body),
        other => unreachable!("no listing for {other}"),
    })
}

/// The folders a decision is kept in once read: loaded, a scan, refused at parse, gone, or the
/// twin of one the site posted before.
const KINDS: [&str; 5] = ["decisions", "scans", "refused", "gone", "twins"];

fn file_name(id: &str) -> String {
    format!("{}.json", id.replace(['/', '\\'], "_"))
}

/// A listing page: the one a `--full` run of the week kept, when the site's listing pages are
/// spaced, else asked of the site `listing_gap` after the last one asked.
async fn listing_page(
    client: &CourtSiteClient,
    site: &Site,
    kept: &Path,
    url: &str,
    page: usize,
    full: bool,
    listed: &mut Option<Instant>,
) -> Result<Option<String>> {
    let spaced = !site.listing_gap.is_zero();
    if spaced && full {
        let age = std::fs::metadata(kept)
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().unwrap_or_default());
        if age.is_ok_and(|age| age < LISTING_KEPT) {
            return Ok(Some(std::fs::read_to_string(kept)?));
        }
    }
    if let Some(at) = *listed {
        tokio::time::sleep(site.listing_gap.saturating_sub(at.elapsed())).await;
    }
    let body = match site.source {
        "cc-ma" => client.form_or_end(url, &cc_ma_listing_form(page)).await?,
        _ => client.page_or_end(url).await?,
    };
    *listed = Some(Instant::now());
    if let (true, Some(body)) = (spaced, &body) {
        std::fs::create_dir_all(kept.parent().expect("a listing page is kept in a folder"))?;
        let part = kept.with_extension("part");
        std::fs::write(&part, body)?;
        std::fs::rename(&part, kept)?;
    }
    Ok(body)
}

fn visited(dir: &Path, id: &str) -> bool {
    KINDS
        .iter()
        .any(|k| dir.join(k).join(file_name(id)).exists())
}

fn keep(dir: &Path, kind: &str, id: &str, value: &Value) -> Result<()> {
    let folder = dir.join(kind);
    std::fs::create_dir_all(&folder)?;
    let path = folder.join(file_name(id));
    let part = path.with_extension("part");
    std::fs::write(&part, serde_json::to_vec(value)?)?;
    std::fs::rename(&part, path)?;
    Ok(())
}

/// What reading a decision's body gave.
enum Read {
    Text {
        text: String,
        from_pdf: bool,
    },
    Scan {
        pages: u32,
    },
    /// The site serves no French text for it: its file is gone, or its page links none.
    Gone,
    /// A scan of a site whose scans are read, which the OCR did not read this time.
    Waiting,
}

/// The OCR a garbled text layer, and a scan of a site whose scans are read, is read by, and what
/// it read.
struct Rereader {
    ocr: Option<Ocr>,
    bucket: Bucket,
    source: &'static str,
    scans: bool,
    read: Cell<usize>,
    pages: Cell<usize>,
}

impl Rereader {
    /// The OCR text of the PDF `file`, Mistral's answer kept in the bucket and taken from there
    /// on a later read; a scan's PDF is kept beside it. `None` when the pool is empty or the OCR
    /// fails.
    async fn text(&self, file: &str, pdf: &[u8], scan: bool) -> Result<Option<String>> {
        let at = key::court_site(self.source, file, "pdf");
        let kept = key::ocr(&at);
        let pages = match ocr::kept::<Vec<Value>>(&self.bucket, &kept).await? {
            Some(pages) => pages,
            None => {
                let Some(ocr) = &self.ocr else {
                    tracing::warn!(
                        source = self.source,
                        file,
                        "court site: a PDF to read by the OCR waits for a Mistral key"
                    );
                    return Ok(None);
                };
                let label = format!("{}/{file}", self.source);
                match ocr.document_kept(&self.bucket, &kept, pdf, &label).await {
                    Ok(reading) => {
                        if scan {
                            self.bucket.put(&at, pdf.to_vec()).await?;
                        }
                        self.read.set(self.read.get() + 1);
                        self.pages.set(self.pages.get() + reading.pages.len());
                        reading.pages
                    }
                    Err(e) => {
                        tracing::warn!(source = self.source, file, error = %e, "court site: the OCR of a PDF failed");
                        return Ok(None);
                    }
                }
            }
        };
        Ok(Some(clean_ocr_markdown(&ocr::text(&pages))))
    }
}

/// A PDF's text layer, or its OCR text when the layer is garbled, or holds none on a site whose
/// scans are read, and `rereader` reads it.
async fn pdf_read(bytes: &[u8], file: &str, rereader: Option<&Rereader>) -> Result<Read> {
    Ok(match lj_sources::pdf::pdftotext_extract(bytes) {
        Ok(raw) if holds_a_decision(&raw) => {
            let reread = match rereader {
                Some(r) if garbled_layer(&raw) => r.text(file, bytes, false).await?,
                _ => None,
            };
            Read::Text {
                text: reread.unwrap_or(raw),
                from_pdf: true,
            }
        }
        _ => match rereader {
            Some(r) if r.scans => match r.text(file, bytes, true).await? {
                Some(text) => Read::Text {
                    text,
                    from_pdf: true,
                },
                None => Read::Waiting,
            },
            _ => Read::Scan {
                pages: lj_sources::pdf::page_count(bytes).unwrap_or(0),
            },
        },
    })
}

async fn read_body(
    client: &CourtSiteClient,
    source: &str,
    entry: &mut SiteEntry,
    rereader: Option<&Rereader>,
) -> Result<Read> {
    match entry.body.clone() {
        SiteBody::Pdf(url) => Ok(match client.file(&url).await? {
            Some(bytes) => pdf_read(&bytes, &entry.id, rereader).await?,
            None => Read::Gone,
        }),
        SiteBody::Html { html, pdf } => {
            let text = html_text(&html);
            if holds_a_decision(&text) {
                return Ok(Read::Text {
                    text,
                    from_pdf: false,
                });
            }
            let Some(url) = pdf else {
                return Ok(Read::Text {
                    text,
                    from_pdf: false,
                });
            };
            Ok(match client.file(&url).await? {
                Some(bytes) => pdf_read(&bytes, &entry.id, rereader).await?,
                None => Read::Gone,
            })
        }
        SiteBody::PdfOnPage(url) => {
            let page = client.page(&url).await?;
            let Some(pdf) = african_court_advisory_pdf(&page) else {
                return Ok(Read::Gone);
            };
            entry.body = SiteBody::Pdf(pdf.clone());
            Ok(match client.file(&pdf).await? {
                Some(bytes) => pdf_read(&bytes, &entry.id, rereader).await?,
                None => Read::Gone,
            })
        }
        SiteBody::Page(url) => {
            let page = client.page(&url).await?;
            let read = match source {
                "cc-ma" => cc_ma_page(&page),
                _ => jurisprudence_mg_page(&page),
            };
            let (text, fields) =
                read.ok_or_else(|| anyhow!("{url}: a decision page without its content"))?;
            entry.fields.extend(fields);
            Ok(Read::Text {
                text,
                from_pdf: false,
            })
        }
    }
}

pub(super) fn candidate(
    source: &str,
    entry: &SiteEntry,
    text: &str,
    from_pdf: bool,
) -> Result<Candidate> {
    let parsed = parse_site_decision(source, entry, text, from_pdf)?;
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
            place: Some(court.place.to_string()),
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
        payload_format: if from_pdf { "pdf" } else { "html" }.to_string(),
        write_mode: WriteMode::Full,
        passages: true,
        distinct: false,
    })
}

#[derive(Default)]
struct Tally {
    counts: IngestCounts,
    loaded: usize,
    scans: usize,
    scan_pages: u64,
    ocr: usize,
    ocr_pages: usize,
    refused: usize,
    gone: usize,
    twins: usize,
    unread: usize,
    waiting: usize,
}

/// A court's decision of a day under its docket keys, sorted.
type Address = (String, NaiveDate, Vec<String>);

fn address(c: &Candidate) -> Option<Address> {
    let e = c
        .prebuilt_extracted
        .as_ref()
        .expect("a court site candidate carries its fields");
    let (Some(court), Some(date)) = (e.jurisdiction.as_ref(), e.date_lecture) else {
        return None;
    };
    let keys: BTreeSet<String> = e
        .docket_numbers
        .iter()
        .map(|n| docket_key(n))
        .filter(|k| !k.is_empty())
        .collect();
    (!keys.is_empty()).then(|| (court.code.clone(), date, keys.into_iter().collect()))
}

/// The provenance of the same site that already holds the candidate's text at its address:
/// waiting in this batch, or loaded before it.
async fn twin_of(
    repo: &DecisionRepository<'_>,
    source: &str,
    c: &Candidate,
    waiting: &[Candidate],
) -> Result<Option<String>> {
    let Some(at) = address(c) else {
        return Ok(None);
    };
    let text = &c.decision.texte_integral_clean;
    if let Some(w) = waiting.iter().find(|w| {
        address(w).as_ref() == Some(&at)
            && text_containment(text, &w.decision.texte_integral_clean) >= SOURCE_TWIN_CONTAINMENT
    }) {
        return Ok(Some(w.decision.source_uid.clone()));
    }
    let dockets = &c
        .prebuilt_extracted
        .as_ref()
        .expect("a court site candidate carries its fields")
        .docket_numbers;
    Ok(repo
        .source_twin(source, &c.decision.source_uid, &at.0, at.1, dockets, text)
        .await?)
}

/// The candidates waiting for their batch, each with what is kept once it is loaded.
struct Pending {
    candidates: Vec<Candidate>,
    kept: Vec<(String, Value)>,
}

impl Pending {
    async fn flush(&mut self, conn: &Connection, dir: &Path, tally: &mut Tally) -> Result<()> {
        if self.candidates.is_empty() {
            return Ok(());
        }
        let candidates = std::mem::take(&mut self.candidates);
        drain_batch(conn, candidates, IngestMode::MissingHash, &mut tally.counts).await?;
        for (id, value) in self.kept.drain(..) {
            keep(dir, "decisions", &id, &value)?;
            tally.loaded += 1;
        }
        Ok(())
    }
}

async fn sync_site(
    settings: &Settings,
    conn: &Connection,
    site: &Site,
    budget: Duration,
    full: bool,
    limit: Option<usize>,
    ocr_tenant: &str,
) -> Result<Tally> {
    let started = Instant::now();
    let dir = settings
        .state_dir
        .join("ingest/court-sites")
        .join(site.source);
    let client = CourtSiteClient::new(&settings.egress_proxy, site.pace)?;
    let repo = DecisionRepository::new(conn);
    let rereader = Rereader {
        ocr: Ocr::tenants(&repo, ocr_tenant).await?,
        bucket: settings.bucket()?,
        source: site.source,
        scans: site.reads_scans,
        read: Cell::new(0),
        pages: Cell::new(0),
    };
    let mut tally = Tally::default();
    let mut pending = Pending {
        candidates: Vec::new(),
        kept: Vec::new(),
    };
    let mut read = 0usize;
    let mut seen = BTreeSet::new();
    let mut listed = None;
    'walks: for walk in walks(site.source) {
        for page in 1.. {
            if (!full && started.elapsed() > budget) || limit.is_some_and(|l| read >= l) {
                break 'walks;
            }
            let Some(url) = listing_url(site.source, walk, page) else {
                break;
            };
            let kept = dir.join("listings").join(format!("{walk}-{page}.html"));
            let Some(body) =
                listing_page(&client, site, &kept, &url, page, full, &mut listed).await?
            else {
                break;
            };
            let listing = read_listing(site.source, walk, &body)?;
            if listing.entries.is_empty() && listing.unread.is_empty() {
                break;
            }
            for unread in &listing.unread {
                tracing::warn!(source = site.source, row = %unread, "court site: a listing row without its number or date");
            }
            tally.unread += listing.unread.len();
            let mut new = 0usize;
            for mut entry in listing.entries {
                if visited(&dir, &entry.id)
                    || limit.is_some_and(|l| read >= l)
                    || !seen.insert(entry.id.clone())
                {
                    continue;
                }
                new += 1;
                read += 1;
                let base = json!({
                    "id": entry.id,
                    "numbers": entry.numbers,
                    "date": entry.date,
                    "url_site": entry.page_url,
                });
                match read_body(&client, site.source, &mut entry, Some(&rereader)).await? {
                    Read::Gone => {
                        keep(&dir, "gone", &entry.id, &base)?;
                        tally.gone += 1;
                    }
                    Read::Waiting => tally.waiting += 1,
                    Read::Scan { pages } => {
                        keep(
                            &dir,
                            "scans",
                            &entry.id,
                            &json!({"entry": base, "pages": pages}),
                        )?;
                        tally.scans += 1;
                        tally.scan_pages += u64::from(pages);
                    }
                    Read::Text { text, from_pdf } => {
                        match candidate(site.source, &entry, &text, from_pdf) {
                            Ok(c) => {
                                if let Some(uid) =
                                    twin_of(&repo, site.source, &c, &pending.candidates).await?
                                {
                                    tracing::warn!(source = site.source, id = %entry.id, twin_of = %uid, "court site: a decision posted twice, kept aside");
                                    keep(
                                        &dir,
                                        "twins",
                                        &entry.id,
                                        &json!({"entry": base, "twin_of": uid}),
                                    )?;
                                    tally.twins += 1;
                                    continue;
                                }
                                let kept = json!({
                                    "source_fields": c.prebuilt_source_fields,
                                    "text": c.decision.texte_integral_clean,
                                });
                                pending.kept.push((entry.id.clone(), kept));
                                pending.candidates.push(c);
                            }
                            Err(e) => {
                                tracing::warn!(source = site.source, id = %entry.id, error = %e, "court site: a decision refused");
                                keep(
                                    &dir,
                                    "refused",
                                    &entry.id,
                                    &json!({"entry": base, "why": e.to_string()}),
                                )?;
                                tally.refused += 1;
                            }
                        }
                    }
                }
                if pending.candidates.len() >= BATCH {
                    pending.flush(conn, &dir, &mut tally).await?;
                    tracing::info!(
                        source = site.source,
                        loaded = tally.loaded,
                        scans = tally.scans,
                        "court site sync"
                    );
                }
            }
            if (new == 0 && !full) || listing.last_page.is_some_and(|last| page >= last) {
                break;
            }
        }
    }
    pending.flush(conn, &dir, &mut tally).await?;
    if let Some(ocr) = &rereader.ocr {
        ocr.park_spent(&repo).await?;
    }
    tally.ocr = rereader.read.get();
    tally.ocr_pages = rereader.pages.get();
    Ok(tally)
}

/// Walks each site's listing, newest first, and loads the decisions not read yet. `sites` takes
/// only those, else every site still publishing; `full` walks each listing to its end with no time
/// budget, else a site starts no listing page `minutes` past its first, the decisions left waiting
/// for the next run, and a listing page with nothing new ends its walk; `limit` stops after that
/// many decisions; `dry_run` reads the first listing page of each and prints its rows.
pub async fn sync_court_sites(
    sites: &[String],
    full: bool,
    limit: Option<usize>,
    minutes: u64,
    dry_run: bool,
    ocr_tenant: &str,
) -> Result<()> {
    let settings = Settings::from_env()?;
    if let Some(unknown) = sites.iter().find(|s| !SITES.iter().any(|x| x.source == *s)) {
        bail!(
            "no court site {unknown}: {}",
            SITES.map(|s| s.source).join(", ")
        );
    }
    let taken: Vec<&Site> = SITES
        .iter()
        .filter(|s| {
            if sites.is_empty() {
                s.publishing
            } else {
                sites.iter().any(|x| x == s.source)
            }
        })
        .collect();
    if dry_run {
        for site in taken {
            dry(&settings, site, limit.unwrap_or(5)).await?;
        }
        return Ok(());
    }
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let mut failed = BTreeMap::new();
    let budget = Duration::from_secs(minutes * 60);
    for site in taken {
        if !repo.court_site_lock(site.source).await? {
            tracing::info!(
                source = site.source,
                "court site sync: another run holds the lock"
            );
            continue;
        }
        match sync_site(&settings, &conn, site, budget, full, limit, ocr_tenant).await {
            Ok(t) => tracing::warn!(
                source = site.source,
                loaded = t.loaded,
                created = t.counts.created,
                updated = t.counts.updated,
                skipped = t.counts.skipped,
                errors = t.counts.errors,
                scans = t.scans,
                scan_pages = t.scan_pages,
                ocr = t.ocr,
                ocr_pages = t.ocr_pages,
                refused = t.refused,
                gone = t.gone,
                twins = t.twins,
                unread = t.unread,
                waiting = t.waiting,
                "court site sync done"
            ),
            Err(e) => {
                tracing::error!(source = site.source, error = %e, "court site sync stopped, the next run resumes");
                failed.insert(site.source, e.to_string());
            }
        }
    }
    if !failed.is_empty() {
        bail!("court site sync failed: {failed:?}");
    }
    Ok(())
}

/// The first listing page of a site, and the first `limit` of its decisions read and parsed, a
/// line on stdout for each: no database, nothing kept.
async fn dry(settings: &Settings, site: &Site, limit: usize) -> Result<()> {
    let client = CourtSiteClient::new(&settings.egress_proxy, site.pace)?;
    let walk = walks(site.source)[0];
    let url = listing_url(site.source, walk, 1).expect("every walk has a first page");
    let body = match site.source {
        "cc-ma" => client
            .form_or_end(&url, &cc_ma_listing_form(1))
            .await?
            .ok_or_else(|| anyhow!("{url}: no first listing page"))?,
        _ => client.page(&url).await?,
    };
    let listing = read_listing(site.source, walk, &body)?;
    println!(
        "{}\ttotal {:?}\t{} rows\t{} unread",
        site.source,
        listing.total,
        listing.entries.len(),
        listing.unread.len()
    );
    for mut entry in listing.entries.into_iter().take(limit) {
        let line = match read_body(&client, site.source, &mut entry, None).await? {
            Read::Gone => "gone".to_string(),
            Read::Waiting => "waiting for the OCR".to_string(),
            Read::Scan { pages } => format!("scan, {pages} pages"),
            Read::Text { text, from_pdf } => {
                match parse_site_decision(site.source, &entry, &text, from_pdf) {
                    Ok(p) => format!(
                        "{}\t{}\t{} chars\t{}",
                        p.court.code,
                        p.decision.date_lecture.as_deref().unwrap_or_default(),
                        p.decision.texte_integral_clean.chars().count(),
                        p.source_fields["listing"]
                    ),
                    Err(e) => format!("refused: {e}"),
                }
            }
        };
        println!(
            "{}\t{}\t{}\t{line}",
            entry.id,
            entry.date.as_deref().unwrap_or("-"),
            entry.numbers.join(" | ")
        );
    }
    Ok(())
}
