//! One Word file redacted, one process per file for a bench: the redacted file
//! goes to `--out`, and the outcome is printed as one JSON line. `expect`
//! prints the text of `--text` with each label in place. `robust-batch`
//! prints a `start`, `stage` and `done` line per file of `--list`.

mod robust;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{anyhow, bail, Result};
use lj_redact_core::labels::{Labels, Needle};
use lj_redact_core::office;
use serde::Serialize;

const USAGE: &str = "usage: redact --file F --format docx|odt|doc --out FILE [ENTRY ...]
       redact expect --text FILE ENTRY [ENTRY ...]
       redact robust-batch --list JSONL --fonts DIR
ENTRY: --needle N --kind manual|person|address|email|phone|identifier|docket|birth-date|birth-place [--label L]";

/// One `--needle` with the `--kind` and `--label` that follow it.
struct Entry {
    text: String,
    kind: Option<String>,
    label: Option<String>,
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1).peekable();
    let mode = args.next_if(|a| a == "robust-batch" || a == "expect");
    let (mut file, mut format, mut out, mut list, mut fonts, mut text) =
        (None, None, None, None, None, None);
    let mut entries: Vec<Entry> = Vec::new();
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| anyhow!("{flag} takes a value\n{USAGE}"))?;
        let last = entries.last_mut();
        match (flag.as_str(), last) {
            ("--file", _) => file = Some(PathBuf::from(value)),
            ("--format", _) => format = Some(value),
            ("--out", _) => out = Some(PathBuf::from(value)),
            ("--list", _) => list = Some(PathBuf::from(value)),
            ("--fonts", _) => fonts = Some(PathBuf::from(value)),
            ("--text", _) => text = Some(PathBuf::from(value)),
            ("--needle", _) => entries.push(Entry {
                text: value,
                kind: None,
                label: None,
            }),
            ("--kind", Some(entry)) if entry.kind.is_none() => entry.kind = Some(value),
            ("--label", Some(entry)) if entry.label.is_none() => entry.label = Some(value),
            _ => bail!("unexpected argument {flag}\n{USAGE}"),
        }
    }
    let needles = entries
        .iter()
        .map(|e| {
            let kind = e
                .kind
                .as_deref()
                .ok_or_else(|| anyhow!("--needle {} has no --kind\n{USAGE}", e.text))?;
            Needle::filed(&e.text, kind, e.label.as_deref(), false)
        })
        .collect::<Result<Vec<_>>>()?;
    match mode.as_deref() {
        Some("robust-batch") => {
            let (Some(list), Some(fonts), None, None, None, None, true) =
                (list, fonts, file, format, out, text, needles.is_empty())
            else {
                bail!("robust-batch takes --list and --fonts only\n{USAGE}");
            };
            robust::run_batch(&list, &fonts)
        }
        Some(_) => {
            let (Some(text), None, None, None, None, None, false) =
                (text, file, format, out, list, fonts, needles.is_empty())
            else {
                bail!("expect takes --text and at least one entry\n{USAGE}");
            };
            expect(&text, &needles)
        }
        None => {
            let (Some(file), Some(format), Some(out), None, None, None) =
                (file, format, out, list, fonts, text)
            else {
                bail!("--file, --format and --out are required\n{USAGE}");
            };
            if !matches!(format.as_str(), "docx" | "odt" | "doc") {
                bail!("--format {format}: docx, odt or doc\n{USAGE}");
            }
            redact(&file, &out, &needles)
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum Status {
    Ok,
    Failed,
}

#[derive(Serialize)]
struct Redaction {
    found: usize,
    removed: usize,
    self_check_leaks: Vec<String>,
}

#[derive(Serialize)]
struct Redacted {
    status: Status,
    detail: String,
    seconds: f64,
    #[serde(flatten)]
    redaction: Option<Redaction>,
}

fn redact(file: &Path, out: &Path, needles: &[Needle]) -> Result<()> {
    let started = Instant::now();
    let bytes = std::fs::read(file)?;
    let (status, detail, redaction) = match office::redact(&bytes, needles) {
        Ok(o) => {
            std::fs::write(out, &o.bytes)?;
            let redaction = Redaction {
                found: o.found,
                removed: o.removed,
                self_check_leaks: o.leaks,
            };
            (Status::Ok, String::new(), Some(redaction))
        }
        Err(e) => (Status::Failed, format!("{e:#}"), None),
    };
    let record = Redacted {
        status,
        detail,
        seconds: started.elapsed().as_secs_f64(),
        redaction,
    };
    println!("{}", serde_json::to_string(&record)?);
    Ok(())
}

/// The text of `file` with each occurrence of `needles` under its label, as
/// the redaction of a document chooses them.
fn expect(file: &Path, needles: &[Needle]) -> Result<()> {
    let source = std::fs::read_to_string(file)?;
    let text: Vec<char> = source.chars().collect();
    let feed = office::plain(&source)?;
    let texts: Vec<String> = needles.iter().map(|n| n.text.to_owned()).collect();
    let (_, replacements) = office::replacements(&[feed], &[], &texts, &Labels::assign(needles))?;
    let mut removed = vec![false; text.len()];
    let mut inserted: BTreeMap<usize, String> = BTreeMap::new();
    for replacement in replacements {
        for (k, anchor) in replacement.anchors.iter().enumerate() {
            let lj_office::Anchor::XmlText { offset, .. } = *anchor else {
                bail!("a character of the text anchored as {anchor:?}");
            };
            if k == 0 {
                inserted
                    .entry(offset as usize)
                    .or_default()
                    .push_str(&replacement.text);
            }
            removed[offset as usize] = true;
        }
    }
    let mut out = String::with_capacity(text.len());
    for (at, c) in text.iter().enumerate() {
        if let Some(with) = inserted.get(&at) {
            out.push_str(with);
        }
        if !removed[at] {
            out.push(*c);
        }
    }
    print!("{out}");
    Ok(())
}
