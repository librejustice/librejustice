//! Aliases de textes juridiques : acronymes & noms usuels → forme développée.
//! Substitut **lexical** au sémantique pour la recherche par titre :
//! l'utilisateur tape « CPC », « RGPD », « loi Badinter » ; on OR-e la forme
//! développée (« code de procédure civile »…) dans la requête BM25 sur le titre
//! formé `legal_article.search_title`.
//!
//! Données curées embarquées (PUR, `include_str!`) sous
//! `data/legal_aliases.json`, ancrées sur le catalogue `legal_text`.

use std::sync::LazyLock;

use serde::Deserialize;

use lj_fold::fold;

/// JSON brut embarqué. Désérialisé une fois via [`ALIASES`].
const LEGAL_ALIASES_JSON: &str = include_str!("../data/legal_aliases.json");

/// Une entrée d'alias curée. Les autres champs du dataset (`kind`,
/// `catalog_status`) sont ignorés à la désérialisation (serde tolère les
/// champs JSON absents du struct).
#[derive(Debug, Clone, Deserialize)]
struct AliasEntry {
    /// Forme tapée par l'utilisateur (déjà minuscule/sans accent côté dataset ;
    /// re-`fold`ée au chargement par sûreté).
    alias: String,
    /// Forme développée à OR-er dans la requête (texte naturel ; le tokenizer
    /// ParadeDB fait l'ascii_folding).
    expansion: String,
    /// A slug several jurisdictions share (`code-civil`), read in the one
    /// searched.
    #[serde(default)]
    canonical_slug: Option<String>,
    /// The uid of the one text the alias names, which a slug remint leaves alone.
    #[serde(default)]
    canonical_text_uid: Option<String>,
    /// The spelling that names the text, for an alias that is an ordinary word
    /// otherwise: « TUE », where « tue » is a verb. Beside an article number, any.
    #[serde(default)]
    cased: Option<String>,
    /// An alias that names the text beside an article number alone: « cc » is a
    /// collective agreement or a court as often as it is the Code civil.
    #[serde(default)]
    cited_only: bool,
}

impl AliasEntry {
    fn written_in(&self, query: &str) -> bool {
        !self.cited_only && self.cased.as_ref().is_none_or(|c| contains_word(query, c))
    }
}

/// Alias chargés : `(alias foldé, entrée)`, dans l'ordre du dataset.
static ALIASES: LazyLock<Vec<(String, AliasEntry)>> = LazyLock::new(|| {
    let entries: Vec<AliasEntry> =
        serde_json::from_str(LEGAL_ALIASES_JSON).expect("legal_aliases.json embarqué valide");
    entries.into_iter().map(|e| (fold(&e.alias), e)).collect()
});

/// Nombre maximal d'expansions retournées (borne anti-bloat de requête).
const MAX_EXPANSIONS: usize = 4;

/// Expansions of the aliases `query` holds as whole words, its punctuation aside
/// (« C. civ. » holds « c civ », « accès » does not hold « css »): the expanded
/// forms, deduplicated, in dataset order, at most [`MAX_EXPANSIONS`].
pub fn expand_query(query: &str) -> Vec<String> {
    let (_, folded) = words(query);
    let mut out: Vec<String> = Vec::new();
    for (alias, entry) in ALIASES.iter() {
        if out.len() >= MAX_EXPANSIONS {
            break;
        }
        if holds(&folded, alias) && entry.written_in(query) && !out.contains(&entry.expansion) {
            out.push(entry.expansion.clone());
        }
    }
    out
}

/// Expansions dont l'alias couvre la requête **entière** (foldée) — la requête
/// EST le nom usuel d'un texte (« code civil du sénégal »), pas une requête qui
/// le contient (« article L442-1 du code de commerce »). Seules ces expansions
/// ont droit à la jambe conteneurs du ranking.
pub fn whole_query_expansions(query: &str) -> Vec<String> {
    let folded = fold(query);
    if folded.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    for (alias, entry) in ALIASES.iter() {
        if *alias == folded && entry.written_in(query) && !out.contains(&entry.expansion) {
            out.push(entry.expansion.clone());
        }
    }
    out
}

/// The catalogue slug of an alias typed **alone** that several jurisdictions
/// share (« cpc », « code civil »), where the public alphabet expects the slug.
/// `None` if the alias is unknown or anchored by uid ([`canonical_text_uid`]).
///
/// Le rapprochement se fait à la clé (séparateurs retirés) : un appelant MCP
/// passe le nom déjà slugifié (`loi-du-6-juillet-1989`), le dataset l'écrit en
/// clair, c'est la même entrée.
///
/// Complète [`strip_query_named_text`], qui cherche un alias *dans* une requête
/// en langue naturelle : ici l'entrée EST le nom du texte, telle qu'un appelant
/// d'API la passe dans un paramètre `code`.
pub fn canonical_slug(name: &str) -> Option<&'static str> {
    let key = crate::instrument_match::match_key(name);
    if key.is_empty() {
        return None;
    }
    ALIASES
        .iter()
        .find(|(alias, _)| crate::instrument_match::match_key(alias) == key)
        .and_then(|(_, e)| e.canonical_slug.as_deref())
}

/// The uid of the text an alias typed **alone** names (« cgi », « loi du 6
/// juillet 1989 »), matched like [`canonical_slug`]. `None` if the alias is
/// unknown or anchored by a shared slug.
pub fn canonical_text_uid(name: &str) -> Option<&'static str> {
    let key = crate::instrument_match::match_key(name);
    if key.is_empty() {
        return None;
    }
    ALIASES
        .iter()
        .find(|(alias, _)| crate::instrument_match::match_key(alias) == key)
        .and_then(|(_, e)| e.canonical_text_uid.as_deref())
}

/// Tous les slugs catalogue que le dataset d'alias désigne. Le fichier est écrit
/// à la main et nomme des slugs qu'une passe `assign-slugs` peut renommer sous
/// lui — l'invariant de fin de chaîne les confronte au catalogue.
pub fn canonical_slugs() -> impl Iterator<Item = &'static str> {
    ALIASES
        .iter()
        .filter_map(|(_, e)| e.canonical_slug.as_deref())
}

/// Les uid catalogue effectivement lus, c'est-à-dire ceux des entrées sans slug
/// — ailleurs le slug gagne et l'uid n'est jamais consulté. Même exposition que
/// [`canonical_slugs`] : une ancre écrite à la main que rien ne confronte au
/// catalogue vieillit sans le dire.
pub fn canonical_uids() -> impl Iterator<Item = &'static str> {
    ALIASES.iter().filter_map(|(_, e)| {
        e.canonical_slug
            .is_none()
            .then_some(e.canonical_text_uid.as_deref())
            .flatten()
    })
}

/// Entries with no anchor at all, `(alias, expansion)`: they claim the text is
/// outside the catalogue, and twelve of them claimed it wrongly while their text
/// was entering the corpus.
pub fn unanchored() -> impl Iterator<Item = (&'static str, &'static str)> {
    ALIASES
        .iter()
        .filter(|(_, e)| e.canonical_slug.is_none() && e.canonical_text_uid.is_none())
        .map(|(_, e)| (e.alias.as_str(), e.expansion.as_str()))
}

/// Connecteurs sans contenu autour ou dans un nom de code : ignorés à
/// l'alignement titre↔requête (« code travail » nomme le « Code du
/// travail ») et jamais porteurs à eux seuls (« du code civil » est
/// navigationnel).
const CODE_QUERY_CONNECTORS: &[&str] = &[
    "de", "du", "des", "d", "la", "le", "les", "l", "au", "aux", "et", "en", "dans", "dit", "que",
    "sur", "pour", "un", "une", "article", "articles", "art",
];

fn is_connector(tok: &str) -> bool {
    CODE_QUERY_CONNECTORS.contains(&tok)
}

/// La requête porte-t-elle le mot « code » ? Pré-garde bon marché avant de
/// charger le catalogue des codes pour [`strip_query_named_text`].
pub fn query_names_code(query: &str) -> bool {
    fold(query).split_whitespace().any(|t| t == "code")
}

/// Cible d'un texte nommé dans une requête, à résoudre côté store.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NamedTextTarget {
    /// Uid catalogue déjà connu (titre de code aligné, alias ancré par uid).
    Uid(String),
    /// Slug catalogue (alias ancré par slug).
    Slug(String),
    /// Motifs `LIKE` sur le slug (instrument cité par nature + numéro ou
    /// date : « loi 89-462 », « loi du 6 juillet 1989 » — les slugs
    /// encodent `nature-n-num-du-date-fr` de façon normalisée).
    SlugPatterns(Vec<String>),
}

/// Natures d'instruments cités par numéro ou date dans une requête.
const INSTRUMENT_NATURES: &[&str] = &["loi", "ordonnance", "decret"];

/// Mois français (forme slug/foldée) pour la forme « du 6 juillet 1989 ».
pub(crate) const FR_MONTHS: &[&str] = &[
    "janvier",
    "fevrier",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "aout",
    "septembre",
    "octobre",
    "novembre",
    "decembre",
];

/// Détecte un **texte nommé dans la requête** et renvoie `(cible, reste)` :
/// le texte visé (à résoudre côté store) et la requête débarrassée de son
/// nom (tokens d'origine préservés). Entrée des jambes « texte nommé » du
/// ranking : les tokens qui nomment le texte ne discriminent rien *à
/// l'intérieur* du texte visé. Trois détecteurs, du plus explicite au moins :
///
/// 1. **Titre de code du catalogue** (« clause pénale code civil ») :
///    connecteurs ignorés des deux côtés, titre le plus long gagnant ;
/// 2. **instrument par nature + numéro/date** (« préavis loi 89-462 ») → motifs
///    de slug, la date après le numéro avalée avec ;
/// 3. **alias du dataset** (« 145 cpc »), tokens contigus dans la requête.
///
/// `None` si la requête nomme un pays (la jambe pays possède le scoping), si
/// rien ne s'aligne, ou si rien de porteur ne subsiste hors du nom (requête
/// navigationnelle : jambe conteneurs).
pub fn strip_query_named_text(
    query: &str,
    codes: &[(String, String)],
) -> Option<(NamedTextTarget, String)> {
    if !crate::jurisdictions::query_jurisdictions(query).is_empty() {
        return None;
    }
    let orig: Vec<&str> = query
        .split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .filter(|t| !t.is_empty())
        .collect();
    let folded: Vec<String> = orig.iter().map(|t| fold(t)).collect();
    let (target, span) = match_code_title(&folded, codes)
        .or_else(|| match_instrument(&folded))
        .or_else(|| match_alias(&orig, &folded))?;
    strip_span(&orig, &folded, span).map(|rest| (target, rest))
}

/// Retire le span `[start, end)` (étendu aux connecteurs qui l'introduisent)
/// et renvoie le reste, ou `None` s'il n'en subsiste rien de porteur.
fn strip_span(
    orig: &[&str],
    folded: &[String],
    (mut start, end): (usize, usize),
) -> Option<String> {
    while start > 0 && is_connector(&folded[start - 1]) {
        start -= 1;
    }
    let rest: Vec<&str> = orig[..start]
        .iter()
        .chain(orig[end..].iter())
        .copied()
        .collect();
    if !rest.iter().any(|t| !is_connector(&fold(t))) {
        return None;
    }
    Some(rest.join(" "))
}

/// Détecteur 1 : titre de code du catalogue aligné sur la requête.
fn match_code_title(
    folded: &[String],
    codes: &[(String, String)],
) -> Option<(NamedTextTarget, (usize, usize))> {
    // Meilleur alignement : (nb de tokens porteurs du titre, span requête).
    let mut best: Option<(usize, usize, usize, &str)> = None;
    for (uid, title) in codes {
        let content: Vec<String> = title
            .split(|c: char| !(c.is_alphanumeric() || c == '-'))
            .filter(|t| !t.is_empty())
            .map(fold)
            .filter(|t| !is_connector(t))
            .collect();
        if content.first().map(String::as_str) != Some("code") {
            continue;
        }
        for start in 0..folded.len() {
            if folded[start] != "code" {
                continue;
            }
            let mut qi = start;
            let mut consumed = 0;
            for want in &content {
                while qi < folded.len() && is_connector(&folded[qi]) {
                    qi += 1;
                }
                if qi < folded.len() && folded[qi] == *want {
                    qi += 1;
                    consumed += 1;
                } else {
                    break;
                }
            }
            if consumed == content.len() && best.is_none_or(|(n, ..)| consumed > n) {
                best = Some((consumed, start, qi, uid));
            }
        }
    }
    let (_, start, end, uid) = best?;
    Some((NamedTextTarget::Uid(uid.to_string()), (start, end)))
}

/// Détecteur 2 : instrument cité par nature + numéro (« loi n° 89-462 »,
/// date qui suit avalée avec) ou nature + date (« loi du 6 juillet 1989 »).
/// Les motifs de slug couvrent les variantes `-n-`/`-no-` et les vieux
/// instruments sans numéro (« loi du 9 décembre 1905 »).
fn match_instrument(folded: &[String]) -> Option<(NamedTextTarget, (usize, usize))> {
    let is_num = |t: &str| {
        t.split_once('-').is_some_and(|(a, b)| {
            !a.is_empty()
                && !b.is_empty()
                && a.chars().all(|c| c.is_ascii_digit())
                && b.chars().all(|c| c.is_ascii_digit())
        })
    };
    // « du 6 juillet 1989 » à partir de `i` (sur « du ») → (date-slug, fin).
    let date_at = |mut i: usize| -> Option<(String, usize)> {
        if folded.get(i).map(String::as_str) != Some("du") {
            return None;
        }
        i += 1;
        let day = folded.get(i)?;
        if !(day == "1er" || day.chars().all(|c| c.is_ascii_digit()) && day.len() <= 2) {
            return None;
        }
        let month = folded.get(i + 1)?;
        if !FR_MONTHS.contains(&month.as_str()) {
            return None;
        }
        let year = folded.get(i + 2)?;
        if year.len() != 4 || !year.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        Some((format!("{day}-{month}-{year}"), i + 3))
    };
    for (start, tok) in folded.iter().enumerate() {
        if !INSTRUMENT_NATURES.contains(&tok.as_str()) {
            continue;
        }
        // Numéro dans la foulée (« n° »/« no »/« numero » tolérés).
        let mut qi = start + 1;
        while qi < folded.len() && matches!(folded[qi].as_str(), "n" | "no" | "numero") {
            qi += 1;
        }
        if qi < folded.len() && is_num(&folded[qi]) {
            let num = &folded[qi];
            let end = date_at(qi + 1).map_or(qi + 1, |(_, e)| e);
            return Some((
                NamedTextTarget::SlugPatterns(vec![format!("{tok}-n%-{num}-du-%")]),
                (start, end),
            ));
        }
        // Sinon, date seule (« loi du 6 juillet 1989 »).
        if let Some((date, end)) = date_at(start + 1) {
            return Some((
                NamedTextTarget::SlugPatterns(vec![
                    format!("{tok}-n%-du-{date}%"),
                    format!("{tok}-du-{date}%"),
                ]),
                (start, end),
            ));
        }
    }
    None
}

/// Détecteur 3 : alias du dataset ancré au catalogue (slug ou uid), tokens
/// contigus dans la requête. L'alias le plus long (en tokens) gagne.
fn match_alias(orig: &[&str], folded: &[String]) -> Option<(NamedTextTarget, (usize, usize))> {
    let (start, end, entry) = longest_alias(orig, folded, false)?;
    Some((anchor(entry), (start, end)))
}

/// The anchored alias `query` names, the whole query included, and the folded
/// tokens around it.
pub fn anchored_alias(query: &str) -> Option<(NamedTextTarget, Vec<String>)> {
    let (orig, folded) = words(query);
    let (start, end, entry) = longest_alias(&orig, &folded, true)?;
    let rest = folded[..start]
        .iter()
        .chain(&folded[end..])
        .cloned()
        .collect();
    Some((anchor(entry), rest))
}

/// The anchored alias that names the text of an article citation, when `query`
/// is nothing but the two: « constitution article 34 ».
pub fn cited_alias(query: &str) -> Option<NamedTextTarget> {
    let (orig, folded) = words(query);
    let (start, end, entry) = longest_alias(&orig, &folded, false)?;
    cites_only(&orig[..start], &orig[end..]).then(|| anchor(entry))
}

fn words(query: &str) -> (Vec<&str>, Vec<String>) {
    let orig: Vec<&str> = query
        .split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .filter(|t| !t.is_empty())
        .collect();
    let folded = orig.iter().map(|t| fold(t)).collect();
    (orig, folded)
}

fn holds(folded: &[String], alias: &str) -> bool {
    let toks: Vec<&str> = alias.split_whitespace().collect();
    !toks.is_empty() && folded.windows(toks.len()).any(|w| w.iter().eq(&toks))
}

/// The words around an alias are an article citation and nothing else,
/// connectors aside.
fn cites_only(before: &[&str], after: &[&str]) -> bool {
    let citation: Vec<&str> = before
        .iter()
        .chain(after)
        .copied()
        .filter(|t| !is_connector(&fold(t)))
        .collect();
    crate::article_key::designated_articles(&citation.join(" ")).is_some()
}

/// The longest anchored alias whose tokens stand contiguous in `folded`, written
/// as its entry requires unless the rest cites an article, as its span and entry;
/// `whole` lets it cover every token.
fn longest_alias(
    orig: &[&str],
    folded: &[String],
    whole: bool,
) -> Option<(usize, usize, &'static AliasEntry)> {
    let mut best: Option<(usize, usize, usize, &AliasEntry)> = None;
    for (alias, entry) in ALIASES.iter() {
        if entry.canonical_slug.is_none() && entry.canonical_text_uid.is_none() {
            continue;
        }
        let toks: Vec<&str> = alias.split_whitespace().collect();
        if toks.is_empty() || toks.len() > folded.len() || (!whole && toks.len() == folded.len()) {
            continue;
        }
        for start in 0..=(folded.len() - toks.len()) {
            if (0..toks.len()).all(|j| folded[start + j] == toks[j])
                && (entry.written_in(&orig[start..start + toks.len()].join(" "))
                    || cites_only(&orig[..start], &orig[start + toks.len()..]))
                && best.is_none_or(|(n, ..)| toks.len() > n)
            {
                best = Some((toks.len(), start, start + toks.len(), entry));
            }
        }
    }
    best.map(|(_, start, end, entry)| (start, end, entry))
}

fn anchor(entry: &AliasEntry) -> NamedTextTarget {
    match (&entry.canonical_slug, &entry.canonical_text_uid) {
        (Some(slug), _) => NamedTextTarget::Slug(slug.clone()),
        (None, Some(uid)) => NamedTextTarget::Uid(uid.clone()),
        (None, None) => unreachable!("filtré plus haut"),
    }
}

/// Forme d'une requête pour un match **conjonctif** sur le titre formé
/// `search_title` : tous les tokens retournés doivent exister dans le titre, donc
/// on élimine les tokens que le titre ne porte jamais et on recolle les numéros
/// d'article à la forme indexée.
///
/// - « article(s) » saute : le mot est exclu de `search_title`, le garder ferait
///   échouer TOUTE conjonction « article L442-1 du code de commerce » ;
/// - « L. 442-1 » se recolle en « L442-1 » : l'index tient le `num` en un seul
///   token, la forme pointée en produit deux qui ne matchent jamais ;
/// - les stopwords FR restent, le tokenizer les élimine des deux côtés.
///
/// The split is the index's, [`lj_textsearch::body_tok::tokens_raw`], not a
/// home-made one: many articles carry a `num` with a separator.
///
/// `None` si rien ne subsiste — l'appelant n'émet alors pas de conjonction.
pub fn conj_title_query(query: &str) -> Option<String> {
    let tokens: Vec<&str> = lj_textsearch::body_tok::tokens_raw(query)
        .into_iter()
        .filter(|t| {
            let t = fold(t);
            t != "article" && t != "articles" && t != "art"
        })
        .collect();
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut iter = tokens.into_iter().peekable();
    while let Some(tok) = iter.next() {
        let is_ref_letter = tok.len() == 1 && matches!(fold(tok).as_str(), "l" | "r" | "d");
        let next_is_num = iter
            .peek()
            .is_some_and(|n| n.chars().next().is_some_and(|c| c.is_ascii_digit()));
        if is_ref_letter && next_is_num {
            out.push(format!("{tok}{}", iter.next().expect("peeked")));
        } else {
            out.push(tok.to_string());
        }
    }
    (!out.is_empty()).then(|| out.join(" "))
}

/// Does `title` carry every token `rest` spends naming a text? The formed title
/// an index matches carries the leaf division too, which names no text:
/// « responsabilité 1240 » reaches article 1240 of the Code civil through it, and
/// confronting the code title alone is what refuses that reading.
pub fn title_names_text(rest: &str, title: &str) -> bool {
    let content = |s: &str| -> Vec<String> {
        s.split(|c: char| !(c.is_alphanumeric() || c == '-'))
            .filter(|t| !t.is_empty())
            .map(fold)
            .filter(|t| !is_connector(t))
            .collect()
    };
    let wanted = content(rest);
    let held = content(title);
    !wanted.is_empty() && wanted.iter().all(|t| held.contains(t))
}

/// `needle` apparaît dans `haystack` borné par des non-alphanumériques (ou les
/// bords). `haystack` et `needle` sont déjà foldés (minuscule, espaces collapsés).
pub(crate) fn contains_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(rel) = haystack[from..].find(needle) {
        let start = from + rel;
        let end = start + needle.len();
        let before_ok = start == 0
            || !haystack[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric);
        let after_ok = end == haystack.len()
            || !haystack[end..]
                .chars()
                .next()
                .is_some_and(char::is_alphanumeric);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{conj_title_query, contains_word, expand_query, NamedTextTarget};

    #[test]
    fn conj_query_drops_article_and_glues_num() {
        assert_eq!(
            conj_title_query("article L. 442-1 du code de commerce").as_deref(),
            Some("L442-1 du code de commerce")
        );
        assert_eq!(
            conj_title_query("article 15 de la loi du 6 juillet 1989").as_deref(),
            Some("15 de la loi du 6 juillet 1989")
        );
        assert_eq!(
            conj_title_query("R600-2 urbanisme").as_deref(),
            Some("R600-2 urbanisme")
        );
        assert_eq!(
            conj_title_query("délai d'un mois").as_deref(),
            Some("délai d un mois")
        );
    }

    #[test]
    fn conj_query_keeps_a_num_with_separator_whole() {
        assert_eq!(
            conj_title_query("article 2.1 convention de New York").as_deref(),
            Some("2.1 convention de New York")
        );
    }

    #[test]
    fn conj_query_empty_when_nothing_left() {
        assert_eq!(conj_title_query("article"), None);
        assert_eq!(conj_title_query(""), None);
    }

    #[test]
    fn acronym_expands_to_full_name() {
        let exp = expand_query("cpc");
        assert!(
            exp.iter().any(|e| e == "code de procédure civile"),
            "{exp:?}"
        );
    }

    #[test]
    fn acronym_in_phrase_expands() {
        let exp = expand_query("article 700 cpc");
        assert!(
            exp.iter().any(|e| e == "code de procédure civile"),
            "{exp:?}"
        );
    }

    #[test]
    fn usual_name_expands() {
        let exp = expand_query("loi badinter");
        assert!(!exp.is_empty(), "loi badinter doit s'étendre");
    }

    #[test]
    fn accented_query_matches_unaccented_alias() {
        let exp = expand_query("RGPD");
        assert!(!exp.is_empty(), "rgpd doit s'étendre");
    }

    #[test]
    fn plain_query_yields_no_expansion() {
        assert!(expand_query("responsabilité du fait des choses").is_empty());
    }

    #[test]
    fn whole_query_alias_expands_embedded_does_not() {
        use super::whole_query_expansions;
        assert_eq!(
            whole_query_expansions("code civil du sénégal"),
            vec!["code de la famille sénégalais".to_string()]
        );
        assert!(whole_query_expansions("article 700 cpc").is_empty());
        assert!(whole_query_expansions("").is_empty());
    }

    #[test]
    fn an_alias_typed_alone_resolves_to_its_anchor() {
        use super::{canonical_slug, canonical_text_uid};
        assert_eq!(canonical_text_uid("cgi"), Some("LEGITEXT000006069577"));
        assert_eq!(canonical_text_uid("ceseda"), Some("LEGITEXT000006070158"));
        assert_eq!(canonical_slug("CPC"), Some("code-de-procedure-civile"));
        assert_eq!(canonical_text_uid("CPC"), None);
        // A usual name the caller already slugified is the same entry as the
        // name written out, or an MCP client never reaches it.
        assert_eq!(
            canonical_text_uid("loi-du-6-juillet-1989"),
            Some("JORFTEXT000000509310")
        );
        assert_eq!(
            canonical_text_uid("loi du 6 juillet 1989"),
            Some("JORFTEXT000000509310")
        );
        assert_eq!(canonical_slug("code-civil"), Some("code-civil"));
        assert_eq!(canonical_slug("pas-un-alias"), None);
        assert_eq!(canonical_text_uid("pas-un-alias"), None);
        assert_eq!(canonical_slug(""), None);
    }

    #[test]
    fn word_boundary_prevents_substring_false_positive() {
        assert!(!contains_word("acces", "css"));
        assert!(!contains_word("classe interne", "css"));
        assert!(contains_word("le css ouvre", "css"));
    }

    fn fake_codes() -> Vec<(String, String)> {
        [
            ("CIV", "Code civil"),
            ("TRAV", "Code du travail"),
            ("CONSO", "Code de la consommation"),
            ("COM", "Code de commerce"),
        ]
        .map(|(u, t)| (u.to_string(), t.to_string()))
        .to_vec()
    }

    fn uid(s: &str) -> NamedTextTarget {
        NamedTextTarget::Uid(s.to_string())
    }

    #[test]
    fn strip_named_text_code_title_anywhere() {
        use super::strip_query_named_text;
        let codes = fake_codes();
        assert_eq!(
            strip_query_named_text("clause pénale code civil", &codes),
            Some((uid("CIV"), "clause pénale".to_string()))
        );
        assert_eq!(
            strip_query_named_text("le code civil et la clause pénale", &codes),
            Some((uid("CIV"), "et la clause pénale".to_string()))
        );
        assert_eq!(
            strip_query_named_text("article 1240 du code civil", &codes),
            Some((uid("CIV"), "article 1240".to_string()))
        );
        assert_eq!(
            strip_query_named_text("période d'essai code du travail", &codes),
            Some((uid("TRAV"), "période d essai".to_string()))
        );
        assert_eq!(
            strip_query_named_text("garantie de conformité code consommation", &codes),
            Some((uid("CONSO"), "garantie de conformité".to_string()))
        );
    }

    #[test]
    fn strip_named_text_instrument_num_and_date() {
        use super::strip_query_named_text;
        assert_eq!(
            strip_query_named_text("congé pour vendre loi n° 89-462 du 6 juillet 1989", &[]),
            Some((
                NamedTextTarget::SlugPatterns(vec!["loi-n%-89-462-du-%".to_string()]),
                "congé pour vendre".to_string()
            ))
        );
        assert_eq!(
            strip_query_named_text("clause pénale ordonnance 2016-131", &[]),
            Some((
                NamedTextTarget::SlugPatterns(vec!["ordonnance-n%-2016-131-du-%".to_string()]),
                "clause pénale".to_string()
            ))
        );
        assert_eq!(
            strip_query_named_text("article 15 de la loi du 6 juillet 1989", &[]),
            Some((
                NamedTextTarget::SlugPatterns(vec![
                    "loi-n%-du-6-juillet-1989%".to_string(),
                    "loi-du-6-juillet-1989%".to_string(),
                ]),
                "article 15".to_string()
            ))
        );
        assert_eq!(
            strip_query_named_text("loi sur les rapports locatifs", &[]),
            None
        );
    }

    #[test]
    fn strip_named_text_alias_anchored() {
        use super::strip_query_named_text;
        assert_eq!(
            strip_query_named_text("article 145 cpc", &[]),
            Some((
                NamedTextTarget::Slug("code-de-procedure-civile".to_string()),
                "article 145".to_string()
            ))
        );
        assert_eq!(
            strip_query_named_text("indemnisation accident loi badinter", &[]),
            Some((
                uid("JORFTEXT000000693454"),
                "indemnisation accident".to_string()
            ))
        );
        assert_eq!(strip_query_named_text("cpc", &[]), None);
    }

    #[test]
    fn a_code_abbreviation_names_its_code_with_or_without_dots() {
        use super::strip_query_named_text;
        let civil = || uid("LEGITEXT000006070721");
        assert_eq!(
            strip_query_named_text("c. civ. 1240", &[]),
            Some((civil(), "1240".to_string()))
        );
        assert_eq!(
            strip_query_named_text("art. 1240 C.civ.", &[]),
            Some((civil(), "art 1240".to_string()))
        );
        assert_eq!(
            strip_query_named_text("C. pr. civ. exéc., art. L. 111-3", &[]),
            Some((uid("LEGITEXT000025024948"), "art L 111-3".to_string()))
        );
        assert!(expand_query("C. civ. responsabilité du fait des choses")
            .contains(&"code civil".to_string()));
    }

    #[test]
    fn cc_names_the_code_civil_beside_an_article_number_alone() {
        use super::strip_query_named_text;
        let civil = || uid("LEGITEXT000006070721");
        assert_eq!(
            strip_query_named_text("Cc 1240", &[]),
            Some((civil(), "1240".to_string()))
        );
        assert_eq!(
            strip_query_named_text("article 1240 cc", &[]),
            Some((civil(), "article 1240".to_string()))
        );
        assert_eq!(strip_query_named_text("cc responsabilité", &[]), None);
        assert!(expand_query("CC Syntec").iter().all(|e| e != "code civil"));
    }

    #[test]
    fn strip_named_text_guards() {
        use super::strip_query_named_text;
        let codes = fake_codes();
        assert_eq!(
            strip_query_named_text("code de la consommation", &codes),
            None
        );
        assert_eq!(strip_query_named_text("le code du travail", &codes), None);
        assert_eq!(
            strip_query_named_text("clause pénale code civil belge", &codes),
            None
        );
        assert_eq!(strip_query_named_text("clause pénale code", &codes), None);
        assert_eq!(strip_query_named_text("code du sport dopage", &codes), None);
        assert_eq!(strip_query_named_text("frais irrépétibles", &codes), None);
    }

    #[test]
    fn le_titre_de_code_porte_le_nom_pas_la_division() {
        use super::title_names_text;
        assert!(title_names_text("code civil", "Code civil"));
        assert!(title_names_text("du code du travail", "Code du travail"));
        assert!(title_names_text("code commerce", "Code de commerce"));
        // The leaf division of the formed title names no text.
        assert!(!title_names_text("responsabilité", "Code civil"));
        assert!(!title_names_text("code civil belge", "Code civil"));
        assert!(!title_names_text("du", "Code civil"));
    }
}
