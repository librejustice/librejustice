//! Helpers I/O fichiers : listing récursif par extension.

use std::path::{Path, PathBuf};

use anyhow::Result;

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
