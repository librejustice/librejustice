//! Purge des décisions retirées du corpus (port de
//! `librejustice-store/pipelines/reverses.py`).
//!
//! La source amont publie trois CSV (un par juridiction) listant les décisions
//! reversées — retirées ou remplacées. On les hard-delete : la ligne
//! `decisions` part, et les FK `ON DELETE CASCADE` purgent `decision_chunks`,
//! `decision_full_text` et `legal_citation` (cf. ADR 0033 / 0145).
//!
//! Le hard-delete passe par `DecisionRepository::delete` (AGENTS.md règle #2,
//! pas de SQL inline). L'encodage source est `cp1252`, le délimiteur `;`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lj_store::repository::DecisionRepository;

/// Bilan d'une purge sur un CSV (port de `PurgeSummary`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgeSummary {
    pub file: String,
    pub processed: usize,
    pub newly_deleted: usize,
    pub already_deleted_or_missing: usize,
}

/// `source_uid` candidats pour une ligne (port de `_candidate_source_uids`).
///
/// La source ne donne qu'un nom de fichier ; on reconstruit les formes
/// possibles du `source_uid` persisté, de la plus spécifique à la plus large.
pub fn candidate_source_uids(filename: &str, zip_name: &str, ta_jur: &str) -> Vec<String> {
    let exact_member = if !ta_jur.is_empty() {
        format!("{ta_jur}/{filename}")
    } else {
        filename.to_string()
    };
    let exact_source_uid = if !zip_name.is_empty() {
        format!("{zip_name}/{exact_member}")
    } else {
        exact_member.clone()
    };
    let path_without_ext = strip_xml(&exact_member);
    let basename_without_ext = strip_xml(filename);

    let mut out = vec![exact_source_uid];
    if !out.contains(&path_without_ext) {
        out.push(path_without_ext);
    }
    if !out.contains(&basename_without_ext) {
        out.push(basename_without_ext);
    }
    out
}

/// `str.removesuffix(".xml")` (ne retire que si présent).
fn strip_xml(value: &str) -> String {
    value.strip_suffix(".xml").unwrap_or(value).to_string()
}

/// Hard-delete à partir d'un CSV. Encodage `cp1252` côté source.
///
/// Port de `purge_csv` : skip header, ignore lignes vides, pour chaque ligne on
/// tente les `source_uid` candidats — `hit` dès qu'un `delete` retourne vrai.
pub async fn purge_csv(csv_path: &Path, repo: &DecisionRepository<'_>) -> Result<PurgeSummary> {
    let bytes = std::fs::read(csv_path)
        .with_context(|| format!("reverses: read {}", csv_path.display()))?;
    let text = decode_cp1252(&bytes);

    let mut processed = 0usize;
    let mut hit = 0usize;
    for (idx, line) in text.lines().enumerate() {
        if idx == 0 {
            continue; // header
        }
        if line.is_empty() {
            continue;
        }
        let row: Vec<&str> = line.split(';').collect();
        // `if not row or not row[0]: continue`.
        if row.first().map(|c| c.is_empty()).unwrap_or(true) {
            continue;
        }
        processed += 1;
        let filename = row[0].trim();
        let zip_name = row.get(4).map(|c| c.trim()).unwrap_or("");
        let ta_jur = row.get(5).map(|c| c.trim()).unwrap_or("");
        let candidates = candidate_source_uids(filename, zip_name, ta_jur);
        let mut deleted = false;
        for source_uid in &candidates {
            if repo
                .delete(source_uid)
                .await
                .with_context(|| format!("reverses: delete {source_uid}"))?
            {
                deleted = true;
            }
        }
        if deleted {
            hit += 1;
        }
    }

    Ok(PurgeSummary {
        file: file_name(csv_path),
        processed,
        newly_deleted: hit,
        already_deleted_or_missing: processed - hit,
    })
}

/// Traite tous les `*_documents_reverses.csv` d'un dossier (port de `purge_all`).
pub async fn purge_all(csv_dir: &Path, repo: &DecisionRepository<'_>) -> Result<Vec<PurgeSummary>> {
    let mut summaries: Vec<PurgeSummary> = Vec::new();
    let mut csv_paths: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(csv_dir)
        .with_context(|| format!("reverses: read_dir {}", csv_dir.display()))?
    {
        let entry = entry.context("reverses: read_dir entry")?;
        let path = entry.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("_documents_reverses.csv"))
        {
            csv_paths.push(path);
        }
    }
    csv_paths.sort();
    for csv_path in csv_paths {
        let summary = purge_csv(&csv_path, repo).await?;
        tracing::info!(file = %summary.file, ?summary, "Purge");
        summaries.push(summary);
    }
    Ok(summaries)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Décode du `cp1252` (Windows-1252) en `String`.
///
/// Identique à latin-1 sauf la plage `0x80..=0x9F` (caractères typographiques
/// Windows). Octets non mappés (`0x81, 0x8D, 0x8F, 0x90, 0x9D`) → U+FFFD, comme
/// le décodage strict… ici on les rend en remplacement plutôt que d'échouer.
fn decode_cp1252(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| cp1252_char(b)).collect()
}

fn cp1252_char(b: u8) -> char {
    match b {
        0x80 => '\u{20AC}',
        0x82 => '\u{201A}',
        0x83 => '\u{0192}',
        0x84 => '\u{201E}',
        0x85 => '\u{2026}',
        0x86 => '\u{2020}',
        0x87 => '\u{2021}',
        0x88 => '\u{02C6}',
        0x89 => '\u{2030}',
        0x8A => '\u{0160}',
        0x8B => '\u{2039}',
        0x8C => '\u{0152}',
        0x8E => '\u{017D}',
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x93 => '\u{201C}',
        0x94 => '\u{201D}',
        0x95 => '\u{2022}',
        0x96 => '\u{2013}',
        0x97 => '\u{2014}',
        0x98 => '\u{02DC}',
        0x99 => '\u{2122}',
        0x9A => '\u{0161}',
        0x9B => '\u{203A}',
        0x9C => '\u{0153}',
        0x9E => '\u{017E}',
        0x9F => '\u{0178}',
        0x81 | 0x8D | 0x8F | 0x90 | 0x9D => '\u{FFFD}',
        other => other as char, // 0x00..=0x7F et 0xA0..=0xFF == latin-1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec : candidate_source_uids dérive les formes spécifique→large.
    #[test]
    fn candidates_full() {
        let c = candidate_source_uids("123456.xml", "TA_202603.zip", "ta_paris");
        assert_eq!(
            c,
            vec![
                "TA_202603.zip/ta_paris/123456.xml".to_string(),
                "ta_paris/123456".to_string(),
                "123456".to_string(),
            ]
        );
    }

    // Spec : sans zip ni jur, le source_uid exact est le filename nu.
    #[test]
    fn candidates_minimal() {
        let c = candidate_source_uids("123456.xml", "", "");
        // exact = "123456.xml" ; path_without_ext = "123456" ; basename idem
        // (déduplication → pas de doublon).
        assert_eq!(c, vec!["123456.xml".to_string(), "123456".to_string()]);
    }

    // Spec : pas de doublon quand path_without_ext == basename_without_ext.
    #[test]
    fn candidates_dedup_basename() {
        let c = candidate_source_uids("a.xml", "zip", "");
        // path_without_ext == basename_without_ext == "a" → un seul "a" (cf. Python
        // _candidate_source_uids : ("zip/a.xml", "a")).
        assert_eq!(c, vec!["zip/a.xml".to_string(), "a".to_string()]);
    }

    // Spec : cp1252 — 0x80=€, 0x92=apostrophe typographique, 0xE9=é.
    #[test]
    fn cp1252_decoding() {
        assert_eq!(decode_cp1252(&[0x80]), "€");
        assert_eq!(decode_cp1252(&[0x92]), "\u{2019}");
        assert_eq!(decode_cp1252(&[0xE9]), "é"); // latin-1 range
        assert_eq!(decode_cp1252(b"abc"), "abc");
    }
}
