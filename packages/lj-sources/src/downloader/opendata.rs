//! Sync incrémental opendata (port de `sources/downloader.py`, HTTP synchrone).

use super::calendar::month_range;
use super::http::{get_to_file_retrying, get_with_body_retrying, path_with_added_extension};
use super::manifest::{default_status, Entry, Manifest};
use super::sha256::sha256_of;
use crate::error::{Result, SourceError};
use chrono::{Datelike, Utc};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use tracing::instrument;

// ----------------------------------------------------------------------------
// Constantes opendata (port de downloader.py)
// ----------------------------------------------------------------------------

const OPENDATA_BASE_URL: &str = "https://opendata.justice-administrative.fr";
use crate::state_paths::OPENDATA_DIR;
const OPENDATA_USER_AGENT: &str = "librejustice-downloader/0.1 (+https://github.com/)";
const DEFAULT_EARLIEST: &str = "2021-01";

/// `(sub_path, file_prefix, reverses_csv)` par juridiction opendata.
fn jurisdiction_meta(jur: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match jur {
        "TA" => Some(("DTA", "TA", "TA_documents_reverses.csv")),
        "CAA" => Some(("DCA", "CAA", "CAA_documents_reverses.csv")),
        "CE" => Some(("DCE", "CE", "CE_documents_reverses.csv")),
        _ => None,
    }
}

const OPENDATA_JURISDICTIONS: &[&str] = &["TA", "CAA", "CE"];

/// Construit l'URL d'une archive opendata (port de `_build_url`).
fn build_opendata_url(jur: &str, yyyymm: &str) -> String {
    let (sub, prefix, _) = jurisdiction_meta(jur).expect("juridiction opendata connue");
    let (y, m) = (&yyyymm[0..4], &yyyymm[4..6]);
    format!("{OPENDATA_BASE_URL}/{sub}/{y}/{m}/{prefix}_{yyyymm}.zip")
}

// ----------------------------------------------------------------------------
// sync_opendata — port de downloader.sync (HTTP synchrone)
// ----------------------------------------------------------------------------

/// Sync incrémental opendata : télécharge les ZIP mensuels manquants/modifiés
/// pour `{TA, CAA, CE}` de [`DEFAULT_EARLIEST`] au mois courant.
///
/// Reprise idempotente via le manifeste : conditional GET (`If-Modified-Since`),
/// HEAD pour vérifier qu'un ZIP local est à jour, refetch des mois `not_found`.
/// La reprise partielle `Range`/`.part` et `tqdm` du Python ne sont pas portées
/// (cf. `unresolved`).
///
/// Client HTTP synchrone (`reqwest::blocking`) : reqwest-middleware est
/// async-only, donc pas de span par requête ici. Le `#[instrument]` ouvre un
/// span parent couvrant tout le sync opendata.
#[instrument(skip(data_dir))]
pub fn sync_opendata(data_dir: &Path, force: bool) -> Result<Manifest> {
    let source_dir = data_dir.join(OPENDATA_DIR);
    fs::create_dir_all(source_dir.join("zips"))?;
    let manifest_path = source_dir.join("manifest.json");
    let mut manifest = Manifest::load(&manifest_path)?;

    let today = Utc::now().date_naive().with_day(1).unwrap();
    let months = month_range(DEFAULT_EARLIEST, today);

    let client = reqwest::blocking::Client::builder()
        .user_agent(OPENDATA_USER_AGENT)
        .connect_timeout(std::time::Duration::from_secs(10))
        // Timeout global (connexion + corps) : sans lui un body qui stalle pend
        // indéfiniment au lieu d'errorer. Un ZIP mensuel reste < quelques Mo →
        // 300 s est large. L'erreur déclenchée est retentée par download_one.
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    for jur in OPENDATA_JURISDICTIONS {
        download_reverses_csv(&client, jur, &source_dir)?;
        let registry = load_reverses_registry(jur, &source_dir)?;
        for yyyymm in &months {
            let key = format!("{jur}/{yyyymm}");
            let url = build_opendata_url(jur, yyyymm);
            let rel = format!("zips/{jur}/{jur}_{yyyymm}.zip");
            let mut entry = manifest.entries.get(&key).cloned().unwrap_or(Entry {
                juridiction: (*jur).to_string(),
                yyyymm: yyyymm.clone(),
                url: url.clone(),
                path: rel.clone(),
                size: None,
                sha256: None,
                last_modified: None,
                last_reversement: None,
                fetched_at: None,
                status: default_status(),
                fully_ingested: false,
                embeddings_complete: false,
            });

            // Fraîcheur pilotée par le registry (plus de HEAD réseau, cf. ADR
            // 0068) : un ZIP déjà `ok` n'est re-téléchargé que si le registry
            // porte un reversement plus récent que le watermark stocké. Absent
            // du registry => aucun reversement => mois stable => skip sans réseau.
            let reg_rev = registry.get(yyyymm).map(String::as_str);
            let abs_path = source_dir.join(&rel);
            if entry.status == "ok"
                && !force
                && abs_path.exists()
                && entry.last_reversement.as_deref() == reg_rev
            {
                continue;
            }
            // Isolation par ZIP : un échec persistant (réseau / body tronqué)
            // après les retries de download_one marque ce mois `error` et passe
            // au suivant — il sera re-tenté au prochain sync (status != ok). Sans
            // ça, un seul mois en échec avortait tout l'opendata (mois restants +
            // juridictions suivantes), cf. crash 2026-06-09.
            if let Err(e) = download_one(&client, &mut entry, &abs_path, force) {
                tracing::warn!(url = %entry.url, error = %e, "download ZIP échoué, on continue");
                entry.status = "error_download".to_string();
            }
            if entry.status == "ok" {
                entry.last_reversement = reg_rev.map(str::to_string);
            }
            manifest.entries.insert(key.clone(), entry);
            manifest.save(&manifest_path)?;
        }
    }
    manifest.save(&manifest_path)?;
    Ok(manifest)
}

/// Charge le registry `documents_reverses` d'une juridiction et calcule, par
/// mois `YYYYMM`, le `max(Date_de_reversement)` normalisé `YYYYMMDDHHMMSS`.
///
/// Source de vérité des modifications opendata : chaque ligne associe un
/// document à son ZIP mensuel (`Nom_du_fichier__zip`) et à sa date de
/// reversement. Le sync en dépend pour la détection de fraîcheur → registry
/// absent/illisible = erreur franche (pas de fallback HEAD silencieux).
fn load_reverses_registry(jur: &str, source_dir: &Path) -> Result<BTreeMap<String, String>> {
    let (_, _, csv_name) = jurisdiction_meta(jur).expect("juridiction opendata connue");
    let path = source_dir.join("documents_reverses").join(csv_name);
    // CSV opendata non-UTF-8 (Latin-1/Windows-1252 : accents dans les noms de
    // fichiers / numéros). Les colonnes lues (ZIP, date) sont ASCII → décodage
    // lossy sans perte sur ce qui nous intéresse.
    let bytes = fs::read(&path).map_err(|e| {
        SourceError::Invalid(format!(
            "registry documents_reverses absent/illisible ({}): {e}",
            path.display()
        ))
    })?;
    let content = String::from_utf8_lossy(&bytes);

    // Entête : Nom_du_fichier__xml;Num;Date_de_lecture;Date_de_reversement;Nom_du_fichier__zip
    let mut max_rev: BTreeMap<String, String> = BTreeMap::new();
    for line in content.lines().skip(1) {
        let cols: Vec<&str> = line.split(';').collect();
        let (Some(rev_raw), Some(zip)) = (cols.get(3), cols.get(4)) else {
            continue;
        };
        let (Some(yyyymm), Some(rev)) = (zip_to_yyyymm(zip), normalize_reversement(rev_raw)) else {
            continue;
        };
        let slot = max_rev.entry(yyyymm).or_insert_with(|| rev.clone());
        if rev > *slot {
            *slot = rev;
        }
    }
    Ok(max_rev)
}

/// `"CAA_202604.zip"` → `"202604"` (segment après le dernier `_`, sans `.zip`).
/// `None` si la forme n'est pas un mois à 6 chiffres.
fn zip_to_yyyymm(zip: &str) -> Option<String> {
    let stem = zip.trim().strip_suffix(".zip")?;
    let yyyymm = stem.rsplit('_').next()?;
    (yyyymm.len() == 6 && yyyymm.bytes().all(|b| b.is_ascii_digit())).then(|| yyyymm.to_string())
}

/// `"01-05-2025 06:09:04:60"` / `"01-07-2025 05:45:14"` → `"20250501060904"`
/// (`YYYYMMDDHHMMSS`, fraction de seconde ignorée) : tri lexical = chronologique.
/// `None` si la forme `DD-MM-YYYY HH:MM:SS[:frac]` n'est pas respectée.
fn normalize_reversement(raw: &str) -> Option<String> {
    let mut parts = raw.split_whitespace();
    let date = parts.next()?;
    let time = parts.next()?;
    let mut d = date.split('-');
    let (dd, mm, yyyy) = (d.next()?, d.next()?, d.next()?);
    let mut t = time.split(':');
    let (hh, min, ss) = (t.next()?, t.next()?, t.next()?);
    if yyyy.len() != 4 || dd.len() > 2 || mm.len() > 2 {
        return None;
    }
    Some(format!("{yyyy}{mm:0>2}{dd:0>2}{hh:0>2}{min:0>2}{ss:0>2}"))
}

/// Télécharge un ZIP : conditional GET (`If-Modified-Since`), 304 → skip, 404 →
/// `not_found` (port de `_download_one`, sans la reprise `Range`/`.part`/`tqdm`).
fn download_one(
    client: &reqwest::blocking::Client,
    entry: &mut Entry,
    abs_path: &Path,
    force: bool,
) -> Result<()> {
    if let Some(parent) = abs_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut req = client.get(&entry.url);
    if !force && abs_path.exists() {
        if let Some(lm) = &entry.last_modified {
            req = req.header(reqwest::header::IF_MODIFIED_SINCE, lm);
        }
    }
    // Streamé sur disque (RAM ~constante : un ZIP mensuel peut peser des centaines
    // de Mo). Sur 200, le corps est écrit dans `.part` ; 304/404/autre n'écrivent rien.
    let part_path = path_with_added_extension(abs_path, "part");
    let (status, last_mod, _n) = get_to_file_retrying(&entry.url, &part_path, || {
        req.try_clone()
            .expect("requête GET sans corps stream : clonable")
            .send()
    })?;
    if status == 304 {
        entry.status = "ok".to_string();
        return Ok(());
    }
    if status == 404 {
        entry.status = "not_found".to_string();
        return Ok(());
    }
    if status != 200 {
        tracing::warn!(url = %entry.url, status, "download HTTP error");
        entry.status = format!("error_http_{status}");
        return Ok(());
    }
    fs::rename(&part_path, abs_path)?;

    entry.size = Some(fs::metadata(abs_path)?.len());
    entry.sha256 = Some(sha256_of(abs_path)?);
    entry.last_modified = last_mod;
    entry.fetched_at = Some(Manifest::now_iso_seconds());
    entry.status = "ok".to_string();
    entry.fully_ingested = false;
    entry.embeddings_complete = false;
    Ok(())
}

/// Télécharge le CSV `documents_reverses` d'une juridiction (port de
/// `_download_reverses_csv`). Absence (404) silencieuse.
fn download_reverses_csv(
    client: &reqwest::blocking::Client,
    jur: &str,
    data_dir: &Path,
) -> Result<()> {
    let (sub, _, csv_name) = jurisdiction_meta(jur).expect("juridiction opendata connue");
    let url = format!("{OPENDATA_BASE_URL}/{sub}/{csv_name}");
    let dst = data_dir.join("documents_reverses").join(csv_name);
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let (status, _, body) = get_with_body_retrying(&url, || client.get(&url).send())?;
    match status {
        200 => {
            let bytes = body.expect("statut 200 → corps lu par get_with_body_retrying");
            fs::write(&dst, &bytes)?;
            tracing::info!(jur, octets = bytes.len(), "manifeste reverses sauvé");
        }
        404 => tracing::info!(jur, "pas de manifeste reverses"),
        s => tracing::warn!(jur, status = s, "manifeste reverses HTTP error"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_opendata_url_matches_python() {
        assert_eq!(
            build_opendata_url("TA", "202603"),
            "https://opendata.justice-administrative.fr/DTA/2026/03/TA_202603.zip"
        );
        assert_eq!(
            build_opendata_url("CE", "202101"),
            "https://opendata.justice-administrative.fr/DCE/2021/01/CE_202101.zip"
        );
    }

    #[test]
    fn zip_to_yyyymm_extracts_month() {
        assert_eq!(zip_to_yyyymm("CAA_202604.zip"), Some("202604".to_string()));
        assert_eq!(zip_to_yyyymm("TA_202207.zip"), Some("202207".to_string()));
        assert_eq!(zip_to_yyyymm("CE_202112.zip"), Some("202112".to_string()));
        assert_eq!(zip_to_yyyymm("CAA_2026.zip"), None); // pas 6 chiffres
        assert_eq!(zip_to_yyyymm("garbage"), None);
    }

    #[test]
    fn normalize_reversement_formats() {
        // TA/CAA : fraction de seconde de longueur variable, ignorée.
        assert_eq!(
            normalize_reversement("01-05-2025 06:09:04:60"),
            Some("20250501060904".to_string())
        );
        assert_eq!(
            normalize_reversement("05-06-2026 04:59:12:595"),
            Some("20260605045912".to_string())
        );
        // CE : pas de fraction.
        assert_eq!(
            normalize_reversement("01-07-2025 05:45:14"),
            Some("20250701054514".to_string())
        );
        // Tri lexical = ordre chronologique.
        assert!(
            normalize_reversement("31-12-2025 23:59:59").unwrap()
                < normalize_reversement("01-01-2026 00:00:00").unwrap()
        );
        assert_eq!(normalize_reversement("pas une date"), None);
    }

    #[test]
    fn registry_keeps_max_reversement_per_zip() {
        let dir = tempfile::tempdir().unwrap();
        let rev_dir = dir.path().join("documents_reverses");
        fs::create_dir_all(&rev_dir).unwrap();
        // csv_name de CAA = "CAA_documents_reverses.csv".
        let csv =
            "Nom_du_fichier__xml;Num;Date_de_lecture;Date_de_reversement;Nom_du_fichier__zip\n\
            DCA_a.xml;a;01/01/2026;01-05-2025 06:09:04:60;CAA_202503.zip\n\
            DCA_b.xml;b;01/01/2026;08-05-2025 06:14:10:77;CAA_202503.zip\n\
            DCA_c.xml;c;01/01/2026;06-05-2025 06:07:24:29;CAA_202504.zip\n";
        fs::write(rev_dir.join("CAA_documents_reverses.csv"), csv).unwrap();

        let map = load_reverses_registry("CAA", dir.path()).unwrap();
        // 202503 : max(01-05, 08-05) = 08-05.
        assert_eq!(
            map.get("202503").map(String::as_str),
            Some("20250508061410")
        );
        assert_eq!(
            map.get("202504").map(String::as_str),
            Some("20250506060724")
        );
    }

    #[test]
    fn registry_absent_is_hard_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_reverses_registry("CAA", dir.path()).is_err());
    }
}
