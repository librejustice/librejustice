//! The front draws every menu and calendar itself: a `<select>`, a `<datalist>`
//! or a date `<input>` opens the browser's own picker, outside the house style.

use std::path::{Path, PathBuf};

const NATIVE_MENUS: [&str; 2] = ["select", "datalist"];
const NATIVE_CALENDARS: [&str; 5] = ["date", "datetime-local", "month", "week", "time"];

fn sources(dir: &Path, into: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read a source directory") {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            sources(&path, into);
        } else if path.extension().is_some_and(|e| e == "rs") {
            into.push(path);
        }
    }
}

/// Native menus and calendars, comment lines skipped. A tag name must end the
/// tag: `<selection` is not one.
fn native_pickers(source: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (no, line) in source.lines().enumerate() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        for tag in NATIVE_MENUS {
            let open = format!("<{tag}");
            let mut rest = line;
            while let Some(pos) = rest.find(&open) {
                rest = &rest[pos + open.len()..];
                if rest
                    .chars()
                    .next()
                    .is_none_or(|c| c.is_ascii_whitespace() || c == '>' || c == '/')
                {
                    found.push((no + 1, format!("<{tag}>")));
                }
            }
        }
        for kind in NATIVE_CALENDARS {
            let attr = format!("type=\"{kind}\"");
            if line.contains(&attr) {
                found.push((no + 1, attr));
            }
        }
    }
    found
}

#[test]
fn no_native_menu_nor_calendar() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&root, &mut files);

    let mut views = 0usize;
    let mut offenders = Vec::new();
    for file in &files {
        let source = std::fs::read_to_string(file).expect("read a source");
        if source.contains("view!") {
            views += 1;
        }
        for (line, what) in native_pickers(&source) {
            offenders.push(format!(
                "{}:{line} {what}",
                file.strip_prefix(&root).unwrap().display()
            ));
        }
    }

    assert!(
        views >= 50,
        "{views} files with a view! among {}: the scan no longer reads the front",
        files.len()
    );
    assert!(
        offenders.is_empty(),
        "native pickers, to be drawn with `DropdownSelect`, `GroupedSelect` or \
         `DatePicker`: {offenders:?}"
    );
}
