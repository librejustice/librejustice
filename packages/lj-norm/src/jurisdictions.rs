//! Which corpora a legal-norm query searches. Foreign codes, francophone or
//! translated, match a French query word for word, and the corpus is meant to
//! grow to the law of the world: the jurisdiction is a **filter**, never a
//! ranking prior. This country/demonym → ISO code table is that of
//! `legal_text.jurisdiction`. Pure, no I/O.

use lj_fold::fold;

/// The law applicable in France, which is where a reader is read from when they
/// have named no place.
pub const DOMESTIC_REGIONS: [&str; 3] = ["FR", "EU", "INTL"];

/// `code ISO` → variantes foldées (minuscules, sans accent). Convention :
/// suffixe `*` = préfixe de token (couvre les flexions du gentilé :
/// `senegal*` matche « sénégalais(e)(s) ») ; entrée à espace = sous-chaîne
/// bornée par mots ; sinon token exact (les formes courtes ambiguës — « mali »
/// ne doit pas matcher « malice »). Seuls les pays présents en corpus.
const COUNTRIES: &[(&str, &[&str])] = &[
    ("AM", &["armenie*", "armenien*"]),
    ("AO", &["angol*"]),
    ("AT", &["autriche*", "autrichien*"]),
    ("BE", &["belge*", "belgique"]),
    ("BF", &["burkina*"]),
    ("BG", &["bulgar*"]),
    ("BI", &["burundi", "burundais*"]),
    ("BJ", &["benin", "beninois*"]),
    ("CD", &["congo", "congolais*", "kinshasa", "rdc", "zaire*"]),
    ("CF", &["centrafri*"]),
    ("CG", &["congo", "congolais*", "brazzaville"]),
    ("CH", &["suisse*", "helvetique*"]),
    ("CI", &["ivoir*"]),
    ("CM", &["cameroun*"]),
    ("DE", &["allemagne", "allemand*"]),
    ("DJ", &["djibout*"]),
    ("DO", &["dominicain*"]),
    ("DZ", &["algerie*", "algerien*"]),
    ("EG", &["egypt*"]),
    ("ES", &["espagne*", "espagnol*"]),
    ("GA", &["gabon*"]),
    ("GN", &["guine*"]),
    ("GR", &["grec*"]),
    ("HT", &["haiti*", "haitien*"]),
    ("HU", &["hongrie*", "hongrois*"]),
    ("IQ", &["irak*", "iraq*"]),
    ("IT", &["italie*", "italien*"]),
    ("JO", &["jordanie*", "jordanien*"]),
    ("KM", &["comor*"]),
    ("LB", &["liban*"]),
    ("LU", &["luxembourg*"]),
    ("MA", &["maroc*"]),
    ("MC", &["monaco", "monegasque*"]),
    ("MG", &["madagascar", "malgache*"]),
    ("ML", &["mali", "malien*"]),
    ("MR", &["mauritanie*", "mauritanien*"]),
    ("MU", &["maurice", "mauricien*"]),
    ("NC", &["caledonie*", "caledonien*"]),
    ("NE", &["niger", "nigerien*"]),
    ("NG", &["nigeria*", "nigerian*"]),
    ("NL", &["neerlandais*", "pays bas"]),
    ("PE", &["perou", "peruvien*"]),
    ("PF", &["polynesie*", "polynesien*", "tahiti*"]),
    ("PL", &["pologne*", "polonais*"]),
    ("PT", &["portug*"]),
    ("RO", &["roumanie*", "roumain*"]),
    ("RS", &["serbie*", "serbe*"]),
    ("RU", &["russie*", "russe*"]),
    ("RW", &["rwand*"]),
    ("SN", &["senegal*"]),
    ("ST", &["sao tome", "santomeen*"]),
    ("SY", &["syrie*", "syrien*"]),
    ("TD", &["tchad*"]),
    ("TG", &["togo", "togolais*"]),
    ("TN", &["tunisie*", "tunisien*", "tunis"]),
    ("TR", &["turquie*", "turc", "turcs", "turque*"]),
    ("UA", &["ukraine*", "ukrainien*"]),
    ("VE", &["venezuel*"]),
    ("VN", &["vietnam*", "viet", "vietnamien*"]),
];

/// Codes ISO des juridictions étrangères que `query` nomme (pays ou gentilé),
/// dans l'ordre de la table. « congo(lais) » est ambigu → `CG` et `CD`.
/// Vide = requête domestique (le prior FR/UE/INTL s'applique seul).
pub fn query_jurisdictions(query: &str) -> Vec<&'static str> {
    strip_query_jurisdictions(query)
        .map(|(codes, _)| codes)
        .unwrap_or_default()
}

/// Folded stems of the instruments an international agreement carries in its
/// title, `*` suffixed as in [`COUNTRIES`]. Over-firing only widens the scope by
/// INTL, so the ambiguous ones stay in: a missing stem loses a treaty.
const INSTRUMENTS: &[&str] = &[
    "accord*",
    "amendement*",
    "arrangement*",
    "avenant*",
    "charte*",
    "convention*",
    "echange*",
    "pacte*",
    "protocole*",
    "traite",
    "traites",
];

/// `traite` is matched whole: its prefix is also that of `traitement`.
fn names_instrument(query: &str) -> bool {
    fold(query)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .any(|t| {
            INSTRUMENTS.iter().any(|s| match s.strip_suffix('*') {
                Some(prefix) => t.starts_with(prefix),
                None => t == *s,
            })
        })
}

/// What a query searches: the corpora, and what is left of it once a country
/// token that only named a corpus is taken out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryScope {
    /// ISO codes of `legal_text.jurisdiction`, never empty. Owned rather than
    /// `&'static`: without a country named, the scope is the reader's home,
    /// which is read at runtime.
    pub jurisdictions: Vec<String>,
    /// The query with the country tokens taken out, `None` when it is searched
    /// as written. Identity resolution reads it, ranking does not: « Code civil
    /// (Belgique) » is named by « code civil », not by « code civil belge »,
    /// where dropping the token from a search wrecks the ranking of « code civil du
    /// Sénégal », whose every formed title carries it.
    pub stripped: Option<String>,
}

/// Scope `query` resolves to. No country named, the law applicable where the
/// reader is, `home`; one named, that country alone, a search across several
/// being unrankable against itself. An instrument word joins INTL, where a
/// bilateral treaty lives rather than in either signatory's corpus.
pub fn query_scope(query: &str, home: &[&str]) -> QueryScope {
    let Some((codes, stripped)) = strip_query_jurisdictions(query) else {
        return QueryScope {
            jurisdictions: home.iter().map(|s| s.to_string()).collect(),
            stripped: None,
        };
    };
    match names_instrument(query) {
        true => QueryScope {
            jurisdictions: codes
                .into_iter()
                .chain(["INTL"])
                .map(str::to_string)
                .collect(),
            stripped: None,
        },
        false => QueryScope {
            jurisdictions: codes.into_iter().map(str::to_string).collect(),
            stripped: Some(stripped),
        },
    }
}

/// Comme [`query_jurisdictions`], mais renvoie aussi la requête **débarrassée
/// des tokens pays/gentilé** (tokens d'origine, casse/accents préservés) —
/// l'entrée de la jambe « pays nommé » du ranking : dans « conditions du divorce
/// au sénégal », le token « sénégal » désigne le corpus, pas le contenu — les
/// articles de fond du divorce sénégalais ne le contiennent pas. `None` si aucun
/// pays nommé ; si rien ne subsiste après retrait (requête réduite au pays), la
/// requête d'origine est renvoyée telle quelle.
pub fn strip_query_jurisdictions(query: &str) -> Option<(Vec<&'static str>, String)> {
    let folded = fold(query);
    if folded.is_empty() {
        return None;
    }
    // Tokens d'origine alignés sur leur forme foldée (mêmes séparateurs).
    let orig_tokens: Vec<&str> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    let tokens: Vec<String> = orig_tokens.iter().map(|t| fold(t)).collect();
    // Un gentilé en composé bilatéral « franco-algérien » nomme un TRAITÉ
    // (accord franco-algérien, convention franco-sénégalaise — corpus INTL),
    // pas le droit interne du pays : le token précédé de « franco » ne
    // compte pas comme désignation de juridiction.
    let bilateral = |i: usize, tokens: &[String]| i > 0 && tokens[i - 1] == "franco";
    let mut matched = vec![false; tokens.len()];
    let mut codes: Vec<&'static str> = Vec::new();
    for (code, stems) in COUNTRIES {
        let mut hit = false;
        for s in *stems {
            match s.strip_suffix('*') {
                Some(prefix) => {
                    for i in 0..tokens.len() {
                        if tokens[i].starts_with(prefix) && !bilateral(i, &tokens) {
                            matched[i] = true;
                            hit = true;
                        }
                    }
                }
                None if s.contains(' ') => {
                    let parts: Vec<&str> = s.split(' ').collect();
                    for i in 0..tokens.len().saturating_sub(parts.len() - 1) {
                        if (0..parts.len()).all(|j| tokens[i + j] == parts[j]) {
                            for slot in matched.iter_mut().skip(i).take(parts.len()) {
                                *slot = true;
                            }
                            hit = true;
                        }
                    }
                }
                None => {
                    for i in 0..tokens.len() {
                        if tokens[i] == *s && !bilateral(i, &tokens) {
                            matched[i] = true;
                            hit = true;
                        }
                    }
                }
            }
        }
        if hit {
            codes.push(code);
        }
    }
    if codes.is_empty() {
        return None;
    }
    let stripped: Vec<&str> = orig_tokens
        .iter()
        .zip(&matched)
        .filter(|(_, m)| !**m)
        .map(|(t, _)| *t)
        .collect();
    let stripped = if stripped.is_empty() {
        query.to_string()
    } else {
        stripped.join(" ")
    };
    Some((codes, stripped))
}

/// The jurisdiction a first path segment names, `None` when it names none,
/// which is what tells the shape carrying one from the shape it replaces: no
/// text slug is two letters, nor `intl`.
pub fn from_url_segment(segment: &str) -> Option<&'static str> {
    DOMESTIC_REGIONS
        .iter()
        .copied()
        .chain(COUNTRIES.iter().map(|(code, _)| *code))
        .find(|code| code.eq_ignore_ascii_case(segment))
}

#[cfg(test)]
mod tests {
    use super::query_jurisdictions;

    #[test]
    fn demonym_and_country_name_detected() {
        assert_eq!(
            query_jurisdictions("code de la famille sénégalais"),
            vec!["SN"]
        );
        assert_eq!(query_jurisdictions("code civil du sénégal"), vec!["SN"]);
        assert_eq!(
            query_jurisdictions("conditions du divorce au Sénégal"),
            vec!["SN"]
        );
        assert_eq!(query_jurisdictions("droit du travail belge"), vec!["BE"]);
    }

    #[test]
    fn domestic_query_detects_nothing() {
        assert!(query_jurisdictions("responsabilité du fait des choses").is_empty());
        assert!(query_jurisdictions("article L. 442-1 du code de commerce").is_empty());
        assert!(query_jurisdictions("").is_empty());
    }

    #[test]
    fn short_exact_forms_do_not_prefix_match() {
        assert_eq!(query_jurisdictions("code minier du mali"), vec!["ML"]);
        assert!(query_jurisdictions("malice et intention de nuire").is_empty());
        assert_eq!(query_jurisdictions("constitution du niger"), vec!["NE"]);
        assert_eq!(
            query_jurisdictions("droit des sociétés au nigeria"),
            vec!["NG"]
        );
    }

    #[test]
    fn congo_is_ambiguous() {
        assert_eq!(
            query_jurisdictions("code de la famille congolais"),
            vec!["CD", "CG"]
        );
    }

    #[test]
    fn strip_removes_country_tokens() {
        use super::strip_query_jurisdictions;
        assert_eq!(
            strip_query_jurisdictions("conditions du divorce au sénégal"),
            Some((vec!["SN"], "conditions du divorce au".to_string()))
        );
        assert_eq!(
            strip_query_jurisdictions("droit du travail belge"),
            Some((vec!["BE"], "droit du travail".to_string()))
        );
        assert_eq!(
            strip_query_jurisdictions("successions aux pays bas"),
            Some((vec!["NL"], "successions aux".to_string()))
        );
        assert_eq!(
            strip_query_jurisdictions("sénégal"),
            Some((vec!["SN"], "sénégal".to_string()))
        );
        assert_eq!(
            strip_query_jurisdictions("trouble anormal de voisinage"),
            None
        );
    }

    #[test]
    fn bilateral_compound_is_not_a_country() {
        use super::strip_query_jurisdictions;
        assert_eq!(
            strip_query_jurisdictions("accord franco-algérien certificat de résidence"),
            None
        );
        assert_eq!(
            strip_query_jurisdictions("convention franco-sénégalaise d'exequatur"),
            None
        );
        assert_eq!(
            strip_query_jurisdictions("accord franco-algérien et droit algérien"),
            Some((vec!["DZ"], "accord franco algérien et droit".to_string()))
        );
    }

    /// The default reader, whose scope is the one every caller had before the
    /// place became a setting.
    fn home() -> Vec<&'static str> {
        super::DOMESTIC_REGIONS.to_vec()
    }

    /// A reader placed abroad searches their own law when the query names no
    /// country, and the named country still wins over the home.
    #[test]
    fn a_reader_at_home_abroad_searches_their_own_law() {
        use super::query_scope;
        let bern = ["CH", "INTL"];
        let scope = query_scope("responsabilité du fait des choses", &bern);
        assert_eq!(scope.jurisdictions, bern.to_vec());
        let named = query_scope("conditions du divorce au sénégal", &bern);
        assert_eq!(named.jurisdictions, vec!["SN"]);
    }

    #[test]
    fn scope_without_a_country_is_the_law_applicable_in_france() {
        use super::{query_scope, DOMESTIC_REGIONS};
        let scope = query_scope("responsabilité du fait des choses", &home());
        assert_eq!(scope.jurisdictions, DOMESTIC_REGIONS.to_vec());
        assert_eq!(scope.stripped, None);
    }

    #[test]
    fn a_named_country_scopes_and_offers_the_query_without_it() {
        use super::query_scope;
        let scope = query_scope("conditions du divorce au sénégal", &home());
        assert_eq!(scope.jurisdictions, vec!["SN"]);
        assert_eq!(scope.stripped.as_deref(), Some("conditions du divorce au"));
    }

    #[test]
    fn an_instrument_keeps_the_country_and_joins_intl() {
        use super::query_scope;
        let scope = query_scope("convention fiscale France Sénégal", &home());
        assert_eq!(scope.jurisdictions, vec!["SN", "INTL"]);
        assert_eq!(scope.stripped, None);
        // The bilateral compound names no country at all, so the treaty is
        // reached on the domestic scope, INTL included.
        let compound = query_scope("convention franco-sénégalaise d'exequatur", &home());
        assert_eq!(compound.jurisdictions, vec!["FR", "EU", "INTL"]);
    }

    #[test]
    fn traitement_is_not_an_instrument() {
        use super::query_scope;
        let scope = query_scope("traitement des données au maroc", &home());
        assert_eq!(scope.jurisdictions, vec!["MA"]);
    }

    #[test]
    fn every_jurisdiction_reads_back_from_its_url_segment() {
        use super::{from_url_segment, COUNTRIES, DOMESTIC_REGIONS};
        use lj_vocab::url_segment;
        let all = COUNTRIES
            .iter()
            .map(|(c, _)| *c)
            .chain(DOMESTIC_REGIONS.iter().copied());
        for code in all {
            let segment = url_segment(code);
            assert_eq!(segment, segment.to_lowercase(), "{code}");
            assert_eq!(from_url_segment(&segment), Some(code), "{code}");
        }
        assert_eq!(url_segment("EU"), "eu");
        assert_eq!(url_segment("INTL"), "intl");
    }

    #[test]
    fn a_text_slug_is_never_read_as_a_jurisdiction() {
        use super::from_url_segment;
        for slug in [
            "code-civil",
            "code-de-l-environnement",
            "constitution",
            "intl-machin",
            "eu-machin",
        ] {
            assert_eq!(from_url_segment(slug), None, "{slug}");
        }
    }
}
