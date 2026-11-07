//! Sync incrémental Judilibre (HTTP async).

use super::calendar::{list_target_months, month_bounds};
use super::http::path_with_added_extension;
use super::manifest::{JurState, Manifest, MonthState};
use crate::error::{Result, SourceError};
use crate::judilibre::{extract_query_param, JudilibreClient};
use chrono::{Datelike, Utc};
use flate2::write::GzEncoder;
use flate2::Compression;
use futures::stream::{self, StreamExt};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

// ----------------------------------------------------------------------------
// Constantes Judilibre
// ----------------------------------------------------------------------------

use crate::state_paths::JUDILIBRE_DIR;
pub(super) const UNDATED_BUCKET: &str = "undated";
const ARCHIVE_KEY: &str = "archive";
const DEFAULT_BATCH_SIZE: i64 = 1000;
/// `/decision` calls in flight.
const FETCH_CONCURRENCY: usize = 16;
/// `/scan` calls in flight.
const LIST_CONCURRENCY: usize = 4;

/// Upper bound of a jurisdiction's one-shot `/scan` into `archive.jsonl.gz`.
#[derive(Clone, Copy)]
enum Archive {
    Until(&'static str),
    WholeStock,
}

/// `WholeStock` for a jurisdiction the taxonomy serves and neither table knows.
fn archive(jur: &str) -> Option<Archive> {
    match jur {
        "cc" => Some(Archive::Until("1986-10-31")),
        "ca" => Some(Archive::Until("2010-07-31")),
        _ if monthly_start(jur).is_some() => None,
        _ => Some(Archive::WholeStock),
    }
}

fn monthly_start(jur: &str) -> Option<&'static str> {
    match jur {
        "cc" => Some("1986-11-01"),
        "ca" => Some("2010-08-01"),
        "tj" => Some("2022-05-28"),
        "tcom" => Some("2024-11-15"),
        _ => None,
    }
}

/// `YYYYMM` extrait de `decision_date` ISO, sinon `UNDATED_BUCKET`.
fn yyyymm_of(decision: &Value) -> String {
    let raw = decision
        .get("decision_date")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let bytes = raw.as_bytes();
    if bytes.len() >= 7 && bytes[4] == b'-' {
        format!("{}{}", &raw[0..4], &raw[5..7])
    } else {
        UNDATED_BUCKET.to_string()
    }
}

// ----------------------------------------------------------------------------
// sync_judilibre (HTTP async)
// ----------------------------------------------------------------------------

/// One-shot `/scan` bootstrap per jurisdiction of the API taxonomy, then one
/// `/transactionalhistory` increment covering every jurisdiction, served or not
/// by the taxonomy. Append-only `.jsonl.gz`, resumed at the cursor.
pub async fn sync_judilibre(
    client: &JudilibreClient,
    data_dir: &Path,
    date_start_iso: &str,
) -> Result<Manifest> {
    let source_dir = data_dir.join(JUDILIBRE_DIR);
    fs::create_dir_all(&source_dir)?;
    let manifest_path = source_dir.join("manifest.json");
    let today = Utc::now().date_naive();

    let mut manifest = Manifest::load(&manifest_path)?;
    let mut bootstrap_started_at: Option<String> = None;

    for jur in client.jurisdictions().await? {
        let jur = jur.as_str();
        let mut state = manifest
            .jurisdictions
            .remove(jur)
            .unwrap_or_else(|| JurState::new(jur));

        let archive = archive(jur);
        let cutoff_yyyymm = match archive {
            Some(Archive::Until(c)) => Some(format!("{}{}", &c[0..4], &c[5..7])),
            _ => None,
        };

        let archive_needs_bootstrap = archive.is_some()
            && !state
                .months
                .get(ARCHIVE_KEY)
                .map(|m| m.bootstrapped)
                .unwrap_or(false);

        // Borne basse mensuelle : `date_start_iso` explicite sinon MONTHLY_START.
        let monthly_start_opt = if !date_start_iso.is_empty() {
            Some(date_start_iso.to_string())
        } else {
            monthly_start(jur).map(str::to_string)
        };
        let jur_target_months = monthly_start_opt
            .as_deref()
            .map(|s| list_target_months(s, today))
            .unwrap_or_default();
        let monthly_targets: Vec<String> = match &cutoff_yyyymm {
            Some(cut) => jur_target_months.into_iter().filter(|m| m > cut).collect(),
            None => jur_target_months,
        };

        let mut pending: Vec<String> = Vec::new();
        for yyyymm in &monthly_targets {
            let month = state.months.entry(yyyymm.clone()).or_default();
            if !month.bootstrapped {
                pending.push(yyyymm.clone());
            }
        }

        if (!pending.is_empty() || archive_needs_bootstrap) && bootstrap_started_at.is_none() {
            bootstrap_started_at = Some(server_now(client).await?);
        }

        if let Some(archive) = archive {
            let archive_done = state
                .months
                .get(ARCHIVE_KEY)
                .map(|m| m.bootstrapped)
                .unwrap_or(false);
            if !archive_done {
                bootstrap_archive(
                    client,
                    &source_dir,
                    jur,
                    archive,
                    &mut state,
                    &mut manifest,
                    &manifest_path,
                )
                .await?;
            }
        }

        for yyyymm in &pending {
            if let Err(e) = bootstrap_month(
                client,
                &source_dir,
                jur,
                yyyymm,
                &mut state,
                &mut manifest,
                &manifest_path,
            )
            .await
            {
                tracing::error!(jur, yyyymm, error = %e, "judilibre_month_failed");
            }
        }

        manifest.jurisdictions.insert(jur.to_string(), state);
        manifest.save(&manifest_path)?;
    }

    if manifest.history_watermark.is_none() {
        if let Some(seed) = &bootstrap_started_at {
            manifest.history_watermark = Some(seed.clone());
            manifest.save(&manifest_path)?;
        }
    }

    incremental_global(client, &source_dir, &mut manifest, &manifest_path).await?;

    manifest.save(&manifest_path)?;
    Ok(manifest)
}

/// Heure UTC serveur via `query_date` de `/transactionalhistory`.
async fn server_now(client: &JudilibreClient) -> Result<String> {
    let now = Manifest::now_iso_seconds();
    let probe = client.transactional_history(&now, None).await?;
    Ok(probe
        .get("query_date")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or(now))
}

fn update_max_update_date(month: &mut MonthState, decisions: &[Value]) {
    for d in decisions {
        if let Some(ud) = d.get("update_date").and_then(|v| v.as_str()) {
            if month
                .max_update_date
                .as_deref()
                .map(|cur| ud > cur)
                .unwrap_or(true)
            {
                month.max_update_date = Some(ud.to_string());
            }
        }
    }
}

/// Append des décisions dans `<jur>/<key>.jsonl.gz`.
pub(super) fn append_decisions(path: &Path, decisions: &[Value]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let mut encoder = GzEncoder::new(file, Compression::default());
    for d in decisions {
        let line = serde_json::to_string(d)
            .map_err(|e| SourceError::Invalid(format!("décision non sérialisable: {e}")))?;
        encoder.write_all(line.as_bytes())?;
        encoder.write_all(b"\n")?;
    }
    encoder.finish()?;
    Ok(())
}

/// Every line of a `.jsonl.gz`, without its `\n`.
pub fn read_jsonl_gz_lines(path: &Path) -> Result<Vec<Vec<u8>>> {
    use std::io::Read;
    let file = fs::File::open(path)?;
    let mut decoder = flate2::read::MultiGzDecoder::new(file);
    let mut content = Vec::new();
    decoder.read_to_end(&mut content)?;
    Ok(split_jsonl_lines(&content))
}

/// Splits JSONL content into trimmed lines, **without the empty pseudo-line** of
/// the final `\n`: `ingested_lines` stays a stable resume offset on an
/// append-only file.
fn split_jsonl_lines(content: &[u8]) -> Vec<Vec<u8>> {
    let content = content.strip_suffix(b"\n").unwrap_or(content);
    if content.is_empty() {
        return Vec::new();
    }
    content
        .split(|&b| b == b'\n')
        .map(|line| {
            let start = line.iter().position(|b| !b.is_ascii_whitespace());
            match start {
                None => Vec::new(),
                Some(s) => {
                    let end = line.iter().rposition(|b| !b.is_ascii_whitespace()).unwrap();
                    line[s..=end].to_vec()
                }
            }
        })
        .collect()
}

/// Parses one `.jsonl.gz` line, a Judilibre decision.
pub fn parse_decision_line(line: &[u8]) -> Result<Value> {
    Ok(sonic_rs::from_slice(line)?)
}

/// `tombstones.jsonl` read once and rewritten once per page, each line kept as
/// written.
pub(super) struct Tombstones {
    path: PathBuf,
    lines: Vec<(String, String)>,
    ids: HashSet<String>,
    dirty: bool,
}

impl Tombstones {
    pub(super) fn load(data_dir: &Path) -> Result<Self> {
        let path = data_dir.join("tombstones.jsonl");
        let mut lines = Vec::new();
        let mut ids = HashSet::new();
        if path.exists() {
            for line in fs::read_to_string(&path)?.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let id = serde_json::from_str::<Value>(line)
                    .ok()
                    .and_then(|v| v.get("id").and_then(Value::as_str).map(str::to_string))
                    .ok_or_else(|| {
                        SourceError::Invalid(format!("tombstones.jsonl: no id in {line}"))
                    })?;
                if ids.insert(id.clone()) {
                    lines.push((id, line.to_string()));
                }
            }
        }
        Ok(Self {
            path,
            lines,
            ids,
            dirty: false,
        })
    }

    #[cfg(test)]
    fn contains(&self, decision_id: &str) -> bool {
        self.ids.contains(decision_id)
    }

    /// Marks an id deleted; a second `deleted` for it adds nothing.
    pub(super) fn add(&mut self, decision_id: &str) {
        if self.ids.insert(decision_id.to_string()) {
            let line = serde_json::json!({
                "id": decision_id,
                "deleted_at": Manifest::now_iso_seconds(),
            });
            self.lines.push((decision_id.to_string(), line.to_string()));
            self.dirty = true;
        }
    }

    /// Revives an id Judilibre created or updated after deleting it.
    pub(super) fn remove(&mut self, decision_id: &str) {
        if self.ids.remove(decision_id) {
            self.lines.retain(|(id, _)| id != decision_id);
            self.dirty = true;
        }
    }

    pub(super) fn save(&mut self) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = String::new();
        for (_, line) in &self.lines {
            out.push_str(line);
            out.push('\n');
        }
        let tmp = path_with_added_extension(&self.path, "tmp");
        fs::write(&tmp, out.as_bytes())?;
        fs::rename(&tmp, &self.path)?;
        self.dirty = false;
        Ok(())
    }
}

/// Marks one id deleted in `data_dir/tombstones.jsonl`.
#[cfg(test)]
pub(super) fn append_tombstone(data_dir: &Path, decision_id: &str) -> Result<()> {
    let mut tombstones = Tombstones::load(data_dir)?;
    tombstones.add(decision_id);
    tombstones.save()
}

/// One-shot `/scan` into `archive.jsonl.gz`, bounded by `archive`.
async fn bootstrap_archive(
    client: &JudilibreClient,
    data_dir: &Path,
    jurisdiction: &str,
    archive: Archive,
    state: &mut JurState,
    manifest: &mut Manifest,
    manifest_path: &Path,
) -> Result<()> {
    let path = data_dir.join(jurisdiction).join("archive.jsonl.gz");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut month = state.months.remove(ARCHIVE_KEY).unwrap_or_default();

    if month.cursor.is_none() && month.decision_count == 0 {
        if path.exists() {
            fs::remove_file(&path)?;
        }
    } else if path.exists() && !is_valid_gzip(&path) {
        tracing::warn!(
            jur = jurisdiction,
            key = "archive",
            "judilibre_corrupt_reset"
        );
        fs::remove_file(&path)?;
        month.cursor = None;
        month.decision_count = 0;
    }

    loop {
        let mut params = vec![
            ("jurisdiction", jurisdiction.to_string()),
            ("date_type", "creation".to_string()),
            ("batch_size", DEFAULT_BATCH_SIZE.to_string()),
        ];
        if let Archive::Until(cutoff_date) = archive {
            params.push(("date_end", cutoff_date.to_string()));
        }
        if let Some(cursor) = &month.cursor {
            params.push(("searchAfter", cursor.clone()));
        }
        let page = client.scan(&params).await?;
        let results = page_results(&page);
        if !results.is_empty() {
            append_decisions(&path, &results)?;
            update_max_update_date(&mut month, &results);
            month.decision_count += results.len() as i64;
            state.total_decisions += results.len() as i64;
        }
        let next_cursor = extract_query_param(
            page.get("next_batch").and_then(|v| v.as_str()),
            "searchAfter",
        );
        month.cursor = next_cursor.clone();

        state.months.insert(ARCHIVE_KEY.to_string(), month.clone());
        manifest
            .jurisdictions
            .insert(jurisdiction.to_string(), state.clone());
        manifest.save(manifest_path)?;
        tracing::info!(
            jur = jurisdiction,
            got = results.len(),
            count = month.decision_count,
            "judilibre_archive"
        );
        if next_cursor.is_none() {
            break;
        }
    }

    month.bootstrapped = true;
    month.last_fetched_at = Some(Manifest::now_iso_seconds());
    state.months.insert(ARCHIVE_KEY.to_string(), month);
    Ok(())
}

/// Bootstrap d'un mois via `/scan`.
async fn bootstrap_month(
    client: &JudilibreClient,
    data_dir: &Path,
    jurisdiction: &str,
    yyyymm: &str,
    state: &mut JurState,
    manifest: &mut Manifest,
    manifest_path: &Path,
) -> Result<()> {
    let mut month = state.months.remove(yyyymm).unwrap_or_default();
    if yyyymm == UNDATED_BUCKET {
        tracing::info!(jur = jurisdiction, "judilibre_bootstrap_skip_undated");
        month.bootstrapped = true;
        state.months.insert(yyyymm.to_string(), month);
        return Ok(());
    }

    let (date_start, date_end) = month_bounds(yyyymm);
    let path = data_dir
        .join(jurisdiction)
        .join(format!("{yyyymm}.jsonl.gz"));
    if month.cursor.is_none() && month.decision_count == 0 {
        if path.exists() {
            fs::remove_file(&path)?;
        }
    } else if path.exists() && !is_valid_gzip(&path) {
        tracing::warn!(jur = jurisdiction, yyyymm, "judilibre_corrupt_reset");
        fs::remove_file(&path)?;
        month.cursor = None;
        month.decision_count = 0;
    }

    loop {
        let mut params = vec![
            ("jurisdiction", jurisdiction.to_string()),
            ("date_type", "creation".to_string()),
            ("date_start", date_start.clone()),
            ("date_end", date_end.clone()),
            ("batch_size", DEFAULT_BATCH_SIZE.to_string()),
        ];
        if let Some(cursor) = &month.cursor {
            params.push(("searchAfter", cursor.clone()));
        }
        let page = client.scan(&params).await?;
        let results = page_results(&page);
        if !results.is_empty() {
            append_decisions(&path, &results)?;
            update_max_update_date(&mut month, &results);
            month.decision_count += results.len() as i64;
            state.total_decisions += results.len() as i64;
        }
        let next_cursor = extract_query_param(
            page.get("next_batch").and_then(|v| v.as_str()),
            "searchAfter",
        );
        month.cursor = next_cursor.clone();

        state.months.insert(yyyymm.to_string(), month.clone());
        manifest
            .jurisdictions
            .insert(jurisdiction.to_string(), state.clone());
        manifest.save(manifest_path)?;
        tracing::info!(
            jur = jurisdiction,
            yyyymm,
            got = results.len(),
            count = month.decision_count,
            "judilibre_scan"
        );
        if next_cursor.is_none() {
            break;
        }
    }

    month.bootstrapped = true;
    month.last_fetched_at = Some(Manifest::now_iso_seconds());
    state.months.insert(yyyymm.to_string(), month);
    Ok(())
}

/// What one round of `/decision` calls left in the cache.
#[derive(Debug, Default)]
pub struct Fetched {
    pub written: usize,
    /// Ids whose call failed, to be asked again.
    pub failed: Vec<String>,
    /// Ids `/decision` answers 404 for.
    pub gone: Vec<String>,
}

/// Fetches `ids` through `/decision`, `FETCH_CONCURRENCY` calls in flight, and
/// appends each decision to `<jur>/<yyyymm>.jsonl.gz`, recording its month.
async fn fetch_into_cache(
    client: &JudilibreClient,
    data_dir: &Path,
    manifest: &mut Manifest,
    ids: &[String],
) -> Result<Fetched> {
    let responses: Vec<(&String, Result<Value>)> = stream::iter(ids)
        .map(|id| async move { (id, client.decision(id).await) })
        .buffered(FETCH_CONCURRENCY)
        .collect()
        .await;

    let mut fetched = Fetched::default();
    let mut groups: BTreeMap<(String, String), Vec<Value>> = BTreeMap::new();
    for (id, response) in responses {
        match response {
            Ok(decision) => {
                let jur = match decision.get("jurisdiction").and_then(|v| v.as_str()) {
                    Some(j) if !j.is_empty() => j.to_string(),
                    _ => {
                        tracing::warn!(id = %id, "judilibre_decision_without_jurisdiction");
                        continue;
                    }
                };
                let yyyymm = yyyymm_of(&decision);
                groups.entry((jur, yyyymm)).or_default().push(decision);
            }
            Err(SourceError::JudilibreApi { status: 404, .. }) => {
                tracing::warn!(id = %id, "judilibre_decision_gone");
                fetched.gone.push(id.clone());
            }
            Err(e) => {
                tracing::error!(id = %id, error = %e, "judilibre_decision_fetch_failed");
                fetched.failed.push(id.clone());
            }
        }
    }

    for ((jur, yyyymm), decisions) in &groups {
        let path = data_dir.join(jur).join(format!("{yyyymm}.jsonl.gz"));
        append_decisions(&path, decisions)?;
        let state = manifest
            .jurisdictions
            .entry(jur.clone())
            .or_insert_with(|| JurState::new(jur));
        let month = state
            .months
            .entry(yyyymm.clone())
            .or_insert_with(|| MonthState {
                bootstrapped: true,
                ..Default::default()
            });
        update_max_update_date(month, decisions);
        fetched.written += decisions.len();
    }
    Ok(fetched)
}

/// Process `/transactionalhistory` global multi-juridictions. The ids a
/// previous pass failed to fetch are asked first; an id that fails again waits
/// for the next pass, the cursor moving on without it.
async fn incremental_global(
    client: &JudilibreClient,
    data_dir: &Path,
    manifest: &mut Manifest,
    manifest_path: &Path,
) -> Result<()> {
    if manifest.history_watermark.is_none() && manifest.history_cursor.is_none() {
        return Ok(());
    }

    if !manifest.history_failed.is_empty() {
        let ids: Vec<String> = std::mem::take(&mut manifest.history_failed)
            .into_iter()
            .collect();
        let fetched = fetch_into_cache(client, data_dir, manifest, &ids).await?;
        manifest
            .history_failed
            .extend(fetched.failed.iter().cloned());
        manifest.save(manifest_path)?;
        tracing::warn!(
            retried = ids.len(),
            written = fetched.written,
            gone = fetched.gone.len(),
            still_failing = fetched.failed.len(),
            "judilibre_history_retry"
        );
    }

    let mut cursor = manifest.history_cursor.clone();
    // `query_date` est calculée une fois (borne basse formelle quand un cursor
    // pilote la pagination) et n'est plus réassignée dans la boucle (le cursor
    // `from_id` est primaire).
    let query_date = if cursor.is_some() {
        manifest
            .history_watermark
            .clone()
            .unwrap_or_else(|| "1970-01-01T00:00:00".to_string())
    } else {
        let wm = manifest
            .history_watermark
            .clone()
            .expect("watermark présent");
        minus_one_hour_iso(&wm)
    };

    let mut tombstones = Tombstones::load(data_dir)?;
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut written = 0usize;

    loop {
        let page = client
            .transactional_history(&query_date, cursor.as_deref())
            .await?;
        let ops: Vec<Value> = page
            .get("transactions")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let query_date_resp = page
            .get("query_date")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        if ops.is_empty() {
            if let Some(qd) = &query_date_resp {
                manifest.history_watermark = Some(qd.clone());
            }
            manifest.save(manifest_path)?;
            break;
        }

        let mut to_fetch: Vec<String> = Vec::new();
        for op in &ops {
            let decision_id = match op.get("id").and_then(|v| v.as_str()) {
                Some(id) if !id.is_empty() => id.to_string(),
                _ => continue,
            };
            let action = op
                .get("action")
                .or_else(|| op.get("type"))
                .or_else(|| op.get("operation"))
                .and_then(|v| v.as_str());
            // Tombstone last-action-wins : un `deleted` marque mort,
            // tout autre événement (created/updated) ressuscite en annulant la
            // tombstone. L'effet s'applique à CHAQUE occurrence — hors du dédup
            // `seen_ids` — pour couvrir delete↔create dans une même fenêtre. Les
            // ops étant chronologiques, le dernier événement de l'id l'emporte.
            // `seen_ids` ne dédup plus que les fetchs.
            if action == Some("deleted") {
                tombstones.add(&decision_id);
                continue;
            }
            tombstones.remove(&decision_id);
            if seen_ids.insert(decision_id.clone()) {
                to_fetch.push(decision_id);
            }
        }
        tombstones.save()?;

        let fetched = fetch_into_cache(client, data_dir, manifest, &to_fetch).await?;
        written += fetched.written;
        manifest.history_failed.extend(fetched.failed);

        let new_cursor =
            extract_query_param(page.get("next_page").and_then(|v| v.as_str()), "from_id");
        if let Some(nc) = &new_cursor {
            cursor = Some(nc.clone());
        }
        manifest.history_cursor = cursor.clone();
        if let Some(qd) = &query_date_resp {
            manifest.history_watermark = Some(qd.clone());
        }
        manifest.save(manifest_path)?;
        tracing::info!(
            ops = ops.len(),
            written,
            cursor = if new_cursor.is_some() {
                "advanced"
            } else {
                "kept"
            },
            "judilibre_history"
        );
        if new_cursor.is_none() {
            break;
        }
    }
    Ok(())
}

/// A decision-date window of `/scan`, both bounds included, as `YYYY-MM-DD`.
pub type Slice = (String, String);

/// One slice before 1950, then one per year, the last open-ended.
pub fn year_slices() -> Vec<Slice> {
    let this_year = Utc::now().date_naive().year();
    let mut years: Vec<Slice> = vec![("0001-01-01".into(), "1949-12-31".into())];
    years.extend((1950..=this_year).map(|y| {
        let end = if y == this_year {
            "9999-12-31".to_string()
        } else {
            format!("{y}-12-31")
        };
        (format!("{y}-01-01"), end)
    }));
    years
}

/// The twelve months of a year slice, December keeping the slice's end.
pub fn month_slices((start, end): &Slice) -> Vec<Slice> {
    let year = &start[0..4];
    (1..=12)
        .map(|month| {
            let (month_start, month_end) = month_bounds(&format!("{year}{month:02}"));
            let month_end = if month == 12 { end.clone() } else { month_end };
            (month_start, month_end)
        })
        .collect()
}

/// The `total` `/scan` announces for each slice of a court, `LIST_CONCURRENCY`
/// calls in flight.
pub async fn slice_totals(
    client: &JudilibreClient,
    jurisdiction: &str,
    slices: Vec<Slice>,
) -> Result<Vec<(Slice, usize)>> {
    stream::iter(slices)
        .map(|slice| async move {
            let n = slice_total(client, jurisdiction, Some(&slice)).await?;
            Ok::<_, SourceError>((slice, n))
        })
        .buffer_unordered(LIST_CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect()
}

/// The ids of these slices of a court, read through the abridged `/scan`.
pub async fn list_slices(
    client: &JudilibreClient,
    jurisdiction: &str,
    slices: Vec<Slice>,
) -> Result<Vec<String>> {
    let listed: Vec<Vec<String>> = stream::iter(slices)
        .map(|slice| async move { list_slice(client, jurisdiction, &slice).await })
        .buffer_unordered(LIST_CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<_>>()?;
    Ok(listed.into_iter().flatten().collect())
}

/// The `total` `/scan` announces for a whole court.
pub async fn court_total(client: &JudilibreClient, jurisdiction: &str) -> Result<usize> {
    slice_total(client, jurisdiction, None).await
}

/// The `total` `/scan` announces for a court, within `slice` when given.
async fn slice_total(
    client: &JudilibreClient,
    jurisdiction: &str,
    slice: Option<&Slice>,
) -> Result<usize> {
    let mut params = vec![
        ("jurisdiction", jurisdiction.to_string()),
        ("batch_size", "1".to_string()),
        ("abridged", "true".to_string()),
    ];
    if let Some((start, end)) = slice {
        params.push(("date_start", start.clone()));
        params.push(("date_end", end.clone()));
        params.push(("date_type", "creation".to_string()));
    }
    let page = client.scan(&params).await?;
    Ok(page.get("total").and_then(Value::as_u64).unwrap_or(0) as usize)
}

/// The ids of one slice, page after page. A slice that moves while it is read
/// can fall short of the `total` its first page announced.
async fn list_slice(
    client: &JudilibreClient,
    jurisdiction: &str,
    (start, end): &Slice,
) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    let mut cursor: Option<String> = None;
    let total = loop {
        let mut params = vec![
            ("jurisdiction", jurisdiction.to_string()),
            ("batch_size", DEFAULT_BATCH_SIZE.to_string()),
            ("abridged", "true".to_string()),
            ("date_start", start.clone()),
            ("date_end", end.clone()),
            ("date_type", "creation".to_string()),
        ];
        if let Some(c) = &cursor {
            params.push(("searchAfter", c.clone()));
        }
        let page = client.scan(&params).await?;
        let total = page.get("total").and_then(Value::as_u64).unwrap_or(0) as usize;
        let results = page_results(&page);
        ids.extend(
            results
                .iter()
                .filter_map(|d| d.get("id").and_then(Value::as_str))
                .map(str::to_string),
        );
        cursor = extract_query_param(
            page.get("next_batch").and_then(|v| v.as_str()),
            "searchAfter",
        );
        if cursor.is_none() || results.is_empty() {
            break total;
        }
    };
    if ids.len() != total {
        tracing::warn!(
            jurisdiction,
            start,
            end,
            listed = ids.len(),
            total,
            "judilibre_slice_listed_short"
        );
    }
    Ok(ids)
}

/// Fetches into the cache the ids the reconciliation found missing, lifting
/// their tombstones, and records them in `reconciled`.
pub async fn reconcile_fetch(
    client: &JudilibreClient,
    data_dir: &Path,
    ids: &[String],
) -> Result<Fetched> {
    let source_dir = data_dir.join(JUDILIBRE_DIR);
    let manifest_path = source_dir.join("manifest.json");
    let mut manifest = Manifest::load(&manifest_path)?;
    let mut tombstones = Tombstones::load(&source_dir)?;
    for id in ids {
        tombstones.remove(id);
    }
    tombstones.save()?;
    let fetched = fetch_into_cache(client, &source_dir, &mut manifest, ids).await?;
    manifest.reconciled.extend(
        ids.iter()
            .filter(|id| !fetched.failed.contains(id))
            .cloned(),
    );
    manifest.save(&manifest_path)?;
    Ok(fetched)
}

/// Tombstones `ids` once `/decision` answers 404 for each; returns those it
/// marked.
pub async fn reconcile_tombstone(
    client: &JudilibreClient,
    data_dir: &Path,
    ids: &[String],
) -> Result<Vec<String>> {
    let source_dir = data_dir.join(JUDILIBRE_DIR);
    let mut tombstones = Tombstones::load(&source_dir)?;
    let responses: Vec<(&String, Result<Value>)> = stream::iter(ids)
        .map(|id| async move { (id, client.decision(id).await) })
        .buffered(FETCH_CONCURRENCY)
        .collect()
        .await;
    let mut marked = Vec::new();
    for (id, response) in responses {
        match response {
            Err(SourceError::JudilibreApi { status: 404, .. }) => {
                tombstones.add(id);
                marked.push(id.clone());
            }
            Ok(_) => tracing::warn!(id = %id, "judilibre_reconcile_served_but_unlisted"),
            Err(e) => {
                tracing::error!(id = %id, error = %e, "judilibre_decision_fetch_failed")
            }
        }
    }
    tombstones.save()?;
    Ok(marked)
}

/// `results` d'une page `/scan` (liste, vide par défaut).
fn page_results(page: &Value) -> Vec<Value> {
    page.get("results")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

/// `dt - 1h` en ISO millisecondes.
fn minus_one_hour_iso(wm: &str) -> String {
    let normalized = wm.replace('Z', "+00:00");
    match chrono::DateTime::parse_from_rfc3339(&normalized) {
        Ok(dt) => (dt - chrono::Duration::hours(1))
            .format("%Y-%m-%dT%H:%M:%S%.3f%:z")
            .to_string(),
        Err(_) => "1970-01-01T00:00:00".to_string(),
    }
}

fn is_valid_gzip(path: &Path) -> bool {
    use std::io::Read;
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut decoder = flate2::read::MultiGzDecoder::new(file);
    let mut buf = [0u8; 65536];
    loop {
        match decoder.read(&mut buf) {
            Ok(0) => return true,
            Ok(_) => continue,
            Err(_) => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_jsonl_lines_count_is_append_stable() {
        assert_eq!(split_jsonl_lines(b"").len(), 0);
        assert_eq!(split_jsonl_lines(b"a\nb\n").len(), 2);
        assert_eq!(split_jsonl_lines(b"a\nb").len(), 2);
        let after_append = split_jsonl_lines(b"a\nb\nc\n");
        assert_eq!(after_append.len(), 3);
        assert_eq!(after_append[2], b"c");
        assert_eq!(split_jsonl_lines(b"a\n  \nb\n").len(), 3);
        assert_eq!(split_jsonl_lines(b"a\n  \nb\n")[1], b"");
    }

    #[test]
    fn yyyymm_of_decision() {
        let d = serde_json::json!({"decision_date": "2024-11-15"});
        assert_eq!(yyyymm_of(&d), "202411");
        let undated = serde_json::json!({"foo": "bar"});
        assert_eq!(yyyymm_of(&undated), "undated");
        // Format inattendu (pas de tiret en position 4) → undated.
        let weird = serde_json::json!({"decision_date": "20241115"});
        assert_eq!(yyyymm_of(&weird), "undated");
    }

    #[test]
    fn tombstone_append_idempotent_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        append_tombstone(dir.path(), "a").unwrap();
        append_tombstone(dir.path(), "a").unwrap(); // idempotent : pas de doublon
        append_tombstone(dir.path(), "b").unwrap();
        let path = dir.path().join("tombstones.jsonl");
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 2);

        let mut tombstones = Tombstones::load(dir.path()).unwrap();
        tombstones.remove("a"); // résurrection de "a"
        tombstones.remove("zzz"); // absent = no-op
        tombstones.save().unwrap();
        let reloaded = Tombstones::load(dir.path()).unwrap();
        assert!(!reloaded.contains("a"));
        assert!(reloaded.contains("b"));
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 1);
    }

    #[test]
    fn tombstone_revive_then_compact_keeps_record() {
        let dir = tempfile::tempdir().unwrap();
        let jur_dir = dir.path().join("cc");
        fs::create_dir_all(&jur_dir).unwrap();
        let path = jur_dir.join("202401.jsonl.gz");
        append_decisions(
            &path,
            &[serde_json::json!({"id": "d", "update_date": "2024-01-01T00:00:00Z"})],
        )
        .unwrap();

        append_tombstone(dir.path(), "d").unwrap(); // deleted
        let mut tombstones = Tombstones::load(dir.path()).unwrap();
        tombstones.remove("d"); // puis re-created
        tombstones.save().unwrap();

        let mut manifest = Manifest::default();
        manifest
            .jurisdictions
            .insert("cc".to_string(), JurState::new("cc"));
        super::super::compact::compact_archives(dir.path(), &manifest, 1).unwrap();

        use std::io::Read;
        let mut decoder = flate2::read::MultiGzDecoder::new(fs::File::open(&path).unwrap());
        let mut out = String::new();
        decoder.read_to_string(&mut out).unwrap();
        assert!(
            out.contains("\"d\""),
            "décision ressuscitée doit survivre au compact"
        );
    }
}
