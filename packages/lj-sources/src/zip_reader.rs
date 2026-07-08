//! Lecteur d'archives ZIP opendata (port de `sources/zip_reader.py`).
//!
//! Chaque XML est décompressé indépendamment via le central directory du ZIP
//! (crate `zip`), comme `zipfile` côté Python. La mémoire utilisée est celle du
//! plus gros XML lu, pas celle de l'archive entière.

use crate::error::{Result, SourceError};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use zip::ZipArchive;

/// `true` si le membre est un fichier `.xml` (insensible à la casse), pas un
/// répertoire. Port fidèle de `info.is_dir() / .lower().endswith(".xml")`.
fn is_xml_member(name: &str, is_dir: bool) -> bool {
    !is_dir && name.to_lowercase().ends_with(".xml")
}

/// Liste les noms des membres XML d'un ZIP via le central directory seul.
pub fn decision_members(zip_path: &Path) -> Result<Vec<String>> {
    let file = File::open(zip_path)?;
    let mut archive = ZipArchive::new(file)?;
    let mut out = Vec::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        if is_xml_member(entry.name(), entry.is_dir()) {
            out.push(entry.name().to_string());
        }
    }
    Ok(out)
}

/// Itère les entrées XML d'un ZIP en renvoyant `(nom_membre, contenu)`.
///
/// Le port Python est un générateur ; ici on matérialise un `Vec` (l'appelant
/// peut itérer dessus). L'ordre suit le central directory, comme `infolist()`.
pub fn iter_decisions(zip_path: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let file = File::open(zip_path)?;
    let mut archive = ZipArchive::new(file)?;
    let mut out = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        if !is_xml_member(entry.name(), entry.is_dir()) {
            continue;
        }
        let name = entry.name().to_string();
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf)?;
        out.push((name, buf));
    }
    Ok(out)
}

/// Lit un unique membre — accès ciblé pour consultation.
pub fn open_decision(zip_path: &Path, member: &str) -> Result<Vec<u8>> {
    let file = File::open(zip_path)?;
    let mut archive = ZipArchive::new(file)?;
    let mut entry = archive.by_name(member)?;
    let mut buf = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Compte les décisions d'une archive.
pub fn count_decisions(zip_path: &Path) -> Result<usize> {
    Ok(decision_members(zip_path)?.len())
}

/// Erreur "membre absent" homogène (`zip` renvoie `FileNotFound`, on garde le
/// type d'erreur d'origine ; cette helper sert la lisibilité des tests).
#[allow(dead_code)]
fn member_not_found(member: &str) -> SourceError {
    SourceError::Invalid(format!("membre absent du zip: {member}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    /// Construit un ZIP en mémoire avec quelques membres, dont des non-XML et un
    /// répertoire, pour figer le filtrage `*.xml` (insensible à la casse).
    fn build_fixture() -> tempfile::NamedTempFile {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = File::create(tmp.path()).unwrap();
        let mut zw = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default();

        zw.start_file("decisions/a.xml", opts).unwrap();
        zw.write_all(b"<doc>a</doc>").unwrap();
        zw.start_file("decisions/B.XML", opts).unwrap();
        zw.write_all(b"<doc>b</doc>").unwrap();
        zw.start_file("readme.txt", opts).unwrap();
        zw.write_all(b"ignore me").unwrap();
        zw.add_directory("decisions/", opts).unwrap();
        zw.finish().unwrap();
        tmp
    }

    #[test]
    fn lists_only_xml_members_case_insensitive() {
        let tmp = build_fixture();
        let mut members = decision_members(tmp.path()).unwrap();
        members.sort();
        assert_eq!(members, vec!["decisions/B.XML", "decisions/a.xml"]);
        assert_eq!(count_decisions(tmp.path()).unwrap(), 2);
    }

    #[test]
    fn reads_member_bytes() {
        let tmp = build_fixture();
        let bytes = open_decision(tmp.path(), "decisions/a.xml").unwrap();
        assert_eq!(bytes, b"<doc>a</doc>");
    }

    #[test]
    fn iterates_all_xml() {
        let tmp = build_fixture();
        let mut decisions = iter_decisions(tmp.path()).unwrap();
        decisions.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0].0, "decisions/B.XML");
        assert_eq!(decisions[1].1, b"<doc>a</doc>");
    }
}
