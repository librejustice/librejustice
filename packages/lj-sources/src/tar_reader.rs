//! Lecteur d'archives `.tar.gz` bulk DILA/LEGI (miroir de `zip_reader.rs`, ADR 0093).
//!
//! Contrairement au ZIP (central directory = accès aléatoire), un tar.gz n'a
//! pas d'index : on le lit en streaming via `GzDecoder` + `tar::Archive`, un
//! membre à la fois. La mémoire utilisée est celle du plus gros membre lu, pas
//! celle de l'archive entière — indispensable pour les **stocks globaux** (LEGI
//! 1,5 M+ membres, plusieurs Go décompressés). `tar` remonte ses erreurs en
//! `std::io::Error`, captées par `SourceError::Io`.

use crate::error::Result;
use flate2::read::GzDecoder;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use tar::Archive;

/// Ouvre l'archive `.tar.gz` en streaming (décompression gzip à la volée).
fn open_archive(tar_path: &Path) -> Result<Archive<GzDecoder<File>>> {
    let file = File::open(tar_path)?;
    Ok(Archive::new(GzDecoder::new(file)))
}

/// Streame TOUS les membres-fichiers (pas les répertoires) d'un `.tar.gz` en
/// appelant `f(nom, contenu)` un membre à la fois — RAM ~constante (= plus gros
/// membre), JAMAIS l'archive entière. Le caller classe le membre (`.xml`
/// article/texte/décision, `.dat` suppressions, PDF de présentation ignoré) ;
/// l'ordre suit l'ordre physique de l'archive.
pub fn for_each_member(
    tar_path: &Path,
    mut f: impl FnMut(String, Vec<u8>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut archive = open_archive(tar_path)?;
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.header().entry_type().is_dir() {
            continue;
        }
        let name = entry.path()?.to_string_lossy().into_owned();
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf)?;
        f(name, buf)?;
    }
    Ok(())
}

/// Streame les **noms** de tous les membres-fichiers sans lire leur contenu —
/// `f(nom)` un membre à la fois. Le `tar::Entries` avance seul au membre suivant
/// (on ne `read_to_end` pas) : O(noms), pas de copie des octets (la décompression
/// gzip reste, un `.tar.gz` n'étant pas seekable). Sert aux pré-passes qui
/// décident sur le chemin seul (winner publie/inedit DILA, #36).
pub fn for_each_member_name(
    tar_path: &Path,
    mut f: impl FnMut(&str) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut archive = open_archive(tar_path)?;
    for entry in archive.entries()? {
        let entry = entry?;
        if entry.header().entry_type().is_dir() {
            continue;
        }
        let name = entry.path()?.to_string_lossy().into_owned();
        f(&name)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;

    /// Construit un `.tar.gz` en mémoire avec 2 membres `.xml` (dont un en
    /// `.XML` majuscule) + 1 membre non-`.xml`, écrit dans un fichier temporaire.
    fn build_fixture() -> tempfile::NamedTempFile {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = File::create(tmp.path()).unwrap();
        let enc = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(enc);

        let add = |builder: &mut tar::Builder<GzEncoder<File>>, name: &str, data: &[u8]| {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, data).unwrap();
        };

        add(&mut builder, "decisions/a.xml", b"<doc>a</doc>");
        add(&mut builder, "decisions/B.XML", b"<doc>b</doc>");
        add(&mut builder, "decisions/presentation.pdf", b"%PDF-ignore");

        let enc = builder.into_inner().unwrap();
        enc.finish().unwrap();
        tmp
    }

    /// `for_each_member` streame TOUS les fichiers (xml comme pdf) dans l'ordre
    /// physique ; la classification (.xml/.dat/ignore) est au caller.
    #[test]
    fn streams_every_file_member_with_content() {
        let tmp = build_fixture();
        let mut members: Vec<(String, Vec<u8>)> = Vec::new();
        for_each_member(tmp.path(), |name, raw| {
            members.push((name, raw));
            Ok(())
        })
        .unwrap();
        members.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(members.len(), 3);
        assert_eq!(
            members[0],
            ("decisions/B.XML".into(), b"<doc>b</doc>".to_vec())
        );
        assert_eq!(
            members[1],
            ("decisions/a.xml".into(), b"<doc>a</doc>".to_vec())
        );
        assert_eq!(members[2].0, "decisions/presentation.pdf");
    }

    /// Une erreur remontée par le callback stoppe le streaming et propage.
    #[test]
    fn callback_error_propagates() {
        let tmp = build_fixture();
        let err = for_each_member(tmp.path(), |_, _| anyhow::bail!("stop")).unwrap_err();
        assert!(err.to_string().contains("stop"));
    }
}
