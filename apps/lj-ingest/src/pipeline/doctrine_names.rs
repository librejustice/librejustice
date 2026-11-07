//! Job `sync-doctrine-names`: the usage names HAL's French law deposits write beside a reference.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::{anyhow, bail, Result};
use chrono::NaiveDate;
use clap::ValueEnum;

use lj_core::case_names::{case_name_key, SHORT_NAME_RARITY};
use lj_core::citation::name_words;
use lj_extract::doctrine::{self, DoctrineCitation, Evidence, NameRule};
use lj_sources::hal;
use lj_store::repository::{DecisionRepository, ElectedCaseName};

use crate::case_names::{add_short_keys, keeps};
use crate::config::Settings;

const SOURCE: &str = "doctrine";

const RULE: NameRule = NameRule {
    deposits: 2,
    text_confirms: true,
};

/// Consecutive transfer errors that stop the run: past them, the way out is down.
const ERRORS_IN_A_ROW: usize = 10;

#[derive(Copy, Clone, Debug, Default, ValueEnum)]
pub enum DoctrineNamesScan {
    /// The deposits submitted in the last 30 days: the weekly pass.
    #[default]
    Weekly,
    /// Every deposit, the unread ones in an order unrelated to their age.
    Stock,
}

pub async fn sync_doctrine_names(
    scan: DoctrineNamesScan,
    limit: Option<usize>,
    dry_run: bool,
) -> Result<()> {
    let settings = Settings::from_env()?;
    if settings.egress_proxy.is_empty() {
        bail!("LIBREJUSTICE_EGRESS_PROXY is empty: HAL is read through the egress proxy");
    }
    let client = hal::http_client_via(&settings.egress_proxy)?;
    let pool = lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    if !dry_run {
        lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
            .await
            .map_err(|e| anyhow!("migrations: {e}"))?;
    }
    let repo = DecisionRepository::new(&conn);
    let visited: HashSet<String> = match dry_run {
        true => HashSet::new(),
        false => repo.doctrine_visited().await?.into_iter().collect(),
    };

    let window = match scan {
        DoctrineNamesScan::Weekly => Some(30),
        DoctrineNamesScan::Stock => None,
    };
    let mut files = Vec::new();
    let mut cursor = hal::CURSOR_START.to_string();
    loop {
        let page = hal::scan_files(&client, window, &cursor).await?;
        files.extend(page.files);
        if page.next_cursor == cursor || files.len() >= page.total {
            break;
        }
        cursor = page.next_cursor;
        tokio::time::sleep(hal::THROTTLE).await;
    }
    let listed = files.len();
    let mut queue: Vec<hal::HalFile> = files
        .into_iter()
        .filter(|f| !visited.contains(&f.hal_id))
        .collect();
    queue.sort_by_key(|f| xxhash_rust::xxh3::xxh3_64(f.hal_id.as_bytes()));
    queue.truncate(limit.unwrap_or(usize::MAX));
    tracing::info!(listed, unread = queue.len(), "sync_doctrine_names started");

    let mut read: Vec<(String, Vec<DoctrineCitation>)> = Vec::new();
    let (mut unreadable, mut giving, mut in_a_row) = (0usize, 0usize, 0usize);
    for (i, file) in queue.iter().enumerate() {
        tokio::time::sleep(hal::THROTTLE).await;
        let bytes = match hal::fetch_file(&client, &file.url).await {
            Ok(bytes) => {
                in_a_row = 0;
                bytes
            }
            Err(e) => {
                in_a_row += 1;
                tracing::warn!(hal_id = file.hal_id, error = %e, "deposit not fetched");
                if in_a_row >= ERRORS_IN_A_ROW {
                    bail!("{in_a_row} deposits in a row not fetched, last: {e}");
                }
                continue;
            }
        };
        let citations = match bytes {
            Some(bytes) => tokio::task::spawn_blocking(move || {
                lj_sources::pdf::lj_pdf_extract(&bytes).map(|text| doctrine::read(&text))
            })
            .await?
            .ok(),
            None => None,
        };
        match &citations {
            None => unreadable += 1,
            Some(c) if !c.is_empty() => giving += 1,
            Some(_) => {}
        }
        if dry_run {
            let count = citations.as_ref().map_or(0, Vec::len);
            println!("deposit\t{}\t{}\t{count}", file.hal_id, file.url);
            read.extend(citations.map(|c| (file.hal_id.clone(), c)));
        } else {
            let value = citations.as_ref().map(serde_json::to_value).transpose()?;
            repo.put_doctrine_deposit(&file.hal_id, value.as_ref())
                .await?;
        }
        if i % 100 == 99 {
            tracing::info!(
                read = i + 1,
                of = queue.len(),
                giving,
                unreadable,
                "sync_doctrine_names reading"
            );
        }
    }

    let deposits: Vec<(String, Vec<DoctrineCitation>)> = match dry_run {
        true => read,
        false => repo
            .doctrine_citations()
            .await?
            .into_iter()
            .map(|(id, v)| Ok((id, serde_json::from_value(v)?)))
            .collect::<Result<_>>()?,
    };
    let evidence = anchor(&repo, &deposits).await?;
    let (evidence, common) = drop_common_words(&repo, evidence).await?;
    let mut elected = elect(&repo, &evidence).await?;
    let shorts = add_short_keys(&repo, &mut elected).await?;
    let elected: Vec<ElectedCaseName> = elected.into_values().collect();
    let names = elected.iter().filter(|e| e.name.is_some()).count();
    if dry_run {
        return report(&repo, &elected).await;
    }
    let unkeyed = repo.drop_case_name_keys(SOURCE, &common).await?;
    let changed = repo.put_case_names(&elected).await?;
    let kept: Vec<i64> = elected.iter().map(|e| e.decision_id).collect();
    let cleared = repo.clear_case_names_except(&[SOURCE], &kept).await?;
    if matches!(scan, DoctrineNamesScan::Stock) && !queue.is_empty() {
        tracing::warn!(
            read = queue.len(),
            giving,
            "the doctrine catch-up read deposits the weekly pass had not"
        );
    }
    println!(
        "doctrine-names: {listed} deposits listed, {} read of which {giving} gave a name and \
         {unreadable} had no readable PDF, {} deposits naming, {} anchored names, {} decisions \
         keyed of which {names} named, {changed} changed and {} cleared, {} names of one common \
         word refused and taken out of {unkeyed} decisions, {shorts} short keys too common",
        queue.len(),
        deposits.len(),
        evidence.len(),
        elected.len(),
        cleared.len(),
        common.len(),
    );
    Ok(())
}

/// Refuses a name of one word too common to name a case: « décision tacite », « le présent
/// arrêt sera notifié », « CAA Paris ». Returns the evidence left, and the `(decision, key)`
/// pairs refused, which an earlier run may have written.
async fn drop_common_words(
    repo: &DecisionRepository<'_>,
    evidence: Vec<Evidence>,
) -> Result<(Vec<Evidence>, Vec<(i64, String)>)> {
    let words: BTreeSet<String> = evidence
        .iter()
        .map(|e| case_name_key(&e.name))
        .filter(|k| !k.is_empty() && !k.contains(' '))
        .collect();
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let common: HashSet<String> = repo
        .common_name_words(&words, SHORT_NAME_RARITY)
        .await?
        .into_iter()
        .collect();
    let (refused, kept): (Vec<Evidence>, Vec<Evidence>) = evidence
        .into_iter()
        .partition(|e| common.contains(&case_name_key(&e.name)));
    let pairs: BTreeSet<(i64, String)> = refused
        .iter()
        .map(|e| (e.decision, case_name_key(&e.name)))
        .collect();
    Ok((kept, pairs.into_iter().collect()))
}

async fn anchor(
    repo: &DecisionRepository<'_>,
    deposits: &[(String, Vec<DoctrineCitation>)],
) -> Result<Vec<Evidence>> {
    let mut anchors: BTreeSet<(&str, NaiveDate, &str)> = BTreeSet::new();
    for c in deposits.iter().flat_map(|(_, c)| c) {
        anchors.insert((c.court.as_str(), c.date.parse()?, c.docket.as_str()));
    }
    let anchors: Vec<(&str, NaiveDate, &str)> = anchors.into_iter().collect();
    let mut resolved: HashMap<(&str, NaiveDate, &str), i64> = HashMap::new();
    for chunk in anchors.chunks(10_000) {
        let jurs: Vec<&str> = chunk.iter().map(|a| a.0).collect();
        let dates: Vec<NaiveDate> = chunk.iter().map(|a| a.1).collect();
        let dockets: Vec<&str> = chunk.iter().map(|a| a.2).collect();
        let ids = repo.resolve_docket_anchors(&jurs, &dates, &dockets).await?;
        resolved.extend(chunk.iter().zip(ids).filter_map(|(a, id)| Some((*a, id?))));
    }
    let mut evidence = Vec::new();
    for (hal_id, citations) in deposits {
        for c in citations {
            if let Some(id) = resolved.get(&(c.court.as_str(), c.date.parse()?, c.docket.as_str()))
            {
                evidence.push(Evidence {
                    deposit: hal_id.clone(),
                    name: c.name.clone(),
                    decision: *id,
                });
            }
        }
    }
    Ok(evidence)
}

async fn elect(
    repo: &DecisionRepository<'_>,
    evidence: &[Evidence],
) -> Result<BTreeMap<i64, ElectedCaseName>> {
    let ids: Vec<i64> = evidence
        .iter()
        .map(|e| e.decision)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let heads: HashMap<i64, HashSet<String>> = repo
        .decision_heads(&ids)
        .await?
        .into_iter()
        .map(|(id, _, head)| {
            (
                id,
                case_name_key(&head)
                    .split(' ')
                    .map(str::to_string)
                    .collect(),
            )
        })
        .collect();
    let confirmed = |d: i64, key: &str| -> bool {
        let words = name_words(key);
        !words.is_empty()
            && heads
                .get(&d)
                .is_some_and(|h| words.iter().all(|w| h.contains(*w)))
    };
    let current: HashMap<i64, (String, String)> = repo
        .current_case_names(&ids)
        .await?
        .into_iter()
        .filter_map(|(id, name, source)| Some((id, (name?, source?))))
        .collect();
    Ok(doctrine::elect(evidence, RULE, &confirmed)
        .into_iter()
        .map(|e| {
            let name = e.name.filter(|n| match current.get(&e.decision) {
                Some((name, source)) => !keeps(name, source, n, SOURCE),
                None => true,
            });
            (
                e.decision,
                ElectedCaseName {
                    decision_id: e.decision,
                    name,
                    keys: e.keys,
                    source: SOURCE,
                },
            )
        })
        .collect())
}

async fn report(repo: &DecisionRepository<'_>, elected: &[ElectedCaseName]) -> Result<()> {
    let ids: Vec<i64> = elected.iter().map(|e| e.decision_id).collect();
    let public: HashMap<i64, String> = repo
        .decision_heads(&ids)
        .await?
        .into_iter()
        .map(|(id, public, _)| (id, public))
        .collect();
    let current: HashMap<i64, (Option<String>, Option<String>)> = repo
        .current_case_names(&ids)
        .await?
        .into_iter()
        .map(|(id, name, source)| (id, (name, source)))
        .collect();
    let (mut new, mut changed, mut kept, mut keys_alone) = (0, 0, 0, 0);
    for e in elected {
        let (now, source) = current.get(&e.decision_id).cloned().unwrap_or((None, None));
        let outranked = matches!(source.as_deref(), Some("source" | "wikimedia" | "ce-list"));
        let what = match (&e.name, &now) {
            (Some(_), Some(_)) if outranked => {
                kept += 1;
                "kept"
            }
            (Some(n), Some(now)) if case_name_key(n) != case_name_key(now) => {
                changed += 1;
                "changed"
            }
            (Some(_), Some(_)) => {
                kept += 1;
                "kept"
            }
            (Some(_), None) => {
                new += 1;
                "new"
            }
            (None, _) => {
                keys_alone += 1;
                "keys"
            }
        };
        println!(
            "{what}\t{}\t{}\t{}\t{}\t{}",
            public.get(&e.decision_id).map_or("", String::as_str),
            now.as_deref().unwrap_or(""),
            source.as_deref().unwrap_or(""),
            e.name.as_deref().unwrap_or(""),
            e.keys.join(" | "),
        );
    }
    println!(
        "doctrine-names dry run: {} decisions, {new} new names, {changed} changed, {kept} kept, \
         {keys_alone} keys alone",
        elected.len()
    );
    Ok(())
}
