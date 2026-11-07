//! The Autorité de la concurrence's full-text dataset on data.gouv.fr, cached as `<dated segment>.json`
//! of its URL, with `<dated segment>.written` once every record of it is written.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::{Result, SourceError};
use crate::registries::{datagouv_latest_resource, download_resource};

const DATASET: &str = "6571e9dd597d4ef5ee64a1a8";
const FULL_TEXT_FILE: &str = "adlc-texte-complet-publications.json";

/// Downloads the latest publication unless the cache holds it, and returns its path.
pub fn fetch_latest(dir: &Path) -> Result<PathBuf> {
    let mut res = datagouv_latest_resource(DATASET, |t| t == FULL_TEXT_FILE, |f| f == "json")?;
    let dated = res
        .url
        .rsplit('/')
        .nth(1)
        .ok_or_else(|| SourceError::Invalid(format!("undated resource URL {}", res.url)))?;
    res.filename = format!("{dated}.json");
    download_resource(&res, dir)
}

pub fn written_marker(publication: &Path) -> PathBuf {
    publication.with_extension("written")
}

/// Removes every file of `dir` that is not `publication` or its marker.
pub fn prune_older(dir: &Path, publication: &Path) -> Result<()> {
    let marker = written_marker(publication);
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path != publication && path != marker {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

pub fn read_records(publication: &Path) -> Result<Vec<Value>> {
    Ok(sonic_rs::from_slice(&fs::read(publication)?)?)
}
