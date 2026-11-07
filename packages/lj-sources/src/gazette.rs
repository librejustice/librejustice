//! The DILA bulk of BODACC and JOAFE: a mirror of the published directories, read back as containers of XML documents, one parution each.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use lj_gazette::Source;
use regex::Regex;

use crate::dila::{http_client, with_stall_watchdog, DILA_BASE_URL};
use crate::downloader::{get_to_file_retrying, get_with_body_retrying};
use crate::error::{Result, SourceError};

fn remote_root(source: Source) -> &'static str {
    match source {
        Source::Bodacc => "BODACC",
        Source::Joafe => "ASSOCIATIONS",
    }
}

fn current_dirs(source: Source, listing_of_root: &[Entry]) -> Vec<String> {
    match source {
        Source::Bodacc => vec!["FluxAnneeCourante/".to_string()],
        Source::Joafe => listing_of_root
            .iter()
            .filter(|e| e.name.starts_with("ASS_") && e.name.ends_with('/'))
            .map(|e| e.name.clone())
            .collect(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Current,
    Stock,
}

#[derive(Debug, Clone)]
struct Entry {
    name: String,
    modified: Option<SystemTime>,
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn parse_listing(html: &str) -> Vec<Entry> {
    static LINE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = LINE.get_or_init(|| {
        Regex::new(r#"href="([^"?/][^"]*)"[^\n]*?(\d{4})-(\d\d)-(\d\d) (\d\d):(\d\d)"#)
            .expect("listing regex")
    });
    re.captures_iter(html)
        .map(|c| {
            let n = |i: usize| c[i].parse::<i64>().unwrap_or(0);
            let secs =
                days_from_civil(n(2), n(3) as u32, n(4) as u32) * 86_400 + n(5) * 3600 + n(6) * 60;
            Entry {
                name: c[1].to_string(),
                modified: u64::try_from(secs)
                    .ok()
                    .map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s)),
            }
        })
        .collect()
}

/// The JOAFE `new/` tree repeats from 2005 what the yearly `ASS_` archives carry; `ASS_2004` holds two weeks of its year.
fn wanted(source: Source, rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let archive = [".taz", ".tar", ".tar.gz", ".tgz", ".zip", ".xml"]
        .iter()
        .any(|e| name.ends_with(e));
    if !archive || name == "html.zip" {
        return false;
    }
    match source {
        Source::Bodacc => true,
        Source::Joafe => match rel.strip_prefix("FluxHistorique/new/") {
            Some(rest) => rest
                .get(0..4)
                .and_then(|y| y.parse::<u32>().ok())
                .is_some_and(|y| y < 2005),
            None => true,
        },
    }
}

fn wanted_dir(source: Source, rel: &str) -> bool {
    !rel.starts_with("Documentation")
        && !rel.starts_with("Modele")
        && wanted(source, &format!("{rel}x.xml"))
}

pub struct SyncReport {
    pub listed: usize,
    pub downloaded: usize,
    pub bytes: u64,
    pub forbidden: usize,
}

fn list(client: &reqwest::blocking::Client, url: &str) -> Result<Vec<Entry>> {
    let (status, _, body) =
        with_stall_watchdog(&format!("GET {url}"), Duration::from_secs(300), || {
            get_with_body_retrying(url, || client.get(url).send())
        })?;
    if status != 200 {
        return Err(SourceError::Invalid(format!(
            "listing {url}: status {status}"
        )));
    }
    Ok(parse_listing(&String::from_utf8_lossy(
        &body.unwrap_or_default(),
    )))
}

/// A listed file is fetched when absent or dated a day past the local copy, the listing's clock not being UTC.
pub fn sync(source: Source, dir: &Path, scope: Scope) -> Result<SyncReport> {
    let client = http_client()?;
    let base = format!("{DILA_BASE_URL}/{}/", remote_root(source));
    let root = list(&client, &base)?;
    let mut todo: Vec<String> = current_dirs(source, &root);
    if scope == Scope::Stock {
        todo.push("FluxHistorique/".to_string());
    }
    let mut report = SyncReport {
        listed: 0,
        downloaded: 0,
        bytes: 0,
        forbidden: 0,
    };
    while let Some(rel_dir) = todo.pop() {
        for e in list(&client, &format!("{base}{rel_dir}"))? {
            let rel = format!("{rel_dir}{}", e.name);
            if e.name.ends_with('/') {
                if scope == Scope::Stock && wanted_dir(source, &rel) {
                    todo.push(rel);
                }
                continue;
            }
            if !wanted(source, &rel) {
                continue;
            }
            report.listed += 1;
            let dst = dir.join(&rel);
            let local = fs::metadata(&dst).ok().and_then(|m| m.modified().ok());
            let stale = match (local, e.modified) {
                (None, _) => true,
                (Some(l), Some(r)) => r > l + Duration::from_secs(26 * 3600),
                (Some(_), None) => false,
            };
            if !stale {
                continue;
            }
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            let tmp = dst.with_extension("part");
            let url = format!("{base}{rel}");
            let (status, _, n) =
                with_stall_watchdog(&format!("GET {url}"), Duration::from_secs(900), || {
                    get_to_file_retrying(&url, &tmp, || client.get(&url).send())
                })?;
            // The JOAFE FluxHistorique lists 20070505.xml and 20120324.xml, which answer 403.
            if status == 403 {
                let _ = fs::remove_file(&tmp);
                report.forbidden += 1;
                tracing::warn!(source = source.slug(), file = %rel, "gazette file listed but forbidden, skipped");
                continue;
            }
            if status != 200 {
                return Err(SourceError::Invalid(format!("{url}: status {status}")));
            }
            fs::rename(&tmp, &dst)?;
            if let Some(m) = e.modified {
                fs::File::options()
                    .write(true)
                    .open(&dst)?
                    .set_modified(m)?;
            }
            report.downloaded += 1;
            report.bytes += n;
            tracing::info!(source = source.slug(), file = %rel, bytes = n, "gazette file downloaded");
        }
    }
    Ok(report)
}

pub struct Container {
    pub origin: String,
    pub docs: Vec<Vec<u8>>,
}

fn gunzip_all(mut bytes: Vec<u8>) -> Result<Vec<u8>> {
    while bytes.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::with_capacity(bytes.len() * 6);
        flate2::read::MultiGzDecoder::new(&bytes[..]).read_to_end(&mut out)?;
        bytes = out;
    }
    if bytes.starts_with(&[0x1f, 0x9d]) {
        bytes = unlzw(&bytes)?;
    }
    if bytes.starts_with(b"7z\xbc\xaf\x27\x1c") {
        bytes = un7z(bytes)?;
    }
    Ok(bytes)
}

/// The one file of a 7z archive, which a few 2024 BODACC containers are.
fn un7z(bytes: Vec<u8>) -> Result<Vec<u8>> {
    let invalid = |e: sevenz_rust2::Error| SourceError::Invalid(format!("7z: {e}"));
    let mut reader = sevenz_rust2::ArchiveReader::new(
        std::io::Cursor::new(bytes),
        sevenz_rust2::Password::empty(),
    )
    .map_err(invalid)?;
    let mut files = Vec::new();
    reader
        .for_each_entries(|entry, r| {
            if !entry.is_directory() {
                let mut buf = Vec::new();
                r.read_to_end(&mut buf)?;
                files.push(buf);
            }
            Ok(true)
        })
        .map_err(invalid)?;
    match <[Vec<u8>; 1]>::try_from(files) {
        Ok([one]) => Ok(one),
        Err(files) => Err(SourceError::Invalid(format!(
            "7z holding {} files",
            files.len()
        ))),
    }
}

/// `compress(1)` data, which `gzip -d` still decodes.
fn unlzw(bytes: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut child = std::process::Command::new("gzip")
        .arg("-dc")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let input = bytes.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let out = child.wait_with_output()?;
    writer
        .join()
        .map_err(|_| SourceError::Invalid("gzip -dc writer panicked".into()))??;
    if !out.status.success() {
        return Err(SourceError::Invalid(format!(
            "gzip -dc (LZW): {}",
            out.status
        )));
    }
    Ok(out.stdout)
}

fn is_xml(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".xml")
}

/// Some 2008-2012 JOAFE containers carry a binary extract named `.xml` beside the XML.
fn is_markup(doc: &[u8]) -> bool {
    doc.strip_prefix(b"\xEF\xBB\xBF")
        .unwrap_or(doc)
        .trim_ascii_start()
        .starts_with(b"<")
}

fn octal(field: &[u8]) -> Option<usize> {
    let digits = std::str::from_utf8(field)
        .ok()?
        .trim_matches(|c: char| c == '\0' || c == ' ');
    if digits.is_empty() {
        return Some(0);
    }
    usize::from_str_radix(digits, 8).ok()
}

/// The regular files of an in-memory tar; some 2017-2021 BODACC containers have space-padded headers whose checksum fails.
fn tar_files(raw: &[u8]) -> Result<Vec<(String, &[u8])>> {
    let mut files = Vec::new();
    let mut at = 0;
    while at + 512 <= raw.len() {
        let header = &raw[at..at + 512];
        if header.iter().all(|&b| b == 0) {
            break;
        }
        let name = String::from_utf8_lossy(&header[..100])
            .trim_matches(|c: char| c == '\0' || c == ' ')
            .to_string();
        let size = octal(&header[124..136])
            .ok_or_else(|| SourceError::Invalid(format!("tar member {name}: size field")))?;
        let data = raw
            .get(at + 512..at + 512 + size)
            .ok_or_else(|| SourceError::Invalid(format!("tar member {name}: truncated")))?;
        if matches!(header[156], b'0' | 0) {
            files.push((name, data));
        }
        at += 512 + size.div_ceil(512) * 512;
    }
    Ok(files)
}

fn taz_docs(bytes: Vec<u8>) -> Result<Vec<Vec<u8>>> {
    let raw = gunzip_all(bytes)?;
    if is_markup(&raw) {
        return Ok(vec![raw]);
    }
    Ok(tar_files(&raw)?
        .into_iter()
        .filter(|(name, data)| is_xml(name) && is_markup(data))
        .map(|(_, data)| data.to_vec())
        .collect())
}

/// A tar stream under as many gzip layers as it carries: some yearly archives are gzipped twice.
fn tar_stream(file: fs::File) -> Result<Box<dyn Read>> {
    let mut reader: Box<dyn BufRead> = Box::new(BufReader::new(file));
    while reader.fill_buf()?.starts_with(&[0x1f, 0x8b]) {
        reader = Box::new(BufReader::new(flate2::read::MultiGzDecoder::new(reader)));
    }
    Ok(reader)
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for e in fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            files_under(&p, out)?;
        } else {
            out.push(p);
        }
    }
    Ok(())
}

fn in_scope(source: Source, scope: Scope, rel: &str) -> bool {
    match (scope, source) {
        (Scope::Stock, _) => true,
        (Scope::Current, Source::Bodacc) => rel.starts_with("FluxAnneeCourante/"),
        (Scope::Current, Source::Joafe) => rel.starts_with("ASS_"),
    }
}

/// `affaire_<parution>_<n>.xml`: one 1997-2004 JOAFE notice without its parution's header.
fn unitary_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^affaire_\d{8}_\d+\.xml$").expect("unitary regex"))
}

/// In path order; the one-notice files of a zip directory make one container.
pub fn for_each_container(
    source: Source,
    dir: &Path,
    scope: Scope,
    mut f: impl FnMut(Container) -> Result<()>,
) -> Result<()> {
    let mut files = Vec::new();
    files_under(dir, &mut files)?;
    files.sort();
    for path in files {
        let rel = path
            .strip_prefix(dir)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        if !wanted(source, &rel) || !in_scope(source, scope, &rel) {
            continue;
        }
        container_file(&rel, &path, &mut f)
            .map_err(|e| SourceError::Invalid(format!("{rel}: {e}")))?;
    }
    Ok(())
}

fn container_file(
    rel: &str,
    path: &Path,
    f: &mut impl FnMut(Container) -> Result<()>,
) -> Result<()> {
    let name = rel.rsplit('/').next().unwrap_or(rel).to_string();
    if name.ends_with(".taz") {
        f(Container {
            origin: rel.to_string(),
            docs: taz_docs(fs::read(path)?)?,
        })?;
    } else if is_xml(&name) {
        f(Container {
            origin: rel.to_string(),
            docs: vec![fs::read(path)?],
        })?;
    } else if name.ends_with(".zip") {
        let mut zip = zip::ZipArchive::new(fs::File::open(path)?)?;
        let mut groups: Vec<(String, Vec<Vec<u8>>)> = Vec::new();
        let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            let member = entry.name().to_string();
            if entry.is_dir() || !is_xml(&member) {
                continue;
            }
            let mut buf = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut buf)?;
            let unitary = member
                .rsplit('/')
                .next()
                .is_some_and(|b| unitary_re().is_match(b));
            let key = match member.rsplit_once('/') {
                Some((parent, _)) if unitary => parent.to_string(),
                _ => member,
            };
            match index.get(&key) {
                Some(&i) => groups[i].1.push(buf),
                None => {
                    index.insert(key.clone(), groups.len());
                    groups.push((key, vec![buf]));
                }
            }
        }
        for (key, docs) in groups {
            f(Container {
                origin: format!("{rel}!{key}"),
                docs,
            })?;
        }
    } else {
        let mut archive = tar::Archive::new(tar_stream(fs::File::open(path)?)?);
        for entry in archive.entries()? {
            let mut entry = entry?;
            let member = entry.path()?.to_string_lossy().into_owned();
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let mut buf = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut buf)?;
            let origin = format!("{rel}!{member}");
            if member.ends_with(".taz") {
                f(Container {
                    origin,
                    docs: taz_docs(buf)?,
                })?;
            } else if is_xml(&member) {
                f(Container {
                    origin,
                    docs: vec![buf],
                })?;
            }
        }
    }
    Ok(())
}
