//! Sync incrémental Judilibre (port de `sources/judilibre/downloader.py`, HTTP
//! async, mono-thread).

use super::calendar::{list_target_months, month_bounds};
use super::http::path_with_added_extension;
use super::manifest::{JurState, Manifest, MonthState};
use crate::error::{Result, SourceError};
use crate::judilibre::{extract_query_param, JudilibreClient};
use chrono::Utc;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

// ----------------------------------------------------------------------------
// Constantes Judilibre (port de judilibre/downloader.py)
// ----------------------------------------------------------------------------

use crate::state_paths::JUDILIBRE_DIR;
pub(super) const UNDATED_BUCKET: &str = "undated";
const ARCHIVE_KEY: &str = "archive";
const DEFAULT_BATCH_SIZE: i64 = 1000;
const DEFAULT_JURISDICTIONS: &[&str] = &["cc", "ca", "tj", "tcom"];

/// Coupure historique par juridiction (`ARCHIVE_CUTOFF`).
fn archive_cutoff(jur: &str) -> Option<&'static str> {
    match jur {
        "cc" => Some("1986-10-31"),
        "ca" => Some("2010-07-31"),
        _ => None,
    }
}

/// Mois de départ du bootstrap mensuel par juridiction (`MONTHLY_START`).
fn monthly_start(jur: &str) -> Option<&'static str> {
    match jur {
        "cc" => Some("1986-11-01"),
        "ca" => Some("2010-08-01"),
        "tj" => Some("2022-05-28"),
        "tcom" => Some("2024-11-15"),
        _ => None,
    }
}

/// `YYYYMM` extrait de `decision_date` ISO, sinon `UNDATED_BUCKET` (port de `_yyyymm_of`).
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
// sync_judilibre — port de judilibre/downloader.sync (HTTP async, mono-thread)
// ----------------------------------------------------------------------------

/// Sync incrémental Judilibre depuis `date_start_iso` (`YYYY-MM-DD`).
///
/// Pour chaque juridiction de [`DEFAULT_JURISDICTIONS`] : bootstrap archive (≤
/// cutoff) + bootstrap mensuel (mois > cutoff) via `/scan`, puis incrémental
/// global via `/transactionalhistory`. Append-only sur `.jsonl.gz`, reprise au
/// cursor. Mono-thread (le Python parallélise le bootstrap mensuel ; logique et
/// résultat identiques, ordre séquentiel). Le verrou `fcntl.flock` n'est pas
/// porté (cf. `unresolved`).
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

    for jur in DEFAULT_JURISDICTIONS {
        let jur = *jur;
        let mut state = manifest
            .jurisdictions
            .remove(jur)
            .unwrap_or_else(|| JurState::new(jur));

        let cutoff = archive_cutoff(jur);
        let cutoff_yyyymm = cutoff.map(|c| format!("{}{}", &c[0..4], &c[5..7]));

        let archive_needs_bootstrap = cutoff.is_some()
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

        // Initialise les MonthState, collecte les mois à bootstrapper.
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

        // Bootstrap archive (séquentiel).
        if let Some(cutoff_date) = cutoff {
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
                    cutoff_date,
                    &mut state,
                    &mut manifest,
                    &manifest_path,
                )
                .await?;
            }
        }

        // Bootstrap mensuel (séquentiel).
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

    // Seed du watermark global.
    if manifest.history_watermark.is_none() {
        if let Some(seed) = &bootstrap_started_at {
            manifest.history_watermark = Some(seed.clone());
            manifest.save(&manifest_path)?;
        }
    }

    // Incrémental global /transactionalhistory.
    incremental_global(client, &source_dir, &mut manifest, &manifest_path).await?;

    manifest.save(&manifest_path)?;
    Ok(manifest)
}

/// Heure UTC serveur via `query_date` de `/transactionalhistory` (port de `_server_now`).
async fn server_now(client: &JudilibreClient) -> Result<String> {
    let now = Manifest::now_iso_seconds();
    let probe = client.transactional_history(&now, None).await?;
    Ok(probe
        .get("query_date")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or(now))
}

/// Met à jour `max_update_date` du mois (port de `_update_max_update_date`).
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

/// Append des décisions dans `<jur>/<key>.jsonl.gz` (port de `_append_decisions`).
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

/// Lit les ids présents dans `tombstones.jsonl` (set). Fichier absent = vide.
fn read_tombstone_ids(path: &Path) -> Result<std::collections::HashSet<String>> {
    let mut ids = std::collections::HashSet::new();
    if !path.exists() {
        return Ok(ids);
    }
    for line in fs::read_to_string(path)?.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let Some(id) = v.get("id").and_then(|v| v.as_str()) {
                ids.insert(id.to_string());
            }
        }
    }
    Ok(ids)
}

/// Marque un id comme supprimé. Idempotent : ne ré-append pas un id déjà
/// tombstoned (évite la croissance par doublons sur des `deleted` répétés).
pub(super) fn append_tombstone(data_dir: &Path, decision_id: &str) -> Result<()> {
    let path = data_dir.join("tombstones.jsonl");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if read_tombstone_ids(&path)?.contains(decision_id) {
        return Ok(());
    }
    let line = serde_json::json!({
        "id": decision_id,
        "deleted_at": Manifest::now_iso_seconds(),
    });
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

/// Annule la tombstone d'un id (corrige la résurrection, ADR 0087). Une décision
/// supprimée puis re-créée côté Judilibre (`deleted` suivi d'un `created`/
/// `updated`) doit redevenir vivante : sans ça, la tombstone périmée la ferait
/// dropper par `compact_archives` et supprimer en base. Réécriture atomique sans
/// l'id ; no-op (sans réécriture) s'il est absent.
fn remove_tombstone(data_dir: &Path, decision_id: &str) -> Result<()> {
    let path = data_dir.join("tombstones.jsonl");
    if !path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(&path)?;
    let mut kept = String::with_capacity(content.len());
    let mut found = false;
    for line in content.lines() {
        let id = serde_json::from_str::<Value>(line.trim())
            .ok()
            .and_then(|v| v.get("id").and_then(|v| v.as_str()).map(str::to_string));
        if id.as_deref() == Some(decision_id) {
            found = true;
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    if !found {
        return Ok(());
    }
    let tmp = path_with_added_extension(&path, "tmp");
    fs::write(&tmp, kept.as_bytes())?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Bootstrap archive : décisions ≤ `cutoff_date` dans `archive.jsonl.gz` (port
/// de `_bootstrap_archive`).
async fn bootstrap_archive(
    client: &JudilibreClient,
    data_dir: &Path,
    jurisdiction: &str,
    cutoff_date: &str,
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
            ("date_end", cutoff_date.to_string()),
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

/// Bootstrap d'un mois via `/scan` (port de `_bootstrap_month`).
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

/// Process `/transactionalhistory` global multi-juridictions (port de
/// `_incremental_global`). Mono-thread : les `client.decision(id)` sont séquen-
/// tiels (le Python utilise un pool de 8 threads ; même résultat).
async fn incremental_global(
    client: &JudilibreClient,
    data_dir: &Path,
    manifest: &mut Manifest,
    manifest_path: &Path,
) -> Result<()> {
    if manifest.history_watermark.is_none() && manifest.history_cursor.is_none() {
        return Ok(());
    }

    let mut cursor = manifest.history_cursor.clone();
    // `query_date` est calculée une fois (borne basse formelle quand un cursor
    // pilote la pagination) et n'est plus réassignée dans la boucle — port
    // fidèle de `_incremental_global` (le cursor `from_id` est primaire).
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

    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
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
            // Tombstone last-action-wins (ADR 0087) : un `deleted` marque mort,
            // tout autre événement (created/updated) ressuscite en annulant la
            // tombstone. L'effet s'applique à CHAQUE occurrence — hors du dédup
            // `seen_ids` — pour couvrir delete↔create dans une même fenêtre. Les
            // ops étant chronologiques, le dernier événement de l'id l'emporte.
            // `seen_ids` ne dédup plus que les fetchs.
            if action == Some("deleted") {
                append_tombstone(data_dir, &decision_id)?;
                continue;
            }
            remove_tombstone(data_dir, &decision_id)?;
            if seen_ids.insert(decision_id.clone()) {
                to_fetch.push(decision_id);
            }
        }

        // Fetch séquentiel (le Python parallélise via ThreadPoolExecutor(8)).
        let mut groups: BTreeMap<(String, String), Vec<Value>> = BTreeMap::new();
        for did in &to_fetch {
            match client.decision(did).await {
                Ok(decision) => {
                    let jur = match decision.get("jurisdiction").and_then(|v| v.as_str()) {
                        Some(j) if !j.is_empty() => j.to_string(),
                        _ => continue,
                    };
                    let yyyymm = yyyymm_of(&decision);
                    groups.entry((jur, yyyymm)).or_default().push(decision);
                }
                Err(e) => {
                    tracing::error!(id = did, error = %e, "judilibre_decision_fetch_failed");
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
            written += decisions.len();
        }

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

/// `results` d'une page `/scan` (liste, vide par défaut).
fn page_results(page: &Value) -> Vec<Value> {
    page.get("results")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

/// `dt - 1h` en ISO millisecondes (port de la marge horaire de `_incremental_global`).
fn minus_one_hour_iso(wm: &str) -> String {
    let normalized = wm.replace('Z', "+00:00");
    match chrono::DateTime::parse_from_rfc3339(&normalized) {
        Ok(dt) => (dt - chrono::Duration::hours(1))
            .format("%Y-%m-%dT%H:%M:%S%.3f%:z")
            .to_string(),
        Err(_) => "1970-01-01T00:00:00".to_string(),
    }
}

/// `true` si un `.jsonl.gz` est lisible sans erreur (port de `_is_valid_gzip`).
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
    fn yyyymm_of_decision() {
        let d = serde_json::json!({"decision_date": "2024-11-15"});
        assert_eq!(yyyymm_of(&d), "202411");
        let undated = serde_json::json!({"foo": "bar"});
        assert_eq!(yyyymm_of(&undated), "undated");
        // Format inattendu (pas de tiret en position 4) → undated.
        let weird = serde_json::json!({"decision_date": "20241115"});
        assert_eq!(yyyymm_of(&weird), "undated");
    }

    // Spec ADR 0087 : append idempotent (pas de doublon) + remove annule.
    #[test]
    fn tombstone_append_idempotent_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        append_tombstone(dir.path(), "a").unwrap();
        append_tombstone(dir.path(), "a").unwrap(); // idempotent : pas de doublon
        append_tombstone(dir.path(), "b").unwrap();
        let path = dir.path().join("tombstones.jsonl");
        assert_eq!(read_tombstone_ids(&path).unwrap().len(), 2);

        remove_tombstone(dir.path(), "a").unwrap(); // résurrection de "a"
        let ids = read_tombstone_ids(&path).unwrap();
        assert!(!ids.contains("a"));
        assert!(ids.contains("b"));

        remove_tombstone(dir.path(), "zzz").unwrap(); // absent = no-op
        assert_eq!(read_tombstone_ids(&path).unwrap().len(), 1);
    }

    // Spec ADR 0087 : deleted→created (résurrection) ⇒ la tombstone est annulée
    // et la décision survit au compact (sans le fix, elle serait droppée).
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
        remove_tombstone(dir.path(), "d").unwrap(); // puis re-created

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
