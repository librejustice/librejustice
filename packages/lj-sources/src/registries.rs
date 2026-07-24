//! Registres d'entités (ADR 0179) : découverte data.gouv, download streamé,
//! itération CSV depuis les stocks (SIRENE, RNA, annuaire des avocats).
//!
//! Les ressources data.gouv ont des URLs datées (une par publication) ; seul
//! l'id de dataset est stable. La découverte interroge l'API dataset et prend
//! la ressource la plus récente qui matche le filtre de l'appelant.

use crate::downloader::{get_to_file_retrying, get_with_body_retrying};
use crate::error::{Result, SourceError};
use std::borrow::Cow;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const DATAGOUV_API: &str = "https://www.data.gouv.fr/api/1/datasets";
const USER_AGENT: &str = "librejustice-registries/0.1";

/// Une ressource de dataset data.gouv (fichier publié).
#[derive(Debug, Clone)]
pub struct RegistryResource {
    pub title: String,
    pub url: String,
    pub filesize: Option<u64>,
    /// Nom de fichier local (dernier segment de l'URL).
    pub filename: String,
}

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(14400))
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("reqwest client build")
}

/// Ressource la plus récente (par `created_at`) du dataset dont le titre
/// passe `title_ok` et le format `format_ok`.
pub fn datagouv_latest_resource(
    dataset: &str,
    title_ok: impl Fn(&str) -> bool,
    format_ok: impl Fn(&str) -> bool,
) -> Result<RegistryResource> {
    let url = format!("{DATAGOUV_API}/{dataset}/");
    let c = client();
    let (status, _, body) = get_with_body_retrying(&url, || c.get(&url).send())?;
    let Some(body) = (status == 200).then_some(body).flatten() else {
        return Err(SourceError::Invalid(format!(
            "data.gouv dataset {dataset}: HTTP {status}"
        )));
    };
    let v: sonic_rs::Value = sonic_rs::from_slice(&body)?;
    use sonic_rs::JsonValueTrait;
    let resources = &v["resources"];
    let mut best: Option<(String, RegistryResource)> = None;
    let mut idx = 0usize;
    loop {
        let r = &resources[idx];
        if r.is_null() {
            break;
        }
        idx += 1;
        let title = r["title"].as_str().unwrap_or_default();
        let format = r["format"].as_str().unwrap_or_default();
        let Some(url) = r["url"].as_str() else {
            continue;
        };
        if !title_ok(title) || !format_ok(format) {
            continue;
        }
        let created = r["created_at"].as_str().unwrap_or_default().to_string();
        if best.as_ref().is_some_and(|(c0, _)| *c0 >= created) {
            continue;
        }
        let filename = url.rsplit('/').next().unwrap_or(title).to_string();
        best = Some((
            created,
            RegistryResource {
                title: title.to_string(),
                url: url.to_string(),
                filesize: r["filesize"].as_u64(),
                filename,
            },
        ));
    }
    best.map(|(_, r)| r).ok_or_else(|| {
        SourceError::Invalid(format!(
            "data.gouv dataset {dataset}: aucune ressource ne matche"
        ))
    })
}

/// Télécharge la ressource sous `dir` (streamé). Manifeste minimal : un
/// fichier local de la taille annoncée est considéré à jour (les
/// publications data.gouv sont immuables — nouveau fichier = nouveau nom).
pub fn download_resource(res: &RegistryResource, dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let dst = dir.join(&res.filename);
    if let (Ok(meta), Some(size)) = (fs::metadata(&dst), res.filesize) {
        if meta.len() == size {
            tracing::info!(file = %dst.display(), size, "registre déjà téléchargé, skip");
            return Ok(dst);
        }
    }
    tracing::info!(url = %res.url, file = %dst.display(), "download registre");
    let c = client();
    let (status, _, written) = get_to_file_retrying(&res.url, &dst, || c.get(&res.url).send())?;
    if status != 200 {
        return Err(SourceError::Invalid(format!(
            "download {}: HTTP {status}",
            res.url
        )));
    }
    tracing::info!(file = %dst.display(), written, "registre téléchargé");
    Ok(dst)
}

/// Décode un champ CSV : UTF-8 si valide, sinon Latin-1 (les stocks RNA et
/// l'annuaire des avocats mélangent les deux selon les millésimes).
pub fn decode_field(bytes: &[u8]) -> Cow<'_, str> {
    match std::str::from_utf8(bytes) {
        Ok(s) => Cow::Borrowed(s),
        Err(_) => Cow::Owned(bytes.iter().map(|&b| b as char).collect()),
    }
}

/// Itère les enregistrements CSV de `path` — un `.zip` (tous ses membres
/// `.csv`, streamés sans extraction) ou un `.csv` nu. `f(headers, record)`
/// reçoit les en-têtes du membre courant (BOM retiré) et chaque ligne.
pub fn for_each_csv_record(
    path: &Path,
    delimiter: u8,
    mut f: impl FnMut(&[String], &csv::ByteRecord) -> Result<()>,
) -> Result<()> {
    let is_zip = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    if is_zip {
        let file = fs::File::open(path)?;
        let mut zip = zip::ZipArchive::new(file)?;
        for i in 0..zip.len() {
            let entry = zip.by_index(i)?;
            if !entry.name().to_ascii_lowercase().ends_with(".csv") {
                continue;
            }
            read_csv(entry, delimiter, &mut f)?;
        }
        Ok(())
    } else {
        read_csv(fs::File::open(path)?, delimiter, &mut f)
    }
}

fn read_csv(
    reader: impl Read,
    delimiter: u8,
    f: &mut impl FnMut(&[String], &csv::ByteRecord) -> Result<()>,
) -> Result<()> {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .from_reader(std::io::BufReader::with_capacity(1 << 20, reader));
    let headers: Vec<String> = rdr
        .byte_headers()
        .map_err(|e| SourceError::Invalid(format!("csv headers: {e}")))?
        .iter()
        .map(|h| decode_field(h).trim_start_matches('\u{feff}').to_string())
        .collect();
    let mut record = csv::ByteRecord::new();
    loop {
        match rdr.read_byte_record(&mut record) {
            Ok(true) => f(&headers, &record)?,
            Ok(false) => return Ok(()),
            Err(e) => return Err(SourceError::Invalid(format!("csv record: {e}"))),
        }
    }
}
