//! BODACC and JOAFE from the DILA bulk: each container not yet written, or changed since, as rows.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::Instant;

use anyhow::{anyhow, bail, Result};
use lj_gazette::census::{self, Comparison, Inspection};
use lj_gazette::record::Record;
use lj_gazette::rows::{self, Codes, Doc, Parsed, Rows};
use lj_gazette::{dict, Source};
use lj_sources::error::SourceError;
use lj_sources::gazette::{for_each_container, sync, Container, Scope};
use lj_store::repository::DecisionRepository;
use rayon::prelude::*;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::config::Settings;

const BATCH_BYTES: usize = 128 << 20;
const READ_IDS: usize = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GazetteMode {
    Import,
    DryRun,
    Verify,
    Reclass,
}

pub fn parse_gazette_source(s: &str) -> Result<Source, String> {
    match s {
        "bodacc" => Ok(Source::Bodacc),
        "joafe" => Ok(Source::Joafe),
        other => Err(format!("{other}: bodacc or joafe")),
    }
}

struct Item {
    parsed: Parsed,
    raw: Option<Vec<Vec<u8>>>,
    inspection: Option<Inspection>,
}

fn item(source: Source, mode: GazetteMode, c: Container) -> Result<Item, String> {
    let parsed = rows::parse_container(source, c.origin, &c.docs).map_err(|e| e.to_string())?;
    Ok(match mode {
        GazetteMode::Import | GazetteMode::Reclass => Item {
            parsed,
            raw: None,
            inspection: None,
        },
        GazetteMode::DryRun => {
            let inspection = census::inspect(source, &c.docs, &parsed)
                .map_err(|e| format!("{}: {e}", parsed.origin))?;
            Item {
                parsed,
                raw: None,
                inspection: Some(inspection),
            }
        }
        GazetteMode::Verify => Item {
            parsed,
            raw: Some(c.docs),
            inspection: None,
        },
    })
}

struct Producer {
    source: Source,
    mode: GazetteMode,
    known: HashMap<String, i64>,
    pending: Vec<Container>,
    bytes: usize,
    unchanged: usize,
    unreadable: Vec<String>,
    tx: mpsc::Sender<Vec<Item>>,
}

impl Producer {
    fn push(&mut self, c: Container) -> Result<()> {
        if self.known.get(&c.origin) == Some(&rows::checksum(&c.docs)) {
            self.unchanged += 1;
            return Ok(());
        }
        self.bytes += c.docs.iter().map(Vec::len).sum::<usize>();
        self.pending.push(c);
        if self.bytes >= BATCH_BYTES {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        self.bytes = 0;
        let (source, mode) = (self.source, self.mode);
        let results: Vec<Result<Item, String>> = self
            .pending
            .par_drain(..)
            .map(|c| item(source, mode, c))
            .collect();
        let mut batch = Vec::with_capacity(results.len());
        for r in results {
            match r {
                Ok(i) => batch.push(i),
                Err(e) if mode == GazetteMode::DryRun => {
                    warn!(error = %e, "gazette container unreadable");
                    self.unreadable.push(e);
                }
                Err(e) => bail!(e),
            }
        }
        self.tx
            .blocking_send(batch)
            .map_err(|_| anyhow!("gazette writer stopped"))
    }
}

type Produced = JoinHandle<Result<(usize, Vec<String>)>>;

/// Reads and parses on its own threads, a batch ahead of the writer.
fn produce(
    source: Source,
    dir: PathBuf,
    scope: Scope,
    mode: GazetteMode,
    known: HashMap<String, i64>,
) -> (Produced, mpsc::Receiver<Vec<Item>>) {
    let (tx, rx) = mpsc::channel(2);
    let handle = std::thread::spawn(move || {
        let mut p = Producer {
            source,
            mode,
            known,
            pending: Vec::new(),
            bytes: 0,
            unchanged: 0,
            unreadable: Vec::new(),
            tx,
        };
        for_each_container(source, &dir, scope, |c| {
            p.push(c).map_err(|e| SourceError::Invalid(e.to_string()))
        })?;
        p.flush()?;
        Ok((p.unchanged, p.unreadable))
    });
    (handle, rx)
}

fn finished(handle: Produced) -> Result<(usize, Vec<String>)> {
    handle
        .join()
        .map_err(|_| anyhow!("gazette reader panicked"))?
}

pub async fn gazette(source: Source, stock: bool, offline: bool, mode: GazetteMode) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.cache_dir().join(source.slug());
    let scope = if stock { Scope::Stock } else { Scope::Current };
    if !offline {
        let d = dir.clone();
        let report = tokio::task::spawn_blocking(move || sync(source, &d, scope)).await??;
        info!(
            source = source.slug(),
            listed = report.listed,
            downloaded = report.downloaded,
            bytes = report.bytes,
            forbidden = report.forbidden,
            "gazette mirror synced"
        );
    }
    match mode {
        GazetteMode::DryRun => dry_run(source, dir, scope).await,
        GazetteMode::Import | GazetteMode::Verify | GazetteMode::Reclass => {
            let pool = lj_pg::db::build_pool(&settings.db_url, 1)
                .map_err(|e| anyhow!("build_pool: {e}"))?;
            let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
            lj_store::migrator::apply_migrations(
                &conn,
                lj_store::migrator::MigrationIntent::Verify,
            )
            .await
            .map_err(|e| anyhow!("migrations: {e}"))?;
            conn.batch_execute("SET statement_timeout = 0")
                .await
                .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
            let repo = DecisionRepository::new(&conn);
            let mut codes = Codes::new(repo.gazette_last_address().await?);
            for (kind, code, value) in repo.gazette_dicts().await? {
                codes.load(kind, code, value)?;
            }
            match mode {
                GazetteMode::Import => import(&repo, codes, source, dir, scope).await,
                GazetteMode::Verify => verify(&repo, codes, source, dir, scope).await,
                _ => reclass(&repo, source, dir, scope).await,
            }
        }
    }
}

async fn import(
    repo: &DecisionRepository<'_>,
    mut codes: Codes,
    source: Source,
    dir: PathBuf,
    scope: Scope,
) -> Result<()> {
    if !repo.gazette_lock().await? {
        bail!("another gazette import holds the lock");
    }
    let known = repo.gazette_files(source).await?;
    let (handle, mut rx) = produce(source, dir, scope, GazetteMode::Import, known);
    let started = Instant::now();
    let (mut containers, mut notices, mut officers, mut addresses, mut repeated) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    while let Some(batch) = rx.recv().await {
        let mut keys: Vec<i64> = batch
            .iter()
            .flat_map(|i| rows::address_keys(&i.parsed))
            .filter(|k| !codes.knows_address(*k))
            .collect();
        keys.sort_unstable();
        keys.dedup();
        for chunk in keys.chunks(READ_IDS) {
            for (key, id) in repo.gazette_address_keys(chunk).await? {
                codes.learn_address(key, id);
            }
        }
        let mut rows = Rows::default();
        for item in batch {
            containers += 1;
            codes.container(source, item.parsed, &mut rows)?;
        }
        rows.finish(&mut codes);
        repo.gazette_write(source, &rows).await?;
        notices += rows.notices.len();
        officers += rows.officers.len();
        addresses += rows.addresses.len();
        repeated += rows.repeated;
        info!(
            source = source.slug(),
            containers,
            notices,
            officers,
            addresses,
            secs = started.elapsed().as_secs(),
            "gazette batch written"
        );
    }
    let (unchanged, _) = finished(handle)?;
    if containers > 0 {
        repo.gazette_refresh_counts(source).await?;
    }
    if repeated > 0 {
        warn!(
            source = source.slug(),
            repeated, "gazette notices met twice in a batch, the last kept"
        );
    }
    info!(
        source = source.slug(),
        containers,
        unchanged,
        notices,
        officers,
        addresses,
        secs = started.elapsed().as_secs(),
        "gazette import done"
    );
    Ok(())
}

/// A notice met in several containers takes its classes from the last, as the import stores it.
async fn reclass(
    repo: &DecisionRepository<'_>,
    source: Source,
    dir: PathBuf,
    scope: Scope,
) -> Result<()> {
    if !repo.gazette_lock().await? {
        bail!("another gazette import holds the lock");
    }
    let table = source.classes();
    let (handle, mut rx) = produce(source, dir, scope, GazetteMode::Reclass, HashMap::new());
    let started = Instant::now();
    let (mut notices, mut rewritten) = (0usize, 0u64);
    while let Some(batch) = rx.recv().await {
        let mut fresh: HashMap<i64, rows::Row> = HashMap::new();
        for item in &batch {
            for row in rows::class_rows(&item.parsed) {
                let rows::Value::Long(id) = row[0] else {
                    bail!("a class row starts with its id");
                };
                fresh.insert(id, row);
            }
        }
        let ids: Vec<i64> = fresh.keys().copied().collect();
        let mut changed = Vec::new();
        for chunk in ids.chunks(READ_IDS) {
            for stored in repo.gazette_rows(&table, chunk).await? {
                let rows::Value::Long(id) = stored[0] else {
                    bail!("a class row starts with its id");
                };
                if fresh.get(&id).is_some_and(|row| *row != stored) {
                    changed.extend(fresh.remove(&id));
                }
            }
        }
        notices += ids.len();
        rewritten += repo.gazette_update(&table, &changed).await?;
        info!(
            source = source.slug(),
            notices,
            rewritten,
            secs = started.elapsed().as_secs(),
            "gazette reclass batch"
        );
    }
    finished(handle)?;
    repo.gazette_refresh_counts(source).await?;
    if rewritten > 0 {
        warn!(
            source = source.slug(),
            notices, rewritten, "gazette notices reclassed"
        );
    }
    info!(
        source = source.slug(),
        notices,
        rewritten,
        secs = started.elapsed().as_secs(),
        "gazette reclass done"
    );
    Ok(())
}

fn header_record(
    codes: &Codes,
    header: Option<&(Option<i32>, Option<String>)>,
) -> Result<Option<Record>> {
    let Some((Some(shape), rest)) = header else {
        return Ok(None);
    };
    Ok(Some(Record {
        shape: codes.value(dict::HEADER_SHAPE, *shape)?.to_string(),
        cells: Vec::new(),
        rest: rest.clone(),
    }))
}

/// A notice met in several containers is stored as last met: only its last occurrence must come back whole.
async fn verify(
    repo: &DecisionRepository<'_>,
    codes: Codes,
    source: Source,
    dir: PathBuf,
    scope: Scope,
) -> Result<()> {
    let headers = repo.gazette_headers(source).await?;
    let layout = source.layout();
    let (handle, mut rx) = produce(source, dir, scope, GazetteMode::Verify, HashMap::new());
    let started = Instant::now();
    let mut last: HashMap<i64, bool> = HashMap::new();
    let mut headers_differing: Vec<String> = Vec::new();
    let mut occurrences = 0usize;
    while let Some(batch) = rx.recv().await {
        let ids: Vec<i64> = batch
            .iter()
            .flat_map(|i| {
                i.parsed
                    .docs
                    .iter()
                    .flat_map(Doc::records)
                    .map(|(id, _)| id)
            })
            .collect();
        let mut stored = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(READ_IDS) {
            stored.extend(repo.gazette_rows(&layout.table, chunk).await?);
        }
        let mut address_ids: Vec<i32> = stored.iter().flat_map(|r| layout.address_ids(r)).collect();
        address_ids.sort_unstable();
        address_ids.dedup();
        let mut addresses: HashMap<i32, Record> = HashMap::with_capacity(address_ids.len());
        for chunk in address_ids.chunks(READ_IDS) {
            for row in repo.gazette_address_rows(chunk).await? {
                let (id, rec) = rows::ADDRESS.record(&row, &codes, &HashMap::new())?;
                addresses.insert(id as i32, rec);
            }
        }
        let records: HashMap<i64, Record> = stored
            .iter()
            .map(|r| layout.record(r, &codes, &addresses))
            .collect::<Result<_, _>>()?;
        for item in batch {
            let header = header_record(&codes, headers.get(&item.parsed.origin))?;
            let raw = item.raw.expect("verify keeps the source documents");
            let c = census::compare(
                source,
                &raw,
                &item.parsed,
                &|id| records.get(&id).cloned(),
                header.as_ref(),
            )?;
            occurrences += c.notices;
            let bad: std::collections::HashSet<i64> =
                c.missing.iter().chain(&c.differing).copied().collect();
            for doc in &item.parsed.docs {
                for (id, _) in doc.records() {
                    last.insert(id, !bad.contains(&id));
                }
            }
            headers_differing.extend(c.headers_differing);
        }
        info!(
            source = source.slug(),
            occurrences,
            notices = last.len(),
            secs = started.elapsed().as_secs(),
            "gazette verify batch"
        );
    }
    finished(handle)?;
    let mut failed: Vec<i64> = last
        .iter()
        .filter(|(_, ok)| !**ok)
        .map(|(id, _)| *id)
        .collect();
    failed.sort_unstable();
    if !failed.is_empty() || !headers_differing.is_empty() {
        bail!(
            "{}: {} notices, {} not given back whole (first {:?}), {} headers differing (first {:?})",
            source.slug(),
            last.len(),
            failed.len(),
            &failed[..failed.len().min(10)],
            headers_differing.len(),
            &headers_differing[..headers_differing.len().min(5)]
        );
    }
    info!(
        source = source.slug(),
        notices = last.len(),
        occurrences,
        secs = started.elapsed().as_secs(),
        "gazette verify: every notice comes back whole"
    );
    Ok(())
}

fn doc_year_kind(doc: &Doc) -> (String, i16) {
    let (date, kind) = match doc {
        Doc::Bodacc(f) => (f.date, f.kind as i16),
        Doc::Joafe(d) => (d.date, 0),
    };
    (lj_gazette::record::format_date(date)[..4].to_string(), kind)
}

async fn dry_run(source: Source, dir: PathBuf, scope: Scope) -> Result<()> {
    let (handle, mut rx) = produce(source, dir, scope, GazetteMode::DryRun, HashMap::new());
    let started = Instant::now();
    let mut codes = Codes::new(0);
    let mut total = Comparison::default();
    let mut rest: HashMap<String, (u64, u64, Vec<String>)> = HashMap::new();
    let mut seen: HashMap<i64, i64> = HashMap::new();
    let (mut same, mut different) = (0usize, 0usize);
    let mut different_ids: Vec<i64> = Vec::new();
    let mut by_year: BTreeMap<(String, i16), u64> = BTreeMap::new();
    let (mut containers, mut officers) = (0usize, 0usize);
    while let Some(batch) = rx.recv().await {
        let mut rows = Rows::default();
        for item in batch {
            containers += 1;
            let inspection = item.inspection.expect("a dry run inspects");
            total.add(inspection.comparison);
            for (path, (n, b, examples)) in inspection.rest {
                let e = rest.entry(path).or_default();
                e.0 += n;
                e.1 += b;
                if e.2.len() < 3 {
                    e.2.extend(examples.into_iter().take(3 - e.2.len()));
                }
            }
            for doc in &item.parsed.docs {
                *by_year.entry(doc_year_kind(doc)).or_default() += doc.notices() as u64;
                for (id, rec) in doc.records() {
                    let key = rows::address_key(rec);
                    match seen.insert(id, key) {
                        Some(prev) if prev == key => same += 1,
                        Some(_) => {
                            different += 1;
                            if different_ids.len() < 20 {
                                different_ids.push(id);
                            }
                        }
                        None => {}
                    }
                }
            }
            codes.container(source, item.parsed, &mut rows)?;
        }
        rows.finish(&mut codes);
        officers += rows.officers.len();
        info!(
            source = source.slug(),
            containers,
            notices = total.notices,
            differing = total.differing.len(),
            secs = started.elapsed().as_secs(),
            "gazette dry run batch"
        );
    }
    let (_, unreadable) = finished(handle)?;
    println!(
        "{} dry run: {containers} containers, {} unreadable, {} s",
        source.slug(),
        unreadable.len(),
        started.elapsed().as_secs()
    );
    for e in unreadable.iter().take(30) {
        println!("  unreadable: {e}");
    }
    println!(
        "notices {}, distinct ids {}, met again identical {same}, met again different {different} {different_ids:?}",
        total.notices,
        seen.len()
    );
    println!(
        "round trip: {} missing, {} differing {:?}, {} headers differing {:?}",
        total.missing.len(),
        total.differing.len(),
        total.differing.iter().take(20).collect::<Vec<_>>(),
        total.headers_differing.len(),
        total.headers_differing.iter().take(10).collect::<Vec<_>>()
    );
    println!("officers {officers}");
    let kinds: &[(&str, i16)] = &[
        ("shape", dict::SHAPE),
        ("address_shape", dict::ADDRESS_SHAPE),
        ("header_shape", dict::HEADER_SHAPE),
        ("role", dict::ROLE),
        ("dept", dict::DEPT),
        ("tribunal", dict::TRIBUNAL),
        ("greffe", dict::GREFFE),
        ("forme", dict::FORME),
        ("jug_famille", dict::JUG_FAMILLE),
        ("jug_nature", dict::JUG_NATURE),
        ("category", dict::CATEGORY),
        ("origin", dict::ORIGIN),
        ("qualite", dict::QUALITE),
        ("modif", dict::MODIF),
        ("radiation", dict::RADIATION),
        ("depot_type", dict::DEPOT_TYPE),
        ("depot_desc", dict::DEPOT_DESC),
        ("code_rcs", dict::CODE_RCS),
        ("devise", dict::DEVISE),
        ("joafe_type", dict::JOAFE_TYPE),
        ("joafe_dept", dict::JOAFE_DEPT),
        ("joafe_theme", dict::JOAFE_THEME),
        ("joafe_lieu", dict::JOAFE_LIEU),
    ];
    for (name, kind) in kinds {
        let n = codes.dictionary_len(*kind);
        if n > 0 {
            println!("dictionary {name}: {n}");
        }
    }
    println!("addresses {}", codes.address_count());
    let mut paths: Vec<_> = rest.into_iter().collect();
    paths.sort_by_key(|p| std::cmp::Reverse(p.1 .1));
    let rest_bytes: u64 = paths.iter().map(|p| p.1 .1).sum();
    println!("rest {rest_bytes} bytes over {} paths", paths.len());
    for (path, (n, b, examples)) in paths.iter().take(80) {
        println!("  {b:>14} {n:>11}  {path}  {examples:?}");
    }
    for ((year, kind), n) in &by_year {
        println!("  {year} kind {kind}: {n}");
    }
    Ok(())
}
