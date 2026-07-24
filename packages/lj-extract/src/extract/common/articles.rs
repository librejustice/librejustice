//! Normalisation des articles, extraction de tokens d'article, validation et
//! SALVAGE garble → code. Port de la partie « Articles » de `extract/common.py`.
//! Regexes = clé de LINKING du référentiel citations — exception ADR 0116.

use std::sync::LazyLock;

use regex::Regex;

// Suffixes d'article RÉELS (articles distincts : « L. 80 B », « 1 bis »…),
// triés longueur décroissante : l'alternation `regex` est leftmost-first, un
// préfixe placé avant son mot long le court-circuiterait (« quater » mangeait
// « quatertricies » → clé tronquée, collision CGI — ADR 0236).
// Les marqueurs ordinaux (1er, 1ère, 1re, 2ème) ne sont PAS des suffixes
// distinctifs : « article 1er » = « article premier » = article 1. On les exclut
// pour que `article_core` tronque au chiffre (1er → 1), forme canonique que la GT
// emploie. La comparaison de l'éval normalisant les deux côtés, ça ne peut
// qu'apparier davantage (1er ≡ 1), jamais créer de faux appariement.
pub(crate) const ART_NUM_SUFFIX: &str = r"quatertricies|quinquedecies|quaterdecies|quatervicies|quintricies|septtricies|duotricies|novodecies|novovicies|octodecies|octovicies|quindecies|quinvicies|septdecies|septvicies|sextricies|tertricies|duodecies|duovicies|quinquies|sexdecies|sexvicies|terdecies|tervicies|untricies|cinquies|undecies|unvicies|septies|sexties|tricies|decies|nonies|novies|octies|quarto|quater|sexies|vicies|bis|ter";

// `_RE_ART_TRAILING_NOISE`.
static RE_ART_TRAILING_NOISE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i),\s*(?:alors\s+(?:en\s+vigueur|applicable)|dans\s+sa\s+r[eé]daction|pr[eé]cit[eé]e?s?|cit[eé]e?s?|devenue?s?|anciennement|du\s+m[eê]me\s+code|de\s+ce\s+code|il\s+est\s+fait\s+application)\b.*$",
    )
    .unwrap()
});
// `_RE_ART_PREFIX_HEAD` : `^([LRDA])\.*\s*(?=\d)`. Lookahead `(?=\d)` → capture.
// Le tiret est accepté comme séparateur de préfixe (« L-624-1 » → « L. 624-1 »,
// variante OCR/greffe mesurée en prod sur 451 couples).
static RE_ART_PREFIX_HEAD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^([LRDA])[.\s-]*(\d)").unwrap());
// Préfixe « LP » (loi du pays — codes territoriaux PF/NC, ADR 0128). Le cœur
// numérique « 711-1 » identifie l'article à lui seul ; les juridictions citent
// majoritairement SANS le préfixe (« 711-1 ») alors que la désignation officielle
// est « LP. 711-1 ». On STRIPPE donc « LP »/« Lp »/« L.P. » en tête (toutes
// variantes dots/espaces) vers le cœur, AVANT la canonicalisation mono-lettre
// `[LRDA]`, pour que « LP. 711-1 », « LP 711-1 » et « 711-1 » convergent vers une
// clé unique. Pas de collision : aucun article mono-lettre ne commence par « LP ».
static RE_ART_LP_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^l\.?\s*p\.?[.\s-]*(\d)").unwrap());
// `_RE_ART_NORMALIZE` : `\b([LlRrAa])\.?\s*(\d)`.
static RE_ART_NORMALIZE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b([LRA])\.?\s*(\d)").unwrap());
// `_RE_ART_PREFIX_NUM` : `^[LRDA]\.`.
static RE_ART_PREFIX_NUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^[LRDA]\.").unwrap());
// Séparateurs inter-chiffres : `_normalize_article` les réécrit en `-` via des
// regex à lookbehind/lookahead ZÉRO-LARGEUR (`(?<=\d)\s*-\s*(?=\d)` puis
// `(?<=\d)\s+(?=\d)`). Le crate `regex` n'a pas de lookaround → on matche le
// SÉPARATEUR seul (sans les chiffres) et on vérifie les bornes via `collapse_between_digits`.
// Crucial : ne pas consommer les chiffres permet aux collapses adjacents de
// s'enchaîner (« 600-4 -1 » → « 600-4-1 »), fidèle au zéro-largeur Python ;
// un `(\d)…(\d)` consommerait le `4` partagé et laisserait « 600-4 -1 ».
static RE_ART_NORM_DASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*-\s*").unwrap());
static RE_ART_NORM_DOT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\.\s*").unwrap());
static RE_ART_NORM_SPACE_DASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static RE_ART_NORM_TRAILING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s+(?:précit[eé]e?s?|susvis[éè]e?s?|modifi[ée]e?s?)$").unwrap()
});
static RE_ART_NORM_SUBPART: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[,\s]+(?:alinéa|alinéas|paragraphe|paragraphes)\s+\d+\b.*$").unwrap()
});
static RE_ART_NORM_SUIVANTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s+et\s+suivants?\b.*$").unwrap());
static RE_ART_NORM_CONDITIONS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s+dans\s+les\s+conditions\s+pr[eé]vues\b.*$").unwrap());
static RE_ART_NORM_WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static RE_ART_NORM_TRAIL_PUNCT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[,;\s]+$").unwrap());

/// Réécrit en `-` chaque match de `re` (un séparateur inter-chiffres) qui est
/// effectivement borné par un chiffre des deux côtés — émulation des lookbehind
/// /lookahead zéro-largeur de Python (`(?<=\d)…(?=\d)`). Le séparateur ne
/// contenant pas les chiffres, les collapses adjacents s'enchaînent comme côté
/// Python (`re.sub` non-chevauchant sur le seul séparateur).
fn collapse_between_digits(s: &str, re: &Regex) -> String {
    re.replace_all(s, |c: &regex::Captures| {
        let m = c.get(0).unwrap();
        let before = s[..m.start()].chars().next_back();
        let after = s[m.end()..].chars().next();
        if before.is_some_and(|c| c.is_ascii_digit()) && after.is_some_and(|c| c.is_ascii_digit()) {
            "-".to_string()
        } else {
            m.as_str().to_string()
        }
    })
    .into_owned()
}

/// `_normalize_article`.
pub fn normalize_article(raw: &str) -> String {
    let mut raw = RE_ART_NORM_WS
        .replace_all(raw.trim(), " ")
        .replace(['º', '°'], "")
        .trim()
        .to_string();
    raw = collapse_between_digits(&raw, &RE_ART_NORM_DASH);
    raw = collapse_between_digits(&raw, &RE_ART_NORM_SPACE_DASH);
    // Sous-numéros pointés (conventions KALI « 1.01 », « 31.5 ») : le point
    // inter-chiffres est un séparateur de segment comme le tiret — même pliage
    // que `article_key` (« 1.01 » ≡ clé `1-01`), sinon `article_core` tronque
    // au premier segment et confond « 1.01 » avec « 1 » (ADR 0236).
    raw = collapse_between_digits(&raw, &RE_ART_NORM_DOT);
    raw = RE_ART_NORM_TRAILING.replace(&raw, "").into_owned();
    raw = RE_ART_NORM_SUBPART.replace(&raw, "").into_owned();
    raw = RE_ART_NORM_SUIVANTS.replace(&raw, "").into_owned();
    raw = RE_ART_NORM_CONDITIONS.replace(&raw, "").into_owned();
    raw = RE_ART_TRAILING_NOISE.replace(&raw, "").into_owned();
    // Loi du pays : « LP. 711-1 »/« LP 711-1 »/« L.P. 711-1 » → « 711-1 » (cœur
    // numérique), avant la canonicalisation mono-lettre, pour converger avec la
    // forme nu citée par les juridictions (cf. RE_ART_LP_PREFIX).
    raw = RE_ART_LP_PREFIX.replace(&raw, "$1").into_owned();
    raw = RE_ART_PREFIX_HEAD
        .replace(&raw, |c: &regex::Captures| {
            format!("{}. {}", c[1].to_uppercase(), &c[2])
        })
        .into_owned();
    raw = RE_ART_NORMALIZE.replace_all(&raw, "$1. $2").into_owned();
    raw = RE_ART_NORM_TRAIL_PUNCT.replace(&raw, "").into_owned();
    // OCR « I » pour « 1 » en suffixe (« L. 761-I ») — un seul I derrière un
    // chiffre, jamais « -II »/« -III » (romains légitimes).
    if let Some(stripped) = raw.strip_suffix("-I") {
        if stripped.ends_with(|c: char| c.is_ascii_digit()) {
            raw = format!("{stripped}-1");
        }
    }
    // Troncature au cœur de l'identifiant (port de `_RE_ARTICLE_CORE`).
    let starts_digit = raw.chars().next().is_some_and(|c| c.is_ascii_digit());
    if starts_digit || RE_ART_PREFIX_NUM.is_match(&raw) {
        if let Some(core) = article_core(&raw) {
            if !core.is_empty() {
                raw = core.trim_end_matches([' ', ',', ';']).to_string();
            }
        }
    }
    raw
}

/// Port de `_RE_ARTICLE_CORE` (négations de lookahead non portables en `regex`).
fn article_core(raw: &str) -> Option<String> {
    static RE_HEAD: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^(?:[LRDA]\.\s*)?\d+(?:-\d+)*").unwrap());
    static RE_SUFFIX_ORDINAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(&format!(r"(?i)^\s*(?:{ART_NUM_SUFFIX})")).unwrap());
    static RE_DASH_NUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*-\s*\d+").unwrap());
    let head = RE_HEAD.find(raw)?;
    let mut end = head.end();
    loop {
        let rest = &raw[end..];
        if let Some(m) = RE_SUFFIX_ORDINAL.find(rest) {
            // Frontière de mot : un ordinal suivi d'une minuscule est un mot de
            // prose qui COMMENCE comme un ordinal (« ter » dans « termine »),
            // pas un suffixe — même règle latin-1 que `match_isolated_letter`.
            let cont = rest[m.end()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_lowercase() || ('\u{e0}'..='\u{ff}').contains(&c));
            if m.start() == 0 && !cont {
                end += m.end();
                continue;
            }
        }
        // Groupes tiret-chiffres APRÈS un suffixe ou une lettre (« 46 quater-0 W » :
        // le « -0 » est un discriminant d'article distinct, pas une troncature).
        if let Some(m) = RE_DASH_NUM.find(rest) {
            if m.start() == 0 {
                end += m.end();
                continue;
            }
        }
        if let Some(n) = match_isolated_letter(rest) {
            end += n;
            continue;
        }
        break;
    }
    Some(raw[..end].to_string())
}

/// Reproduit `\s*[A-Za-z](?![a-zà-ÿ])` : espaces puis une lettre ASCII non
/// suivie d'une minuscule (ASCII ou latin-1). Renvoie les octets consommés.
fn match_isolated_letter(rest: &str) -> Option<usize> {
    let trimmed = rest.trim_start_matches([' ', '\t']);
    let ws = rest.len() - trimmed.len();
    let mut it = trimmed.chars();
    let letter = it.next()?;
    if !letter.is_ascii_alphabetic() {
        return None;
    }
    if let Some(next) = it.next() {
        let is_lower_latin = next.is_ascii_lowercase() || ('\u{e0}'..='\u{ff}').contains(&next);
        if is_lower_latin {
            return None;
        }
    }
    Some(ws + letter.len_utf8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_article_chains_adjacent_dash_collapses() {
        // Parité `_normalize_article` : les collapses inter-chiffres sont à
        // lookbehind/lookahead ZÉRO-LARGEUR → un chiffre partagé n'est pas
        // consommé, donc « 600-4 -1 » s'enchaîne en « 600-4-1 » (le port naïf
        // `(\d)…(\d)` consommait le « 4 » et tronquait à « L. 600-4 »).
        assert_eq!(normalize_article("L. 600-4 -1"), "L. 600-4-1");
        assert_eq!(normalize_article("L. 1235-3 -1"), "L. 1235-3-1");
        // Espaces purs enchaînés entre chiffres : « 1 2 3 » → « 1-2-3 ».
        assert_eq!(normalize_article("L. 1 2 3"), "L. 1-2-3");
        // La lettre de division n'est pas un chiffre → « 80 B » préservé.
        assert_eq!(normalize_article("L. 80 B"), "L. 80 B");
    }

    #[test]
    fn normalize_article_basic_prefix_and_dash() {
        assert_eq!(normalize_article("l. 761-1"), "L. 761-1");
        assert_eq!(normalize_article("L 521 2"), "L. 521-2");
        assert_eq!(normalize_article("R..2181-3"), "R. 2181-3");
    }

    #[test]
    fn normalize_article_strips_subpart_and_suivants() {
        assert_eq!(normalize_article("L. 312-8 alinéa 1"), "L. 312-8");
        assert_eq!(normalize_article("R. 421-1 et suivants"), "R. 421-1");
    }

    #[test]
    fn normalize_article_keeps_real_suffix_distinct_from_base() {
        // Un suffixe RÉEL collé au cœur par un ESPACE est un article distinct
        // (« 605-1 bis » ≠ « 605-1 ») : il survit à `article_core`. Un tiret, lui,
        // est un séparateur de segment → tronqué (« 605-1-bis » s'effondrerait sur
        // « 605-1 »). Les datasets curés posent donc le suffixe avec un espace.
        assert_eq!(normalize_article("605-1 bis"), "605-1 bis");
        assert_eq!(normalize_article("605-1 ter"), "605-1 ter");
        assert_eq!(normalize_article("131-1 bis"), "131-1 bis");
        assert_ne!(normalize_article("605-1 bis"), normalize_article("605-1"));
        // `quarto` (ordinal idiosyncratique du CPP nigérien, là où le canon
        // français emploie `quater`) est un suffixe reconnu → préservé, distinct.
        assert_eq!(normalize_article("605-11 quarto"), "605-11 quarto");
        assert_ne!(
            normalize_article("605-11 quarto"),
            normalize_article("605-11")
        );
    }

    #[test]
    fn normalize_article_serie_ordinale_complete_et_frontiere() {
        // Série ordinale au-delà de « vicies » (annexes CGI, ADR 0236) : le mot
        // ENTIER est le suffixe — l'alternation longueur-décroissante empêche
        // « quater » de manger « quatertricies ».
        assert_eq!(normalize_article("199 duovicies"), "199 duovicies");
        assert_eq!(normalize_article("199 novovicies"), "199 novovicies");
        assert_eq!(normalize_article("199 quatertricies"), "199 quatertricies");
        assert_ne!(
            normalize_article("199 quatertricies"),
            normalize_article("199 quater")
        );
        // Frontière de mot : un mot de prose qui COMMENCE comme un ordinal
        // n'est pas un suffixe (« terrain » ≠ « ter »).
        assert_eq!(normalize_article("15 terrain"), "15");
    }

    #[test]
    fn normalize_article_discriminants_post_suffixe_et_points() {
        // Discriminant tiret-chiffre APRÈS le suffixe (« 46 quater-0 W », CGI
        // annexe III) : article distinct, préservé intégralement.
        assert_eq!(normalize_article("46 quater-0 W"), "46 quater-0 W");
        assert_ne!(
            normalize_article("46 quater-0 W"),
            normalize_article("46 quater")
        );
        // Sous-numéros pointés (conventions KALI) : « 1.01 » ≡ clé « 1-01 »,
        // distinct de « 1 » — même pliage que `article_key`.
        assert_eq!(normalize_article("1.01"), "1-01");
        assert_eq!(normalize_article("31.5"), "31-5");
        assert_ne!(normalize_article("1.01"), normalize_article("1"));
    }

    #[test]
    fn normalize_article_core_cuts_trailing_prose() {
        assert_eq!(normalize_article("L. 225-106 dernier alinéa"), "L. 225-106");
        // « 27 ancien » → « 27 » (lettre suivie d'une minuscule = prose).
        assert_eq!(normalize_article("27 ancien"), "27");
    }

    // Variante tiret-préfixe (451 couples mesurés en prod) : « L-624-1 »
    // est la même citation que « L. 624-1 », normalisée à l'extraction.
    #[test]
    fn normalize_article_dash_prefix_separator() {
        assert_eq!(normalize_article("L-624-1"), "L. 624-1");
        assert_eq!(normalize_article("R-421-1"), "R. 421-1");
        assert_eq!(normalize_article("D-161-2"), "D. 161-2");
        // Les formes déjà propres restent stables.
        assert_eq!(normalize_article("L. 624-1"), "L. 624-1");
        assert_eq!(normalize_article("L624-1"), "L. 624-1");
    }

    #[test]
    fn normalize_article_loi_du_pays_prefix_collapses_to_core() {
        // Loi du pays (codes territoriaux PF/NC) : « LP. N »/« LP N »/« L.P. N »
        // convergent vers le cœur numérique nu — forme dominante citée par les
        // juridictions (« 711-1 » 78×) alors que l'officiel porte « LP. ».
        assert_eq!(normalize_article("LP. 711-1"), "711-1");
        assert_eq!(normalize_article("LP 711-1"), "711-1");
        assert_eq!(normalize_article("Lp. 421-1"), "421-1");
        assert_eq!(normalize_article("Lp 423-1"), "423-1");
        assert_eq!(normalize_article("L.P. 340-9"), "340-9");
        assert_eq!(normalize_article("LP. 715-7"), "715-7");
        // La forme nu est inchangée → collapse avec les variantes LP ci-dessus.
        assert_eq!(normalize_article("711-1"), "711-1");
        // NON-RÉGRESSION : un article mono-lettre « L. » n'est PAS un loi du pays
        // (pas de « p » après le L) → préfixe conservé, distinct du cœur nu.
        assert_eq!(normalize_article("L. 711-1"), "L. 711-1");
        assert_eq!(normalize_article("L. 761-1"), "L. 761-1");
        // « A. »/« D. » (arrêté/délibération) restent canonicalisés mono-lettre.
        assert_eq!(normalize_article("D. 340-2"), "D. 340-2");
        assert_eq!(normalize_article("A. 1640-9-3"), "A. 1640-9-3");
    }
}
