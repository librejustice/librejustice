//! Helpers I/O fichiers : listing récursif par extension, lecture `.jsonl.gz`.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::Result;

/// Liste les `*.tar.gz` sous `path` (dossier ; non récursif — les tarballs DILA
/// vivent à plat dans `tarballs/`). Dossier absent ⇒ liste vide.
pub(super) fn collect_tar_gz(path: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !path.is_dir() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_file() && p.to_str().is_some_and(|s| s.ends_with(".tar.gz")) {
            out.push(p);
        }
    }
    Ok(out)
}

/// Liste les `*.zip` sous `path` (récursif si dossier, sinon le fichier seul).
pub(super) fn collect_zip_paths(path: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if path.is_file() {
        if path.extension().and_then(|e| e.to_str()) == Some("zip") {
            out.push(path.to_path_buf());
        }
        return Ok(out);
    }
    if path.is_dir() {
        collect_with_ext(path, "zip", &mut out)?;
        out.sort();
    }
    Ok(out)
}

/// Liste les `*.jsonl.gz` sous `path` (récursif).
pub(super) fn collect_jsonl_gz(path: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if path.is_file() {
        if path.to_str().is_some_and(|s| s.ends_with(".jsonl.gz")) {
            out.push(path.to_path_buf());
        }
        return Ok(out);
    }
    if path.is_dir() {
        collect_jsonl_gz_recursive(path, &mut out)?;
        out.sort();
    }
    Ok(out)
}

fn collect_with_ext(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_dir() {
            collect_with_ext(&p, ext, out)?;
        } else if p.extension().and_then(|e| e.to_str()) == Some(ext) {
            out.push(p);
        }
    }
    Ok(())
}

fn collect_jsonl_gz_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_dir() {
            collect_jsonl_gz_recursive(&p, out)?;
        } else if p.to_str().is_some_and(|s| s.ends_with(".jsonl.gz")) {
            out.push(p);
        }
    }
    Ok(())
}

/// Lit toutes les lignes (sans `\n`) d'un `.jsonl.gz`.
pub(super) fn read_jsonl_gz_lines(path: &Path) -> Result<Vec<Vec<u8>>> {
    let file = std::fs::File::open(path)?;
    let mut decoder = flate2::read::MultiGzDecoder::new(file);
    let mut content = Vec::new();
    decoder.read_to_end(&mut content)?;
    Ok(split_jsonl_lines(&content))
}

/// Découpe un contenu JSONL en lignes trimées, **sans la pseudo-ligne vide**
/// induite par le `\n` final. Compter les lignes comme Python (`enumerate(fh)`)
/// est requis pour que `ingested_lines` reste un offset de resume stable sur
/// un fichier append-only.
fn split_jsonl_lines(content: &[u8]) -> Vec<Vec<u8>> {
    let content = content.strip_suffix(b"\n").unwrap_or(content);
    if content.is_empty() {
        return Vec::new();
    }
    content
        .split(|&b| b == b'\n')
        .map(|line| {
            // strip() Python : trim whitespace ASCII des deux côtés.
            let start = line.iter().position(|b| !b.is_ascii_whitespace());
            match start {
                None => Vec::new(),
                Some(s) => {
                    let end = line.iter().rposition(|b| !b.is_ascii_whitespace()).unwrap();
                    line[s..=end].to_vec()
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec resume incrémental Judilibre : le compte de lignes doit être stable
    // sur un fichier append-only (pas de pseudo-ligne vide après le \n final),
    // sinon l'offset `ingested_lines` saute une vraie ligne au run suivant.
    #[test]
    fn split_jsonl_lines_count_is_append_stable() {
        assert_eq!(split_jsonl_lines(b"").len(), 0);
        assert_eq!(split_jsonl_lines(b"a\nb\n").len(), 2);
        assert_eq!(split_jsonl_lines(b"a\nb").len(), 2);
        // Append d'une ligne : les 2 premières inchangées, la 3e est la nouvelle.
        let after_append = split_jsonl_lines(b"a\nb\nc\n");
        assert_eq!(after_append.len(), 3);
        assert_eq!(after_append[2], b"c");
        // Ligne blanche intermédiaire : comptée (comme enumerate(fh) Python).
        assert_eq!(split_jsonl_lines(b"a\n  \nb\n").len(), 3);
        assert_eq!(split_jsonl_lines(b"a\n  \nb\n")[1], b"");
    }
}
