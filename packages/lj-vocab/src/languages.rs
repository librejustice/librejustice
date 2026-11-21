//! Language codes, stored in ISO 639-1.

/// The Union's twenty-four official languages, in ISO 639-1.
pub const EU_OFFICIAL_LANGUAGES: [&str; 24] = [
    "bg", "cs", "da", "de", "el", "en", "et", "fi", "fr", "ga", "hr", "hu", "it", "lv", "lt", "mt",
    "nl", "pl", "pt", "ro", "sk", "sl", "es", "sv",
];

/// The ISO 639-1 code of one of the Union's official languages, named by its
/// Publications Office code (`DEU`, `ENG`, `FRA`…).
pub fn iso_639_1_of_eu(code: &str) -> Option<&'static str> {
    Some(match code {
        "BUL" => "bg",
        "CES" => "cs",
        "DAN" => "da",
        "DEU" => "de",
        "ELL" => "el",
        "ENG" => "en",
        "EST" => "et",
        "FIN" => "fi",
        "FRA" => "fr",
        "GLE" => "ga",
        "HRV" => "hr",
        "HUN" => "hu",
        "ITA" => "it",
        "LAV" => "lv",
        "LIT" => "lt",
        "MLT" => "mt",
        "NLD" => "nl",
        "POL" => "pl",
        "POR" => "pt",
        "RON" => "ro",
        "SLK" => "sk",
        "SLV" => "sl",
        "SPA" => "es",
        "SWE" => "sv",
        _ => return None,
    })
}

/// The Publications Office code of an official language, lowercased as CELLAR
/// takes it in `Accept-Language`, from its ISO 639-1 code.
pub fn eu_of_iso_639_1(code: &str) -> Option<&'static str> {
    Some(match code {
        "bg" => "bul",
        "cs" => "ces",
        "da" => "dan",
        "de" => "deu",
        "el" => "ell",
        "en" => "eng",
        "et" => "est",
        "fi" => "fin",
        "fr" => "fra",
        "ga" => "gle",
        "hr" => "hrv",
        "hu" => "hun",
        "it" => "ita",
        "lv" => "lav",
        "lt" => "lit",
        "mt" => "mlt",
        "nl" => "nld",
        "pl" => "pol",
        "pt" => "por",
        "ro" => "ron",
        "sk" => "slk",
        "sl" => "slv",
        "es" => "spa",
        "sv" => "swe",
        _ => return None,
    })
}

/// The ISO 639-1 code of a stored text's language: an official language of the Union, or Arabic.
pub fn iso_639_1_of_text(code: &str) -> Option<&'static str> {
    iso_639_1_of_eu(code).or((code == "ARA").then_some("ar"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_twenty_four_official_languages_map_to_distinct_codes() {
        let codes = [
            "BUL", "CES", "DAN", "DEU", "ELL", "ENG", "EST", "FIN", "FRA", "GLE", "HRV", "HUN",
            "ITA", "LAV", "LIT", "MLT", "NLD", "POL", "POR", "RON", "SLK", "SLV", "SPA", "SWE",
        ];
        let mapped: std::collections::BTreeSet<_> =
            codes.iter().map(|c| iso_639_1_of_eu(c).unwrap()).collect();
        assert_eq!(mapped.len(), 24);
        assert_eq!(iso_639_1_of_eu("ELL"), Some("el"));
        assert_eq!(iso_639_1_of_eu("fra"), None);
        for c in codes {
            let iso = iso_639_1_of_eu(c).unwrap();
            assert_eq!(eu_of_iso_639_1(iso).unwrap(), c.to_lowercase());
            assert!(EU_OFFICIAL_LANGUAGES.contains(&iso));
        }
    }
}
