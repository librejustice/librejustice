//! Service-Public.fr « vos droits » sheets: the DILA feeds, one zip per audience.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use lj_parse::ServicePublicSheet;

use crate::downloader::http::get_to_file_retrying;
use crate::error::{Result, SourceError};

const FEED_BASE: &str = "https://lecomarquage.service-public.fr/vdd/3.4";

/// The audiences read, a sheet in both keeping the first. `asso`, rebuilt less
/// often, holds sheets that `part` serves newer.
pub const AUDIENCES: &[&str] = &["part", "pro"];

const USER_AGENT: &str = "librejustice-ingest/1.0 (+https://librejustice.fr)";

/// Downloads each audience's latest zip under `dir`, in [`AUDIENCES`] order.
pub fn fetch_service_public(dir: &Path) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(dir)?;
    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(900))
        .build()?;
    let mut zips = Vec::new();
    for audience in AUDIENCES {
        let url = format!("{FEED_BASE}/{audience}/zip/vosdroits-latest.zip");
        let dst = dir.join(format!("{audience}.zip"));
        let tmp = dst.with_extension("zip.part");
        let (status, _, octets) = crate::dila::with_stall_watchdog(
            "GET service-public",
            Duration::from_secs(300),
            || get_to_file_retrying(&url, &tmp, || client.get(&url).send()),
        )?;
        if status != 200 {
            return Err(SourceError::Invalid(format!("{url}: status {status}")));
        }
        fs::rename(&tmp, &dst)?;
        tracing::info!(audience, octets, "service_public downloaded");
        zips.push(dst);
    }
    Ok(zips)
}

/// The sheets of one zip, its `F<n>.xml` members, and the count of members that
/// did not parse, each logged.
pub fn read_sheets(zip_path: &Path) -> Result<(Vec<ServicePublicSheet>, usize)> {
    let mut archive = zip::ZipArchive::new(fs::File::open(zip_path)?)?;
    let (mut sheets, mut errors) = (Vec::new(), 0);
    for i in 0..archive.len() {
        let mut member = archive.by_index(i)?;
        let name = member
            .name()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string();
        let is_sheet = name
            .strip_prefix('F')
            .and_then(|n| n.strip_suffix(".xml"))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        if !is_sheet {
            continue;
        }
        let mut raw = Vec::with_capacity(member.size() as usize);
        member.read_to_end(&mut raw)?;
        match lj_parse::parse_service_public_sheet(&raw) {
            Ok(Some(sheet)) => sheets.push(sheet),
            Ok(None) => {}
            Err(e) => {
                tracing::error!(zip = %zip_path.display(), member = %name, error = %e, "service_public: parse failed");
                errors += 1;
            }
        }
    }
    Ok((sheets, errors))
}
