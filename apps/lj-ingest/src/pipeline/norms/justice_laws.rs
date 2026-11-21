//! Justice Laws: every federal Act and regulation in force, as its index names them.

use std::collections::HashMap;

use chrono::NaiveDate;
use lj_norm::text_aliases::TextAlias;
use lj_parse::{JusticeLawsInstrument, JusticeLawsKind};

use super::{Target, CANADA};

/// The publisher's folder for the files a run keeps.
pub(super) const CACHE: &str = "justice-laws";

/// A relist lists again the instruments not listed for this long, the oldest first.
pub(super) const RELIST_AFTER_DAYS: i64 = 28;

/// At most this many a relist: two listings each, 47 minutes at the FTP host's pace.
pub(super) const RELIST_AT_MOST: usize = 700;

/// Its collection in each language, its English folder, and the day each folder last changed.
pub(super) struct Instrument {
    pub french: &'static str,
    pub english: Option<(&'static str, String)>,
    pub changed: NaiveDate,
    pub english_changed: Option<NaiveDate>,
}

pub(super) type Index = HashMap<String, Instrument>;

/// The day each folder of each collection last changed.
pub(super) type Folders = HashMap<&'static str, HashMap<String, NaiveDate>>;

fn collections(kind: JusticeLawsKind) -> (&'static str, &'static str) {
    match kind {
        JusticeLawsKind::Act => ("lois", "acts"),
        JusticeLawsKind::Regulation => ("reglements", "regulations"),
    }
}

fn folder(unique_id: &str) -> String {
    unique_id.replace(' ', "_")
}

/// `CA/C-46`, `CA/DORS-2007-151`, `CA/CRC-ch-669`.
fn text_uid(unique_id: &str) -> String {
    let id = unique_id.replace("C.R.C.", "CRC");
    let parts: Vec<&str> = id
        .split([' ', ','])
        .map(|p| p.trim_end_matches('.'))
        .filter(|p| !p.is_empty())
        .collect();
    format!("CA/{}", parts.join("-"))
}

/// `DORS/2007-151`, `SOR/2007-151`, `C.R.C., ch. 669`.
fn instrument_number(unique_id: &str) -> String {
    match unique_id.split_once('-') {
        Some((kind @ ("DORS" | "TR" | "SOR" | "SI"), rest)) => format!("{kind}/{rest}"),
        _ => unique_id.to_string(),
    }
}

/// `L.R.C. 1985, ch. C-46`, `L.R.C. 1985, ch. 3 (2e suppl.)`, `L.C. 2001, ch. 27`, `DORS/2007-151`.
fn citation(i: &JusticeLawsInstrument) -> String {
    match (i.kind, i.fr.official_number.as_deref()) {
        (JusticeLawsKind::Regulation, _) => instrument_number(&i.fr.unique_id),
        (JusticeLawsKind::Act, None) => format!("L.R.C. 1985, ch. {}", i.fr.unique_id),
        (JusticeLawsKind::Act, Some(n)) if n == i.fr.unique_id => format!("L.R.C. 1985, ch. {n}"),
        (JusticeLawsKind::Act, Some(n)) if n.starts_with("1985, ch. ") && n.contains("suppl.") => {
            format!("L.R.C. {n}")
        }
        (JusticeLawsKind::Act, Some(n)) => format!("L.C. {n}"),
    }
}

fn title(i: &JusticeLawsInstrument) -> String {
    let name = i.fr.title.trim();
    let country = match name.to_lowercase().contains("canad") {
        true => "",
        false => " (Canada)",
    };
    format!("{name}{country}, {}", citation(i))
}

fn nature(i: &JusticeLawsInstrument) -> &'static str {
    let title = i.fr.title.to_lowercase();
    match i.kind {
        JusticeLawsKind::Act if title.starts_with("code ") => "CODE",
        JusticeLawsKind::Act => "loi",
        JusticeLawsKind::Regulation if title.starts_with("décret") => "decret",
        JusticeLawsKind::Regulation if title.starts_with("arrêté") => "arrete",
        JusticeLawsKind::Regulation => "reglement",
    }
}

fn aliases(i: &JusticeLawsInstrument) -> Vec<TextAlias> {
    let mut aliases = vec![
        TextAlias::number(&instrument_number(&i.fr.unique_id)),
        TextAlias::title("fr", i.fr.title.trim()),
    ];
    if let Some(en) = &i.en {
        if i.kind == JusticeLawsKind::Regulation {
            aliases.push(TextAlias::number(&instrument_number(&en.unique_id)));
        }
        aliases.push(TextAlias::title("en", en.title.trim()));
    }
    aliases
}

/// The selected texts whose French folder the FTP host holds, keyed by that folder.
pub(super) fn targets(
    index: Vec<JusticeLawsInstrument>,
    folders: &Folders,
    selected: &dyn Fn(&str) -> bool,
) -> (Vec<Target>, Index) {
    let changed = |collection: &str, name: &str| folders.get(collection)?.get(name).copied();
    let (mut targets, mut instruments, mut unlisted) = (Vec::new(), Index::new(), 0usize);
    for i in index {
        let (french, english) = collections(i.kind);
        let key = folder(&i.fr.unique_id);
        let Some(day) = changed(french, &key) else {
            unlisted += 1;
            continue;
        };
        if !selected(&key) {
            continue;
        }
        let english_folder =
            i.en.as_ref()
                .map(|en| folder(&en.unique_id))
                .and_then(|f| changed(english, &f).map(|day| (f, day)));
        let target = match CANADA.iter().find(|f| f.key == key) {
            Some(f) => Target {
                aliases: aliases(&i),
                ..Target::from(f)
            },
            None => Target {
                url: format!("https://laws-lois.justice.gc.ca/fra/{french}/{key}/"),
                text_uid: text_uid(&i.fr.unique_id),
                title: title(&i),
                title_key: None,
                nature: nature(&i).to_string(),
                current: None,
                justel: None,
                aliases: aliases(&i),
                key: key.clone(),
            },
        };
        instruments.insert(
            key,
            Instrument {
                french,
                english_changed: english_folder.as_ref().map(|(_, day)| *day),
                english: english_folder.map(|(f, _)| (english, f)),
                changed: day,
            },
        );
        targets.push(target);
    }
    tracing::info!(
        texts = targets.len(),
        english = instruments.values().filter(|i| i.english.is_some()).count(),
        unlisted,
        "justice laws index read"
    );
    (targets, instruments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_parse::JusticeLawsEntry;

    fn instrument(
        kind: JusticeLawsKind,
        id: &str,
        number: Option<&str>,
        title: &str,
    ) -> JusticeLawsInstrument {
        JusticeLawsInstrument {
            kind,
            fr: JusticeLawsEntry {
                unique_id: id.to_string(),
                official_number: number.map(str::to_string),
                title: title.to_string(),
            },
            en: None,
        }
    }

    #[test]
    fn a_federal_text_is_named_by_its_title_and_its_french_citation() {
        use JusticeLawsKind::{Act, Regulation};
        let cases = [
            (
                instrument(Act, "A-1", Some("A-1"), "Loi sur l’accès à l’information"),
                "Loi sur l’accès à l’information (Canada), L.R.C. 1985, ch. A-1",
                "CA/A-1",
            ),
            (
                instrument(
                    Act,
                    "D-3.4",
                    Some("1985, ch. 3 (2e suppl.)"),
                    "Loi sur le divorce",
                ),
                "Loi sur le divorce (Canada), L.R.C. 1985, ch. 3 (2e suppl.)",
                "CA/D-3.4",
            ),
            (
                instrument(
                    Act,
                    "A-0.6",
                    Some("2019, ch. 10"),
                    "Loi canadienne sur l’accessibilité",
                ),
                "Loi canadienne sur l’accessibilité, L.C. 2019, ch. 10",
                "CA/A-0.6",
            ),
            (
                instrument(Regulation, "DORS-2007-151", None, "Décret de remise"),
                "Décret de remise (Canada), DORS/2007-151",
                "CA/DORS-2007-151",
            ),
            (
                instrument(
                    Regulation,
                    "C.R.C., ch. 669",
                    None,
                    "Règlement sur les avances",
                ),
                "Règlement sur les avances (Canada), C.R.C., ch. 669",
                "CA/CRC-ch-669",
            ),
        ];
        for (i, expected_title, expected_uid) in cases {
            assert_eq!(title(&i), expected_title);
            assert_eq!(text_uid(&i.fr.unique_id), expected_uid);
        }
    }
}
