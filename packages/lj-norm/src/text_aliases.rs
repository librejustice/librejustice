//! The names a text is cited by besides its title, written by hand in `data/text_aliases.tsv`:
//! its abbreviations and short titles in each language, its number in its collection. The
//! writer of a text writes the rows of its uid together with the names it reads from its source.

use std::collections::HashMap;
use std::sync::LazyLock;

const TEXT_ALIASES_TSV: &str = include_str!("../data/text_aliases.tsv");

/// A name a text is cited by besides its title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextAlias {
    pub kind: TextAliasKind,
    /// ISO 639-1, empty for a number.
    pub lang: String,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextAliasKind {
    /// `ZGB`, `CC`, `oud BW`.
    Abbreviation,
    /// Its number in its collection: `210` in the RS, `173.36` in Vaud's RSV.
    Number,
    /// A short title in one language: `Zivilgesetzbuch`.
    Title,
}

impl TextAliasKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TextAliasKind::Abbreviation => "abbreviation",
            TextAliasKind::Number => "number",
            TextAliasKind::Title => "title",
        }
    }
}

impl TextAlias {
    pub fn abbreviation(lang: &str, value: &str) -> Self {
        Self {
            kind: TextAliasKind::Abbreviation,
            lang: lang.to_string(),
            value: value.to_string(),
        }
    }

    pub fn number(value: &str) -> Self {
        Self {
            kind: TextAliasKind::Number,
            lang: String::new(),
            value: value.to_string(),
        }
    }

    pub fn title(lang: &str, value: &str) -> Self {
        Self {
            kind: TextAliasKind::Title,
            lang: lang.to_string(),
            value: value.to_string(),
        }
    }
}

/// Panics on a malformed row: the table is embedded, and its test reads every row.
static TABLE: LazyLock<HashMap<&'static str, Vec<TextAlias>>> = LazyLock::new(|| {
    let mut table: HashMap<&'static str, Vec<TextAlias>> = HashMap::new();
    for line in TEXT_ALIASES_TSV.lines().skip(1).filter(|l| !l.is_empty()) {
        let [uid, kind, lang, value] = line.split('\t').collect::<Vec<_>>()[..] else {
            panic!("text_aliases.tsv: four columns expected: {line:?}");
        };
        let alias = match kind {
            "abbreviation" => TextAlias::abbreviation(lang, value),
            "title" => TextAlias::title(lang, value),
            "number" if lang.is_empty() => TextAlias::number(value),
            _ => panic!("text_aliases.tsv: unknown kind or a number with a language: {line:?}"),
        };
        table.entry(uid).or_default().push(alias);
    }
    table
});

/// The hand-written names of `text_uid`, none when the table has no row for it.
pub fn of(text_uid: &str) -> Vec<TextAlias> {
    TABLE.get(text_uid).cloned().unwrap_or_default()
}

/// The names a title gives in its closing brackets: `Steuergesetz (StG)` is cited `StG`,
/// `Gesetz über den Schutz der Kulturgüter (Kantonales Kulturgüterschutzgesetz, kKGSG)` by a
/// short title and an abbreviation.
pub fn in_title(lang: &str, title: &str) -> Vec<TextAlias> {
    let Some(inner) = title.trim_end().strip_suffix(')') else {
        return Vec::new();
    };
    let mut depth = 0usize;
    let open = inner.char_indices().rev().find_map(|(i, c)| match c {
        ')' => {
            depth += 1;
            None
        }
        '(' if depth == 0 => Some(i),
        '(' => {
            depth -= 1;
            None
        }
        _ => None,
    });
    open.map_or_else(Vec::new, |open| in_list(lang, &inner[open + 1..]))
}

/// The names of a list such as `Baugesetz, BauG`. A part holding a digit names nothing; a part is
/// an abbreviation when each of its words carries two capitals and no run of six small letters,
/// a short title when it opens on a capital.
pub fn in_list(lang: &str, list: &str) -> Vec<TextAlias> {
    let short = |word: &str| {
        word.chars().filter(|c| c.is_uppercase()).count() >= 2
            && word
                .split(|c: char| !c.is_lowercase())
                .all(|run| run.chars().count() < 6)
    };
    list.split([',', ';'])
        .filter_map(|part| {
            let words: Vec<&str> = part.split_whitespace().collect();
            let part = words.join(" ");
            if words.is_empty() || part.contains(|c: char| c.is_ascii_digit()) {
                None
            } else if words.iter().all(|w| short(w)) {
                Some(TextAlias::abbreviation(lang, &part))
            } else if part.starts_with(char::is_uppercase) {
                Some(TextAlias::title(lang, &part))
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_reads_as_the_base_accepts_it() {
        let mut seen = std::collections::HashSet::new();
        for (uid, aliases) in TABLE.iter() {
            for a in aliases {
                assert!(!a.value.is_empty(), "{uid}: empty value");
                let two_letters =
                    a.lang.len() == 2 && a.lang.bytes().all(|b| b.is_ascii_lowercase());
                assert_eq!(
                    a.kind == TextAliasKind::Number,
                    a.lang.is_empty(),
                    "{uid}: a number has no language, a name has one"
                );
                assert!(
                    a.lang.is_empty() || two_letters,
                    "{uid}: language {:?}",
                    a.lang
                );
                assert!(
                    seen.insert((*uid, a.kind, &a.lang, &a.value)),
                    "{uid}: duplicate {a:?}"
                );
            }
        }
        assert!(of("CH/CODE-CIVIL").contains(&TextAlias::abbreviation("de", "ZGB")));
    }

    #[test]
    fn a_title_names_its_abbreviation_and_short_title_in_its_closing_brackets() {
        let names = |title| in_title("de", title);
        assert_eq!(
            names("Steuergesetz (StG)"),
            [TextAlias::abbreviation("de", "StG")]
        );
        assert_eq!(
            names("Gesetz über den Schutz der Kulturgüter (Kantonales Kulturgüterschutzgesetz, kKGSG)"),
            [
                TextAlias::title("de", "Kantonales Kulturgüterschutzgesetz"),
                TextAlias::abbreviation("de", "kKGSG"),
            ]
        );
        assert_eq!(
            in_title("fr", "Règlement sur la conservation des documents (Règlement sur la conservation, RC JCP)"),
            [
                TextAlias::title("fr", "Règlement sur la conservation"),
                TextAlias::abbreviation("fr", "RC JCP"),
            ]
        );
        assert_eq!(
            in_title("fr", "Loi fédérale du 14 mars 1958 sur la responsabilité (Loi  sur la responsabilité, LRCF)"),
            [
                TextAlias::title("fr", "Loi sur la responsabilité"),
                TextAlias::abbreviation("fr", "LRCF"),
            ]
        );
        assert_eq!(
            in_title(
                "fr",
                "Loi du 28.10.2008 sur la procédure administrative (LPA-VD)"
            ),
            [TextAlias::abbreviation("fr", "LPA-VD")]
        );
        assert_eq!(
            names("Reglement für das Museum Luzern (Museumsreglement)"),
            [TextAlias::title("de", "Museumsreglement")]
        );
        assert!(names("Verordnung über die Anhörung der Gemeinden").is_empty());
        assert!(names("Arrêté (du 12 mars 2000)").is_empty());
        assert!(names("Kantonsratsbeschluss (Teilrevision 2019)").is_empty());
        assert_eq!(
            names("Verordnung über das Informatiksicherheitsmanagement (IMS-Verordnung)"),
            [TextAlias::title("de", "IMS-Verordnung")]
        );
        assert_eq!(
            names("Verordnung über die elektronische Steuererklärung (ETax-Verordnung; ETaxV)"),
            [
                TextAlias::title("de", "ETax-Verordnung"),
                TextAlias::abbreviation("de", "ETaxV"),
            ]
        );
    }
}
