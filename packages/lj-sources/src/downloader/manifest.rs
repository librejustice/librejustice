//! Manifest de sync versionné sur disque.
//!
//! Persiste un [`Manifest`] JSON sur disque pour la reprise idempotente. Les
//! deux sources ont des formats de manifeste distincts (opendata : `entries`
//! par `{jur}/{yyyymm}` ; Judilibre : `jurisdictions` + watermark/cursor
//! `/transactionalhistory`). [`Manifest`] porte les deux formes ; `load`
//! détecte le format présent sur disque et `save` réécrit la forme courante.

use crate::error::{Result, SourceError};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

// ----------------------------------------------------------------------------
// Manifest opendata
// ----------------------------------------------------------------------------

/// Entrée opendata (port du dataclass `Entry`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub juridiction: String,
    pub yyyymm: String,
    pub url: String,
    pub path: String,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub last_modified: Option<String>,
    /// `max(Date_de_reversement)` du registry `documents_reverses` pour ce ZIP,
    /// normalisé `YYYYMMDDHHMMSS` (tri lexical = chronologique). Watermark de
    /// fraîcheur : un ZIP n'est re-téléchargé que si le registry présente un
    /// reversement plus récent. `None` = jamais vu au registry (mois stable).
    #[serde(default)]
    pub last_reversement: Option<String>,
    #[serde(default)]
    pub fetched_at: Option<String>,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub fully_ingested: bool,
    #[serde(default)]
    pub embeddings_complete: bool,
}

pub(super) fn default_status() -> String {
    "pending".to_string()
}

// ----------------------------------------------------------------------------
// Manifest Judilibre
// ----------------------------------------------------------------------------

/// État d'avancement d'un `(juridiction, mois)` (port de `MonthState`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MonthState {
    #[serde(default)]
    pub bootstrapped: bool,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub decision_count: i64,
    #[serde(default)]
    pub max_update_date: Option<String>,
    #[serde(default)]
    pub last_fetched_at: Option<String>,
    #[serde(default)]
    pub fully_ingested: bool,
    #[serde(default)]
    pub embeddings_complete: bool,
    #[serde(default)]
    pub ingested_size: Option<u64>,
    #[serde(default)]
    pub ingested_lines: Option<i64>,
}

/// État d'avancement d'une juridiction (port de `JurState`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JurState {
    pub jurisdiction: String,
    #[serde(default)]
    pub months: BTreeMap<String, MonthState>,
    #[serde(default)]
    pub total_decisions: i64,
}

impl JurState {
    pub(super) fn new(jurisdiction: &str) -> Self {
        Self {
            jurisdiction: jurisdiction.to_string(),
            months: BTreeMap::new(),
            total_decisions: 0,
        }
    }
}

// ----------------------------------------------------------------------------
// Manifest unifié
// ----------------------------------------------------------------------------

/// Manifest de sync versionné sur disque. Porte les deux formats (opendata /
/// Judilibre) — la variante est fixée à la création (par la fonction `sync_*`)
/// et détectée au chargement.
#[derive(Debug, Clone, Default)]
pub struct Manifest {
    /// Format opendata : entrées par clé `{jur}/{yyyymm}`.
    pub entries: BTreeMap<String, Entry>,
    /// Format Judilibre : état par juridiction.
    pub jurisdictions: BTreeMap<String, JurState>,
    /// Curseur global `/transactionalhistory` (borne basse `query_date`).
    pub history_watermark: Option<String>,
    /// `from_id` opaque (source de vérité du flux transactionnel).
    pub history_cursor: Option<String>,
}

#[derive(Serialize)]
struct OpendataManifestOut<'a> {
    updated_at: String,
    entries: &'a BTreeMap<String, Entry>,
}

#[derive(Deserialize)]
struct OpendataManifestIn {
    #[serde(default)]
    entries: BTreeMap<String, Entry>,
}

#[derive(Serialize)]
struct JudilibreManifestOut<'a> {
    updated_at: String,
    history_watermark: &'a Option<String>,
    history_cursor: &'a Option<String>,
    jurisdictions: &'a BTreeMap<String, JurState>,
}

impl Manifest {
    pub(super) fn now_iso_seconds() -> String {
        Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    }

    /// Charge un manifeste. Détecte le format par la présence de la clé
    /// `jurisdictions` (Judilibre) ou `entries` (opendata). Fichier absent =
    /// manifeste vide.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)?;
        let value: Value = serde_json::from_str(&raw)
            .map_err(|e| SourceError::Invalid(format!("manifest illisible: {e}")))?;
        let obj = value
            .as_object()
            .ok_or_else(|| SourceError::Invalid("manifest: objet attendu".into()))?;

        if obj.contains_key("jurisdictions") {
            let mut manifest = Manifest::default();
            if let Some(jurs) = obj.get("jurisdictions").and_then(|v| v.as_object()) {
                for (jur, raw_state) in jurs {
                    manifest
                        .jurisdictions
                        .insert(jur.clone(), parse_jur_state(jur, raw_state));
                }
            }
            // Migration legacy : watermark global absent → min des watermarks per-jur.
            manifest.history_watermark = obj
                .get("history_watermark")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .or_else(|| legacy_min_watermark(obj));
            manifest.history_cursor = obj
                .get("history_cursor")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            Ok(manifest)
        } else {
            let parsed: OpendataManifestIn = serde_json::from_value(value)
                .map_err(|e| SourceError::Invalid(format!("manifest opendata illisible: {e}")))?;
            Ok(Manifest {
                entries: parsed.entries,
                ..Default::default()
            })
        }
    }

    /// Écrit le manifeste (tmp + rename atomique). Choisit le format selon les
    /// champs renseignés (Judilibre si `jurisdictions` non vide ou watermark/
    /// cursor présents, sinon opendata).
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let is_judilibre = !self.jurisdictions.is_empty()
            || self.history_watermark.is_some()
            || self.history_cursor.is_some()
            || self.entries.is_empty();

        let json = if is_judilibre {
            serde_json::to_string_pretty(&JudilibreManifestOut {
                updated_at: Self::now_iso_seconds(),
                history_watermark: &self.history_watermark,
                history_cursor: &self.history_cursor,
                jurisdictions: &self.jurisdictions,
            })
        } else {
            serde_json::to_string_pretty(&OpendataManifestOut {
                updated_at: Self::now_iso_seconds(),
                entries: &self.entries,
            })
        }
        .map_err(|e| SourceError::Invalid(format!("manifest non sérialisable: {e}")))?;

        let tmp = path.with_extension(match path.extension().and_then(|e| e.to_str()) {
            Some(ext) => format!("{ext}.tmp"),
            None => "tmp".to_string(),
        });
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// Parse une entrée `jurisdictions[jur]`, en migrant l'ancien format au passage
/// (port de `_parse_jur_state` : ancien format sans `months`).
fn parse_jur_state(jur: &str, raw: &Value) -> JurState {
    let total_decisions = raw
        .get("total_decisions")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let months = raw
        .get("months")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| {
                    serde_json::from_value::<MonthState>(v.clone())
                        .ok()
                        .map(|ms| (k.clone(), ms))
                })
                .collect()
        })
        .unwrap_or_default();
    JurState {
        jurisdiction: jur.to_string(),
        months,
        total_decisions,
    }
}

/// `min` des watermarks legacy per-jur (`history_watermark` /
/// `watermark_update_date`), port de la migration `Manifest.load`.
fn legacy_min_watermark(obj: &serde_json::Map<String, Value>) -> Option<String> {
    let jurs = obj.get("jurisdictions")?.as_object()?;
    jurs.values()
        .filter_map(|v| {
            v.get("history_watermark")
                .or_else(|| v.get("watermark_update_date"))
                .and_then(|w| w.as_str())
                .map(str::to_string)
        })
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_roundtrip_opendata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        let mut m = Manifest::default();
        m.entries.insert(
            "TA/202603".to_string(),
            Entry {
                juridiction: "TA".to_string(),
                yyyymm: "202603".to_string(),
                url: "u".to_string(),
                path: "p".to_string(),
                size: Some(42),
                sha256: None,
                last_modified: None,
                last_reversement: None,
                fetched_at: None,
                status: "ok".to_string(),
                fully_ingested: true,
                embeddings_complete: false,
            },
        );
        m.save(&path).unwrap();
        let loaded = Manifest::load(&path).unwrap();
        assert_eq!(loaded.entries.len(), 1);
        let e = &loaded.entries["TA/202603"];
        assert_eq!(e.status, "ok");
        assert_eq!(e.size, Some(42));
        assert!(e.fully_ingested);
    }

    #[test]
    fn manifest_roundtrip_judilibre() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        let mut m = Manifest {
            history_watermark: Some("2024-01-01T00:00:00+00:00".to_string()),
            history_cursor: Some("cursor123".to_string()),
            ..Default::default()
        };
        let mut state = JurState::new("cc");
        state.total_decisions = 5;
        state.months.insert(
            "archive".to_string(),
            MonthState {
                bootstrapped: true,
                decision_count: 5,
                ..Default::default()
            },
        );
        m.jurisdictions.insert("cc".to_string(), state);
        m.save(&path).unwrap();

        let loaded = Manifest::load(&path).unwrap();
        assert_eq!(loaded.history_cursor.as_deref(), Some("cursor123"));
        assert_eq!(loaded.jurisdictions["cc"].total_decisions, 5);
        assert!(loaded.jurisdictions["cc"].months["archive"].bootstrapped);
    }
}
