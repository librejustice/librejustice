//! Compactage des archives `.jsonl.gz` (port de `compact_archives`, mono-thread).

use super::http::path_with_added_extension;
use super::judilibre::JUDILIBRE_SOURCE_DIR;
use super::manifest::Manifest;
use crate::error::{Result, SourceError};
use chrono::Utc;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::Path;

/// Réécrit chaque `.jsonl.gz` en supprimant les ids tombstoned + dédoublons
/// (port de `compact_archives`). Mono-thread (`max_workers` ignoré ; le Python
/// parallélise les fichiers). Atomique (tmp + rename).
pub fn compact_archives(data_dir: &Path, manifest: &Manifest, _max_workers: usize) -> Result<()> {
    let source_dir = if data_dir.join(JUDILIBRE_SOURCE_DIR).is_dir() {
        data_dir.join(JUDILIBRE_SOURCE_DIR)
    } else {
        data_dir.to_path_buf()
    };

    let tombstones_path = source_dir.join("tombstones.jsonl");
    let mut deleted_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    if tombstones_path.exists() {
        for line in fs::read_to_string(&tombstones_path)?.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                if let Some(id) = v.get("id").and_then(|v| v.as_str()) {
                    deleted_ids.insert(id.to_string());
                }
            }
        }
    }

    for jur in manifest.jurisdictions.keys() {
        let jur_dir = source_dir.join(jur);
        if !jur_dir.is_dir() {
            continue;
        }
        let mut paths: Vec<std::path::PathBuf> = fs::read_dir(&jur_dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with(".jsonl.gz"))
                    .unwrap_or(false)
            })
            .collect();
        paths.sort();
        for path in paths {
            compact_one(&path, &deleted_ids)?;
        }
    }
    Ok(())
}

/// Compacte un fichier : dédup par id (last-in-file wins), filtre tombstones,
/// réécrit atomique (port de `_compact_one`). Renvoie le nombre de records gardés.
fn compact_one(path: &Path, deleted_ids: &std::collections::HashSet<String>) -> Result<usize> {
    use std::io::Read;
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Ok(0),
    };
    let mut decoder = flate2::read::MultiGzDecoder::new(file);
    let mut content = String::new();
    if decoder.read_to_string(&mut content).is_err() {
        tracing::warn!(path = %path.display(), "judilibre_compact_unreadable");
        return Ok(0);
    }

    // Préserve l'ordre d'insertion (last write wins) comme le dict Python.
    let mut order: Vec<String> = Vec::new();
    let mut by_id: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(line)
            .map_err(|e| SourceError::Invalid(format!("compact: ligne JSON invalide: {e}")))?;
        let rid = match record.get("id").and_then(|v| v.as_str()) {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => continue,
        };
        if deleted_ids.contains(&rid) {
            continue;
        }
        if let Some(existing) = by_id.get(&rid) {
            check_append_invariant(&rid, existing, &record, path)?;
        } else {
            order.push(rid.clone());
        }
        by_id.insert(rid, record);
    }

    let tmp = path_with_added_extension(path, "tmp");
    {
        let out = fs::File::create(&tmp)?;
        let mut encoder = GzEncoder::new(out, Compression::default());
        for rid in &order {
            let record = &by_id[rid];
            let line = serde_json::to_string(record).map_err(|e| {
                SourceError::Invalid(format!("compact: record non sérialisable: {e}"))
            })?;
            encoder.write_all(line.as_bytes())?;
            encoder.write_all(b"\n")?;
        }
        encoder.finish()?;
    }
    fs::rename(&tmp, path)?;
    Ok(by_id.len())
}

/// Vérifie l'invariant append-only (`newer.update_date >= older.update_date`)
/// quand les deux existent (port de `_check_append_invariant`).
fn check_append_invariant(rid: &str, older: &Value, newer: &Value, path: &Path) -> Result<()> {
    let old_ud = older.get("update_date").and_then(|v| v.as_str());
    let new_ud = newer.get("update_date").and_then(|v| v.as_str());
    let (old_ud, new_ud) = match (old_ud, new_ud) {
        (Some(o), Some(n)) => (o, n),
        _ => return Ok(()),
    };
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let old_dt = parse_flexible_iso(old_ud);
    let new_dt = parse_flexible_iso(new_ud);
    let (old_dt, new_dt) = match (old_dt, new_dt) {
        (Some(o), Some(n)) => (o, n),
        _ => {
            return Err(SourceError::Invalid(format!(
                "compact: update_date non-ISO pour id={rid} dans {name} (old={old_ud:?}, new={new_ud:?})"
            )))
        }
    };
    // Mélange naïf/aware = format incohérent.
    if old_dt.has_offset != new_dt.has_offset {
        return Err(SourceError::Invalid(format!(
            "compact: update_date mix naive/aware pour id={rid} dans {name} (old={old_ud:?}, new={new_ud:?})"
        )));
    }
    if new_dt.utc < old_dt.utc {
        return Err(SourceError::Invalid(format!(
            "compact: invariant append-only violé pour id={rid} dans {name} (ordre fichier récent mais update_date {new_ud} < {old_ud})"
        )));
    }
    Ok(())
}

struct ParsedDt {
    utc: chrono::DateTime<Utc>,
    has_offset: bool,
}

/// Parse un ISO `YYYY-MM-DDTHH:MM:SS[.fff][+oo:oo|Z]`, avec ou sans offset
/// (port du `datetime.fromisoformat` Python + remplacement `Z`→`+00:00`).
fn parse_flexible_iso(s: &str) -> Option<ParsedDt> {
    let normalized = s.replace('Z', "+00:00");
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&normalized) {
        return Some(ParsedDt {
            utc: dt.with_timezone(&Utc),
            has_offset: true,
        });
    }
    // Naïf (sans offset) : on l'interprète en UTC pour la comparaison.
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d"] {
        if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(ParsedDt {
                utc: ndt.and_utc(),
                has_offset: false,
            });
        }
        if fmt == "%Y-%m-%d" {
            if let Ok(d) = chrono::NaiveDate::parse_from_str(s, fmt) {
                return Some(ParsedDt {
                    utc: d.and_hms_opt(0, 0, 0).unwrap().and_utc(),
                    has_offset: false,
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::super::judilibre::{append_decisions, append_tombstone};
    use super::super::manifest::JurState;
    use super::*;

    #[test]
    fn append_invariant_rejects_regression() {
        let older = serde_json::json!({"id": "x", "update_date": "2024-02-01T00:00:00"});
        let newer = serde_json::json!({"id": "x", "update_date": "2024-01-01T00:00:00"});
        let err = check_append_invariant("x", &older, &newer, Path::new("f.jsonl.gz"));
        assert!(err.is_err());
    }

    #[test]
    fn append_invariant_accepts_progression_and_missing() {
        let older = serde_json::json!({"id": "x", "update_date": "2024-01-01T00:00:00Z"});
        let newer = serde_json::json!({"id": "x", "update_date": "2024-02-01T00:00:00Z"});
        assert!(check_append_invariant("x", &older, &newer, Path::new("f")).is_ok());
        // update_date manquant → tolérant.
        let a = serde_json::json!({"id": "x"});
        let b = serde_json::json!({"id": "x", "update_date": "2024-02-01T00:00:00Z"});
        assert!(check_append_invariant("x", &a, &b, Path::new("f")).is_ok());
    }

    #[test]
    fn append_invariant_rejects_naive_aware_mix() {
        let older = serde_json::json!({"id": "x", "update_date": "2024-01-01T00:00:00"});
        let newer = serde_json::json!({"id": "x", "update_date": "2024-02-01T00:00:00Z"});
        assert!(check_append_invariant("x", &older, &newer, Path::new("f")).is_err());
    }

    #[test]
    fn compact_dedups_and_drops_tombstones() {
        let dir = tempfile::tempdir().unwrap();
        let jur_dir = dir.path().join("cc");
        fs::create_dir_all(&jur_dir).unwrap();
        let path = jur_dir.join("202401.jsonl.gz");

        let recs = vec![
            serde_json::json!({"id": "a", "update_date": "2024-01-01T00:00:00Z", "v": 1}),
            serde_json::json!({"id": "a", "update_date": "2024-01-02T00:00:00Z", "v": 2}),
            serde_json::json!({"id": "b", "update_date": "2024-01-01T00:00:00Z"}),
            serde_json::json!({"id": "c", "update_date": "2024-01-01T00:00:00Z"}),
        ];
        append_decisions(&path, &recs).unwrap();

        // Tombstone pour "c".
        append_tombstone(dir.path(), "c").unwrap();

        let mut manifest = Manifest::default();
        manifest
            .jurisdictions
            .insert("cc".to_string(), JurState::new("cc"));

        compact_archives(dir.path(), &manifest, 1).unwrap();

        // Relit le fichier compacté.
        use std::io::Read;
        let mut decoder = flate2::read::MultiGzDecoder::new(fs::File::open(&path).unwrap());
        let mut out = String::new();
        decoder.read_to_string(&mut out).unwrap();
        let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
        // a (dédupliqué, v=2) + b ; c retiré (tombstone).
        assert_eq!(lines.len(), 2);
        let a: Value =
            serde_json::from_str(lines.iter().find(|l| l.contains("\"a\"")).unwrap()).unwrap();
        assert_eq!(a.get("v").and_then(|v| v.as_i64()), Some(2));
        assert!(!out.contains("\"c\""));
    }
}
