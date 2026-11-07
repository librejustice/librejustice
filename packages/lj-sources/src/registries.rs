//! Registres d'entités : découverte data.gouv, download streamé, itération CSV
//! depuis les stocks (SIRENE, RNA, annuaire des avocats).
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

/// Toutes les ressources du dataset dont le titre passe `title_ok` et le
/// format `format_ok`, par `created_at` croissant — l'ordre de publication,
/// qui est celui du rejeu chronologique.
pub fn datagouv_resources(
    dataset: &str,
    title_ok: impl Fn(&str) -> bool,
    format_ok: impl Fn(&str) -> bool,
) -> Result<Vec<RegistryResource>> {
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
    let mut out: Vec<(String, RegistryResource)> = Vec::new();
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
        let filename = url.rsplit('/').next().unwrap_or(title).to_string();
        out.push((
            r["created_at"].as_str().unwrap_or_default().to_string(),
            RegistryResource {
                title: title.to_string(),
                url: url.to_string(),
                filesize: r["filesize"].as_u64(),
                filename,
            },
        ));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out.into_iter().map(|(_, r)| r).collect())
}

/// Ressource la plus récente (par `created_at`) du dataset dont le titre
/// passe `title_ok` et le format `format_ok`.
pub fn datagouv_latest_resource(
    dataset: &str,
    title_ok: impl Fn(&str) -> bool,
    format_ok: impl Fn(&str) -> bool,
) -> Result<RegistryResource> {
    datagouv_resources(dataset, title_ok, format_ok)?
        .pop()
        .ok_or_else(|| {
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
            tracing::info!(file = %dst.display(), size, "registry already downloaded, skip");
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
    tracing::info!(file = %dst.display(), written, "registry downloaded");
    Ok(dst)
}

/// Décode un champ CSV : UTF-8 si valide, sinon Windows-1252 (les stocks RNA et
/// l'annuaire des avocats mélangent les deux selon les millésimes).
pub fn decode_field(bytes: &[u8]) -> Cow<'_, str> {
    match std::str::from_utf8(bytes) {
        Ok(s) => Cow::Borrowed(s),
        Err(_) => Cow::Owned(bytes.iter().map(|&b| cp1252(b)).collect()),
    }
}

/// Unassigned bytes 0x81, 0x8D, 0x8F, 0x90 and 0x9D keep their Latin-1 control.
fn cp1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{81}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{8D}',
        '\u{017D}', '\u{8F}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}',
        '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}',
        '\u{9D}', '\u{017E}', '\u{0178}',
    ];
    match b {
        0x80..=0x9F => HIGH[usize::from(b - 0x80)],
        _ => char::from(b),
    }
}

/// Itère les enregistrements CSV de `path` — un `.zip` (tous ses membres
/// `.csv`, streamés sans extraction) ou un `.csv` nu. `f(headers, record)`
/// reçoit les en-têtes du membre courant (BOM et blancs de bord retirés —
/// les millésimes de l'annuaire des avocats en sèment) et chaque ligne.
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
        .map(|h| {
            decode_field(h)
                .trim_start_matches('\u{feff}')
                .trim()
                .to_string()
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_field_reads_windows_1252() {
        assert_eq!(decode_field("Ploÿart".as_bytes()), "Ploÿart");
        assert_eq!(decode_field(b"Plo\xffart"), "Ploÿart");
        assert_eq!(decode_field(b"PLO\x9fART"), "PLO\u{0178}ART");
        assert_eq!(decode_field(b"d\x92Aubrac"), "d\u{2019}Aubrac");
        assert_eq!(decode_field(b"\x8d"), "\u{8d}");
    }
}
