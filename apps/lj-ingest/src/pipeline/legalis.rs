//! Legalis's decisions: each page of its sitemap fetched when new or modified since its last
//! visit, kept under `<state_dir>/ingest/legalis/html/`, then placed and loaded.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};

use lj_core::decision::Decision;
use lj_parse::{legalis_decision, legalis_source_uid, LegalisRefused};
use lj_redact_core::labels::{labelled, Labels, Needle};
use lj_redact_core::office::plain;
use lj_redact_core::{suggest, Anchor, Kind};
use lj_sources::legalis::{parse_page, Answer, LegalisClient};
use lj_store::repository::{DecisionRepository, Identity};

use crate::config::Settings;

use super::batch::drain_batch;
use super::europe::europe_pool;
use super::{
    content_checksum, extract_ctx, generate_public_id, Candidate, ExtractCtx, IngestCounts,
    IngestMode, WriteMode, BATCH_SIZE,
};

/// The daily run sends no request past this, 200 pages at the site's pace.
const BUDGET: Duration = Duration::from_secs(600);

/// A slug's state when its page was last fetched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Visit {
    modified: String,
    /// The post its page showed, `None` for a page gone or showing no decision.
    post: Option<u64>,
}

type Visits = BTreeMap<String, Visit>;

enum Verdict {
    Attach(&'static str),
    Copy,
    Create,
}

/// What reading decided for a post the address held.
static VERDICTS: LazyLock<HashMap<u64, Verdict>> = LazyLock::new(|| {
    include_str!("../../data/legalis_verdicts.tsv")
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let cols: Vec<&'static str> = l.split('\t').collect();
            let (post, verdict) = match cols[..] {
                [post, "attach", target] => (post, Verdict::Attach(target)),
                [post, "copy", _] => (post, Verdict::Copy),
                [post, "create"] => (post, Verdict::Create),
                _ => panic!("legalis_verdicts.tsv: malformed line {l:?}"),
            };
            let post = post
                .parse()
                .unwrap_or_else(|_| panic!("legalis_verdicts.tsv: post {post:?}"));
            (post, verdict)
        })
        .collect()
});

fn read_visits(path: &Path) -> Result<Visits> {
    match std::fs::read(path) {
        Ok(raw) => Ok(serde_json::from_slice(&raw)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Visits::new()),
        Err(e) => Err(e.into()),
    }
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let part = path.with_extension("part");
    std::fs::write(&part, bytes)?;
    std::fs::rename(&part, path)?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum Stop {
    Refused(u16),
    Budget,
    Limit,
}

/// What became of the pages read this run.
#[derive(Debug, Default)]
struct Tally {
    fetched: usize,
    gone: usize,
    no_decision: usize,
    refused: usize,
    unplaced: usize,
    copies: usize,
    /// Dry run: what the upsert would do.
    known: usize,
    created: usize,
    merged: usize,
    held: usize,
}

/// The court's code when the extraction places the decision on a court the referential holds.
pub(super) fn placed(
    decision: &Decision,
    ctx: &ExtractCtx,
    codes: &HashSet<&str>,
) -> Option<String> {
    let j = lj_extract::extract::jurisdiction_scanned(decision, None, &[])?;
    let code = lj_extract_rows::court_code(&j, &ctx.jur_codes, &ctx.jur_sources);
    codes.contains(code.as_str()).then_some(code)
}

/// The lines Legalis writes under a decision to name its bench and its counsel.
fn bench_or_counsel(line: &str) -> bool {
    line.split_once(':').is_some_and(|(head, _)| {
        matches!(
            lj_fold::slug(head, ' ').as_str(),
            "la cour" | "le tribunal" | "avocat" | "avocats"
        )
    })
}

/// The civilities a private person's name follows; a counsel's `Maître` is not one.
const CIVILITIES: [&str; 15] = [
    "m",
    "mm",
    "mme",
    "mmes",
    "mlle",
    "mlles",
    "monsieur",
    "messieurs",
    "madame",
    "mesdames",
    "mademoiselle",
    "dr",
    "docteur",
    "pr",
    "professeur",
];

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| lj_fold::slug(w, ' '))
        .collect()
}

fn initials(name: &[String]) -> bool {
    name.iter().all(|w| w.chars().count() == 1)
}

/// What the redaction rules find that Legalis left: a name a civility carries, out of the bench
/// and counsel lines, an address, an email, a phone, a birth. On 27 pages read, every name with
/// no civility was a heading, an acronym or a company, every identifier a company's or a mark's.
fn unmasked(text: &str, feed: &[(char, Anchor)]) -> Vec<(String, Kind)> {
    let body = words(text);
    let named: Vec<String> = text
        .lines()
        .filter(|l| bench_or_counsel(l))
        .flat_map(words)
        .collect();
    let suggestions = suggest(feed);
    let private = |name: &[String]| {
        !name.is_empty() && !initials(name) && !name.iter().all(|w| named.contains(w))
    };
    let carried: Vec<Vec<String>> = suggestions
        .iter()
        .filter(|s| s.kind == Kind::Person)
        .map(|s| words(&s.text))
        .filter(|name| {
            private(name)
                && body
                    .windows(name.len() + 1)
                    .any(|w| CIVILITIES.contains(&w[0].as_str()) && w[1..] == name[..])
        })
        .collect();
    suggestions
        .into_iter()
        .filter(|s| match s.kind {
            Kind::Docket | Kind::Identifier => false,
            Kind::Person => {
                let name = words(&s.text);
                private(&name) && carried.iter().any(|c| c.ends_with(&name))
            }
            _ => true,
        })
        .map(|s| (s.text, s.kind))
        .collect()
}

/// Legalis masks the parties; what the redaction rules still find is masked as the judicial open
/// data masks it.
pub(super) fn pseudonymised(text: &str) -> Result<String> {
    let feed = plain(text)?;
    let found = unmasked(text, &feed);
    if found.is_empty() {
        return Ok(text.to_string());
    }
    let needles: Vec<Needle> = found
        .iter()
        .map(|(text, kind)| Needle {
            text,
            kind: Some(*kind),
            label: None,
            held: false,
        })
        .collect();
    let labels = Labels::assign(&needles);
    let texts: Vec<String> = found.iter().map(|(t, _)| t.clone()).collect();
    labelled(&feed, &texts, &labels)
}

/// The candidate a kept page gives, `None` for one that is not loaded.
fn candidate(
    slug: &str,
    html: &str,
    ctx: &ExtractCtx,
    codes: &HashSet<&str>,
    tally: &mut Tally,
) -> Result<Option<Candidate>> {
    let Some(page) = parse_page(html) else {
        tally.no_decision += 1;
        return Ok(None);
    };
    let parsed = match legalis_decision(&page) {
        Ok(parsed) => parsed,
        Err(reason @ (LegalisRefused::NoDate | LegalisRefused::NoText)) => {
            tracing::info!(slug, ?reason, title = %page.title, "legalis page refused");
            tally.refused += 1;
            return Ok(None);
        }
    };
    let mut decision = parsed.decision;
    if placed(&decision, ctx, codes).is_none() {
        tracing::info!(slug, title = %page.title, "legalis decision waits: no court placed");
        tally.unplaced += 1;
        return Ok(None);
    }
    let text = pseudonymised(&decision.texte_integral_clean)?;
    decision.texte_integral_raw = text.clone();
    decision.texte_integral_clean = text;
    Ok(Some(Candidate {
        decision_id: None,
        public_id: generate_public_id(),
        content_checksum: content_checksum(format!("{}\n{}", page.title, page.text).as_bytes()),
        prebuilt_source_fields: Some(parsed.source_fields),
        prebuilt_extracted: None,
        decision,
        raw_payload: Vec::new(),
        payload_format: "html".to_string(),
        write_mode: WriteMode::Full,
        passages: true,
        distinct: false,
    }))
}

/// What the upsert would do with each candidate, written nowhere.
async fn foresee(
    repo: &DecisionRepository<'_>,
    ctx: &ExtractCtx,
    candidates: &[Candidate],
    tally: &mut Tally,
) -> Result<()> {
    for c in candidates {
        let uid = &c.decision.source_uid;
        if repo.find_provenance(uid).await?.is_some() {
            tally.known += 1;
            continue;
        }
        let extracted = lj_extract_rows::extracted_fields(&c.decision, &ctx.snapshots())?;
        let fields = c
            .prebuilt_source_fields
            .as_ref()
            .expect("a legalis candidate has its fields");
        let identity = match (c.decision_id, c.distinct) {
            (Some(id), _) => Identity::Attach(id),
            (None, true) => Identity::Create,
            (None, false) => {
                repo.identity_of(&c.decision, Some(&extracted), fields)
                    .await?
            }
        };
        let court = extracted.jurisdiction.as_ref().map(|j| j.code.as_str());
        let date = extracted.date_lecture;
        match identity {
            Identity::Attach(id) => {
                tally.merged += 1;
                tracing::info!(uid, court, ?date, joins = id, "legalis dry run: merged");
            }
            Identity::Create => {
                tally.created += 1;
                tracing::info!(uid, court, ?date, "legalis dry run: created");
            }
            Identity::Hold(id) | Identity::Copy(id) => {
                tally.held += 1;
                tracing::info!(uid, court, ?date, near = id, "legalis dry run: held");
            }
        }
    }
    Ok(())
}

/// Fetches the pages new or modified since their last visit, newest first, then loads them and
/// every kept page whose decision the base lacks; `dry_run` tells what the load would do.
pub async fn sync_legalis(full: bool, limit: Option<usize>, dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.state_dir.join("ingest/legalis");
    let pages = dir.join("html");
    std::fs::create_dir_all(&pages)?;
    let visits_path = dir.join("visits.json");
    let mut visits = read_visits(&visits_path)?;

    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    if !repo.legalis_lock().await? {
        tracing::info!("legalis sync: another run holds the lock");
        return Ok(());
    }
    let client = LegalisClient::new(&settings.egress_proxy)?;
    let started = Instant::now();
    let mut tally = Tally::default();
    let mut stop = None;
    let mut fetched: HashSet<String> = HashSet::new();

    match client.entries().await? {
        Answer::Refused(status) => stop = Some(Stop::Refused(status)),
        Answer::Page(mut entries) => {
            entries.sort_by(|a, b| b.modified.cmp(&a.modified));
            tracing::info!(
                entries = entries.len(),
                visited = visits.len(),
                full,
                "legalis sync: start"
            );
            for entry in entries {
                if visits
                    .get(&entry.slug)
                    .is_some_and(|v| v.modified == entry.modified)
                {
                    continue;
                }
                if !full && started.elapsed() >= BUDGET {
                    stop = Some(Stop::Budget);
                    break;
                }
                if limit.is_some_and(|l| tally.fetched >= l) {
                    stop = Some(Stop::Limit);
                    break;
                }
                let post = match client.page(&entry.slug).await? {
                    Answer::Refused(status) => {
                        stop = Some(Stop::Refused(status));
                        break;
                    }
                    Answer::Page(None) => {
                        tally.gone += 1;
                        None
                    }
                    Answer::Page(Some(html)) => {
                        tally.fetched += 1;
                        let path = pages.join(format!("{}.html", entry.slug));
                        write_atomically(&path, html.as_bytes())?;
                        fetched.insert(entry.slug.clone());
                        parse_page(&html).map(|p| p.post_id)
                    }
                };
                visits.insert(
                    entry.slug,
                    Visit {
                        modified: entry.modified,
                        post,
                    },
                );
                write_atomically(&visits_path, &serde_json::to_vec(&visits)?)?;
            }
        }
    }

    let posts: Vec<(&String, u64, String)> = visits
        .iter()
        .filter_map(|(slug, v)| v.post.map(|p| (slug, p, legalis_source_uid(p))))
        .collect();
    let uids: Vec<String> = posts.iter().map(|(_, _, uid)| uid.clone()).collect();
    let known = repo.known_source_uids(&uids).await?;
    let ctx = extract_ctx(&conn).await?;
    let codes: HashSet<&str> = ctx.jur_codes.values().map(String::as_str).collect();
    let mut candidates = Vec::new();
    for (slug, post, uid) in &posts {
        if !fetched.contains(*slug) && known.contains(uid) {
            continue;
        }
        let html = std::fs::read_to_string(pages.join(format!("{slug}.html")))?;
        let Some(mut cand) = candidate(slug, &html, ctx, &codes, &mut tally)? else {
            continue;
        };
        match VERDICTS.get(post) {
            None => {}
            Some(Verdict::Copy) => {
                tally.copies += 1;
                continue;
            }
            Some(Verdict::Create) => cand.distinct = true,
            Some(Verdict::Attach(public_id)) => {
                match repo.decision_id_by_public_id(public_id).await? {
                    Some(id) => cand.decision_id = Some(id),
                    None => {
                        tracing::warn!(uid, public_id, "verdict target gone, resolving the address")
                    }
                }
            }
        }
        candidates.push(cand);
    }

    let mut counts = IngestCounts::default();
    if dry_run {
        foresee(&repo, ctx, &candidates, &mut tally).await?;
    } else {
        let mut rest = candidates.into_iter().peekable();
        while rest.peek().is_some() {
            drain_batch(
                &conn,
                rest.by_ref().take(BATCH_SIZE).collect(),
                IngestMode::MissingHash,
                &mut counts,
            )
            .await?;
        }
    }
    tracing::info!(
        requests = client.sent(),
        fetched = tally.fetched,
        gone = tally.gone,
        no_decision = tally.no_decision,
        refused = tally.refused,
        unplaced = tally.unplaced,
        read_copies = tally.copies,
        dry_known = tally.known,
        dry_created = tally.created,
        dry_merged = tally.merged,
        dry_held = tally.held,
        created = counts.created,
        updated = counts.updated,
        skipped = counts.skipped,
        held = counts.held,
        errors = counts.errors,
        stop = ?stop,
        "legalis sync done"
    );
    if let Some(Stop::Refused(status)) = stop {
        bail!(
            "legalis answered {status} after {} requests, the next run resumes",
            client.sent()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verdict_table_reads() {
        assert!(!VERDICTS.is_empty());
    }

    #[test]
    fn a_name_is_masked_when_a_civility_carries_it_and_no_bench_line_names_it() {
        let text = "EXPOSE DU LITIGE\n\
            M. Jean Dupont reproche à la société ACME SAS la mise en ligne de sa photographie. \
            Dupont demande réparation. M. X. intervient, Me Paul Leroy plaide.\n\
            La cour : Mme Claire Martin, présidente\n\
            Mme Claire Martin a signé l'arrêt.";
        let feed = plain(text).unwrap();
        let found: Vec<String> = unmasked(text, &feed).into_iter().map(|(t, _)| t).collect();
        assert!(found.iter().any(|t| t == "Jean Dupont"), "{found:?}");
        assert!(found.iter().any(|t| t == "Dupont"), "{found:?}");
        for kept in ["EXPOSE", "ACME", "X", "Leroy", "Martin"] {
            assert!(
                !found.iter().any(|t| t.contains(kept)),
                "{kept} in {found:?}"
            );
        }
        let out = pseudonymised(text).unwrap();
        assert!(
            !out.contains("Dupont") && out.contains("Claire Martin"),
            "{out}"
        );
    }
}
