//! Clé publique d'un numéro d'article, segment d'URL `/norm/{juris}/{code}/{clé}` et
//! alphabet des colonnes `num_key`.
//!
//! Alphabet : minuscules ASCII, chiffres, `-`, plus `()`. Le préfixe de partie
//! (`L`, `R`, `D`, `A`, `LO`, `LP`) est collé au numéro : « L. 761-1 » →
//! `l761-1`. Tout séparateur se plie en `-` : « 106 Bis » → `106-bis`.
//! L'astérisque de Légifrance, marqueur et non séparateur, s'efface sans rien
//! laisser : « R*600-1 » → `r600-1` comme « R. 600-1 ». Le libellé autoritaire
//! qui la garde est `legal_article.num`.
//!
//! La fonction est un point fixe (`article_key(article_key(x)) ==
//! article_key(x)`) : les backfills se rejouent, et toute forme citée d'un
//! même numéro (« L761-1 », « l. 761-1 », « L 761-1 ») tombe sur la même clé.

/// Préfixes de partie collés au numéro (minuscules, après pliage).
const GLUED_PREFIXES: [&str; 6] = ["l", "r", "d", "a", "lo", "lp"];

/// Clé publique d'un numéro d'article (voir doc de module).
pub fn article_key(num: &str) -> String {
    let mut folded = String::with_capacity(num.len());
    for c in num
        .chars()
        .filter(|c| *c != '*')
        .flat_map(lj_fold::fold_char)
    {
        if c.is_ascii_alphanumeric() || matches!(c, '(' | ')') {
            folded.push(c);
        } else if !folded.is_empty() && !folded.ends_with('-') {
            folded.push('-');
        }
    }
    let folded = folded.trim_end_matches('-');

    // Collage du préfixe de partie : `l-761-1` → `l761-1`.
    for prefix in GLUED_PREFIXES {
        if let Some(tail) = folded.strip_prefix(prefix) {
            if let Some(tail) = tail.strip_prefix('-') {
                if tail.starts_with(|c: char| c.is_ascii_digit()) {
                    return format!("{prefix}{tail}");
                }
            }
        }
    }
    folded.to_string()
}

/// Clé d'IDENTITÉ d'un numéro d'article servi intégral — la valeur de
/// `legal_article.num_key` et du segment d'URL `/norm/{juris}/{code}/{clé}`.
///
/// Distincte de la clé de CITATION (`normalize_article` de `lj-extract`, pliée par
/// [`article_key`]) : celle-ci tronque volontairement au cœur de l'identifiant
/// (ordinaux hors vocabulaire, discriminants « -0 W », numéros pointés « 1.01 »),
/// ce qui apparie des citations bruitées mais confond des articles distincts.
/// L'identité, elle, garde le numéro DILA entier à deux normalisations près :
/// le marqueur ordinal de tête (« 1er » ≡ « 1 », « 1er bis » ≡ « 1 bis »,
/// « premier » ≡ « 1 ») et l'astérisque, tous deux marqueurs et non
/// discriminants. Point fixe comme `article_key`.
pub fn identity_key(num: &str) -> String {
    let t = num.trim();
    if t.eq_ignore_ascii_case("premier") {
        return "1".to_string();
    }
    let nd = t.bytes().take_while(|b| b.is_ascii_digit()).count();
    if nd > 0 {
        let rest = &t[nd..];
        let low = rest.to_lowercase();
        for marker in ["ère", "ere", "er"] {
            if low.starts_with(marker)
                && low[marker.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric())
            {
                return article_key(&format!("{}{}", &t[..nd], &rest[marker.len()..]));
            }
        }
    }
    article_key(t)
}

/// Clé d'identité d'un article de **recueil** : le numéro plié, qualifié par la
/// pièce qui le numérote. `("KALITEXT000005675711", "2")` → `kalitext000005675711-2`.
///
/// Un recueil rassemble des pièces qui renumérotent chacune à partir de 1, et le
/// numéro seul n'y identifie rien. Les articles de la pièce de tête gardent leur
/// clé nue : c'est eux que désigne un numéro cité sans autre précision. Point
/// fixe comme [`article_key`], dont l'alphabet absorbe l'identifiant de pièce.
pub fn piece_key(piece_uid: &str, num_key: &str) -> String {
    format!("{}-{}", article_key(piece_uid), num_key)
}

/// Même clé, séparateur réinséré à la frontière chiffre/lettre finale : `l80a`
/// → `l80-a`, `119bis` → `119-bis`. `None` hors de cette forme, clé déjà
/// séparée comprise (`l80-a`, `l312-12`, `lo6213-1`).
///
/// Recovers an article read once the exact key has failed, the fonds carrying
/// `l80-a` where the caller writes `l80a`. The bound is narrow on purpose:
/// ignoring the separators would merge `l312-12` and `l312-1-2`, two distinct
/// articles, where the digit/letter boundary hits almost no key of the fonds.
pub fn letter_suffix_separated(key: &str) -> Option<String> {
    let b = key.as_bytes();
    let mut cut = b.len();
    while cut > 0 && b[cut - 1].is_ascii_lowercase() {
        cut -= 1;
    }
    if cut == b.len() || cut == 0 || !b[cut - 1].is_ascii_digit() {
        return None;
    }
    Some(format!("{}-{}", &key[..cut], &key[cut..]))
}

/// The key of a single-article text, written `unique` by the fonds and `1` by a
/// caller. Only valid **after** the literal key has failed: a text holding both
/// `1` and `unique` must answer on `1`.
pub fn sole_article_alias(key: &str) -> Option<&'static str> {
    (key == "1").then_some("unique")
}

/// The key left when a written number opens with the word it names: « Article
/// 1240 » folds to `article-1240`, and the article is `1240`. Tried only after
/// the literal key, 116 keys of the corpus really being `article-N`, and only
/// before a digit, `art-premier` naming no article of the fonds.
pub fn without_spoken_word(key: &str) -> Option<&str> {
    ["article-", "art-"]
        .iter()
        .find_map(|word| key.strip_prefix(word))
        .filter(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
}

/// The keys a reading tries: the literal one, then the catch-ups in the order
/// the literal key has to fail for them to be safe.
pub fn candidate_keys(key: &str) -> Vec<String> {
    let mut keys = vec![key.to_string()];
    keys.extend(letter_suffix_separated(key));
    keys.extend(without_spoken_word(key).map(str::to_string));
    keys.extend(sole_article_alias(key).map(str::to_string));
    keys
}

/// Forme d'affichage d'une clé publique à préfixe de partie simple :
/// `l761-1` → « L. 761-1 », `lo6213-1` → « LO. 6213-1 ». Hors de ce
/// sous-ensemble (numérique nu, annexes…), la clé se rend telle quelle. Le
/// libellé autoritaire reste `legal_article.num` quand on l'a : lui seul porte
/// l'astérisque de Légifrance.
pub fn display(key: &str) -> String {
    for prefix in ["lo", "lp", "l", "r", "d", "a"] {
        if let Some(tail) = key.strip_prefix(prefix) {
            if tail.starts_with(|c: char| c.is_ascii_digit()) {
                return format!("{}. {tail}", prefix.to_uppercase());
            }
        }
    }
    key.to_string()
}

/// Numéros d'article **cités dans une requête de recherche**, rendus en clés
/// [`article_key`] : « L. 423-7 ceseda parent » → `["l423-7"]`.
///
/// L'index stocke le numéro en **un seul token** (`L423-7` → `l423-7`), alors
/// que « L. 423-7 » se coupe en `l` (stopword) + `423-7`, qui ne matche pas.
///
/// Cinq formes reconnues :
///
/// - préfixe de partie **collé** au numéro — `L423-7`, `R*212-4` ;
/// - préfixe **détaché** — `L. 423-7`, `L 423-7`, `l.423-7` ;
/// - numéro **nu à segments** — `706-88`, `131-30-1` ;
/// - nombre **nu annoncé par « article »** — « article 21 loi 65-557 » cite
///   l'article 21 ; « 21 jours », lui, ne cite rien ;
/// - numéro **pointé annoncé** — « article 8.1 » cite `8-1`. Le point seul ne
///   suffit pas à annoncer une citation, contrairement au trait d'union : sans
///   le mot « article », `1.2` est un décimal bien plus souvent qu'un numéro.
///
/// Un préfixe détaché suivi d'un nombre **sans trait d'union** ne compte que
/// s'il est écrit en capitale : « L 21 » cite, « à 21 ans » ne cite pas (`à` se
/// plie en `a`, qui est un préfixe de partie).
///
/// The **ordinal** is part of it: « article 7 bis » cites `7-bis`, not `7`, two
/// distinct articles. Order of appearance kept, duplicates removed.
pub fn query_article_keys(query: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for (key, _, _) in key_spans(query) {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

/// Each article citation of `query`: its key, and the byte range it occupies,
/// the word announcing it included. Ranges are disjoint and increasing, which
/// is what lets [`strip_query_article_keys`] cut on them in one pass.
fn key_spans(query: &str) -> Vec<(String, usize, usize)> {
    let tokens = cited_tokens(query);
    let mut spans: Vec<(String, usize, usize)> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let (at, tok) = tokens[i];
        let digits = |t: &str| t.starts_with(|c: char| c.is_ascii_digit());
        let announced = i > 0
            && matches!(
                tokens[i - 1].1.to_lowercase().as_str(),
                "article" | "articles" | "art"
            );
        // Read before the branches below walk `i` onto the number.
        let start = match announced {
            true => tokens[i - 1].0,
            false => at,
        };
        let mut end = at + tok.len();
        let key = match part_prefix_len(tok) {
            // Glued prefix: `L423-7`, `R*212-4`.
            Some(n) if digits(&tok[n..]) => Some(article_key(tok)),
            // Prefix detached from the number that follows: `L. 423-7`, `l.423-7`.
            Some(n) if n == tok.len() => match tokens.get(i + 1) {
                Some((nat, next))
                    if digits(next)
                        && (next.contains('-') || tok.chars().any(|c| c.is_uppercase())) =>
                {
                    i += 1;
                    end = nat + next.len();
                    Some(article_key(&format!("{tok}{next}")))
                }
                _ => None,
            },
            _ if digits(tok) && (tok.contains('-') || announced) => Some(article_key(tok)),
            _ => None,
        };
        if let Some(mut key) = key {
            if let Some((oat, ord)) = tokens.get(i + 1).filter(|(_, t)| is_ordinal(t)) {
                key = article_key(&format!("{key} {ord}"));
                end = oat + ord.len();
                i += 1;
            }
            spans.push((key, start, end));
        }
        i += 1;
    }
    spans
}

/// What is left of `query` once every article citation is cut out, and the keys
/// cut. The rest is empty when the query is nothing but its citation, which is
/// what tells a reference from a question that cites one.
pub fn strip_query_article_keys(query: &str) -> (Vec<String>, String) {
    let mut keys: Vec<String> = Vec::new();
    let mut rest = String::new();
    let mut cursor = 0usize;
    for (key, start, end) in key_spans(query) {
        if !keys.contains(&key) {
            keys.push(key);
        }
        rest.push_str(&query[cursor..start.max(cursor)]);
        rest.push(' ');
        cursor = end.max(cursor);
    }
    rest.push_str(&query[cursor..]);
    (keys, rest.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// The article number a filter input names. [`query_article_keys`] holds a bare
/// number back because « 21 jours » cites nothing in a question; a field that
/// takes article filters and nothing else has no such reading, so « 700 » there
/// is article 700. Announcing the number rather than parsing it again is what
/// keeps ordinals and part prefixes on the one path.
pub fn filter_article_key(needle: &str) -> Option<String> {
    if let Some(key) = query_article_keys(needle).into_iter().next() {
        return Some(key);
    }
    let tokens = cited_tokens(needle);
    let i = tokens
        .iter()
        .position(|(_, t)| t.starts_with(|c: char| c.is_ascii_digit()))?;
    let rest: Vec<&str> = tokens[i..].iter().map(|(_, t)| *t).collect();
    query_article_keys(&format!("article {}", rest.join(" ")))
        .into_iter()
        .next()
}

/// Words a citation may carry between its numbers without asking anything.
const CITATION_CONNECTORS: [&str; 5] = ["et", "article", "articles", "art", "a"];

/// The keys of `text` when it is nothing but a citation: every article key it
/// carries, plus each bare integer of at least `min_digits` digits, joined by
/// connectors and nothing else. `None` as soon as a word remains.
fn citation_keys(text: &str, min_digits: usize) -> Option<Vec<String>> {
    let (mut keys, rest) = strip_query_article_keys(text);
    for (_, tok) in cited_tokens(&rest) {
        let folded = lj_fold::fold(tok);
        if CITATION_CONNECTORS.contains(&folded.as_str()) {
            continue;
        }
        if tok.len() >= min_digits && tok.bytes().all(|b| b.is_ascii_digit()) {
            let key = article_key(tok);
            if !keys.contains(&key) {
                keys.push(key);
            }
            continue;
        }
        return None;
    }
    (!keys.is_empty()).then_some(keys)
}

/// The keys of a query that cites and names nothing else: « L752-2 », « 1240 »,
/// « articles 1240 et 1241 ». A bare integer counts from three digits, alone in
/// the bar it names an article where « 21 » or « 65 » name a count or a year.
pub fn bare_citation(query: &str) -> Option<Vec<String>> {
    citation_keys(query, 3)
}

/// The articles `rest` designates when it is all that remains of a query whose
/// text has been named: « code civil 1240 » designates, « code civil 1240
/// responsabilité » asks. Any integer counts, the text being named.
pub fn designated_articles(rest: &str) -> Option<Vec<String>> {
    citation_keys(rest, 1).or_else(|| citation_keys(&format!("article {rest}"), 1))
}

/// The key a query cites and the tokens left to name its text. [`designated_article`]
/// read from the other end: it wants the name already cut, this one cuts the
/// number, so the rest can be confronted with a title instead of a catalogue.
pub fn cited_reference(query: &str) -> Option<(String, String)> {
    let named = |key: String, rest: &str| {
        let rest = rest.trim();
        (!rest.is_empty()).then(|| (key, rest.to_string()))
    };
    let (keys, rest) = strip_query_article_keys(query);
    // Two keys and the announced one is no longer the only candidate: « loi 89-462
    // article 15 » reads its instrument number as a key too.
    if keys.len() > 1 {
        return None;
    }
    if let Some(key) = keys.into_iter().next() {
        return named(key, &rest);
    }
    // In « code civil 1240 » the name that precedes the number is what announces
    // it, and no word does.
    let (head, tail) = query.trim().rsplit_once(char::is_whitespace)?;
    let key = query_article_keys(&format!("article {tail}"))
        .into_iter()
        .next()?;
    named(key, head)
}

/// Découpage d'une requête en tokens candidats à la citation : alphanumériques,
/// `-` et `*` de l'alphabet des clés, plus un `.` ou un `/` **encadré de
/// chiffres**, qui reste dans le token.
///
/// The dot cannot be kept unconditionally: « art. 1240 » would give the token `art.`,
/// and the detection of the announced article, which compares with `art`, would fail.
/// Framed by digits, it can only separate a number, as collective agreements (`8.1`)
/// and foreign codes (`286.2`) most often do.
fn cited_tokens(query: &str) -> Vec<(usize, &str)> {
    let bytes = query.as_bytes();
    let kept = |c: char| c.is_alphanumeric() || c == '-' || c == '*';
    let framed = |i: usize| {
        i > 0 && bytes[i - 1].is_ascii_digit() && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)
    };
    let mut tokens = Vec::new();
    let mut start = None;
    for (i, c) in query.char_indices() {
        if kept(c) || (matches!(c, '.' | '/') && framed(i)) {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            tokens.push((s, &query[s..i]));
        }
    }
    if let Some(s) = start {
        tokens.push((s, &query[s..]));
    }
    tokens
}

/// Ordinaux latins qui suffixent un numéro d'article, dans l'ordre où le
/// législateur les emploie.
const ORDINALS: [&str; 9] = [
    "bis",
    "ter",
    "quater",
    "quinquies",
    "sexies",
    "septies",
    "octies",
    "nonies",
    "decies",
];

fn is_ordinal(token: &str) -> bool {
    ORDINALS.contains(&token.to_lowercase().as_str())
}

/// Forme d'une clé où l'ordinal final est **détaché** : `7-bis` → « 7 bis ».
/// `None` si la clé n'en porte pas.
///
/// The corpus writes the ordinal apart: those numbers are stored « 7 bis », never
/// « 7-bis ». An index keeping the hyphen inside the token does not bring the two
/// together: the key as it stands finds nothing, the corpus's form is needed.
pub fn spaced_ordinal(key: &str) -> Option<String> {
    let (head, tail) = key.rsplit_once('-')?;
    is_ordinal(tail).then(|| format!("{head} {tail}"))
}

/// Forme d'une clé **tout en chiffres** où le séparateur est rendu au point :
/// `8-1` → « 8.1 ». `None` si la clé porte une lettre ou pas de séparateur.
///
/// Même raison que [`spaced_ordinal`] : la clé plie tout séparateur en `-`, le
/// corpus écrit `8.1`, et l'index tient ce numéro en un token — chercher `8-1`
/// n'y tombe pas. Depuis la clé on ne peut pas savoir laquelle des deux
/// graphies le corpus porte, donc on cherche les deux ; c'est une alternative de
/// plus dans un `should`, pas un filtre.
pub fn dotted_form(key: &str) -> Option<String> {
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let parts: Vec<&str> = key.split('-').collect();
    (parts.len() > 1 && parts.iter().all(|p| numeric(p))).then(|| parts.join("."))
}

/// A key whose part prefix is **detached**: `l761-2` → « l 761-2 ». `None` when
/// it carries none. Same reason as [`spaced_ordinal`]: the texts carrying the
/// legislative part of a code write « L. 1221-1 », two tokens in the index,
/// which the glued key never meets.
pub fn spaced_part(key: &str) -> Option<String> {
    GLUED_PREFIXES
        .iter()
        .filter(|p| {
            key.starts_with(**p) && key[p.len()..].starts_with(|c: char| c.is_ascii_digit())
        })
        .max_by_key(|p| p.len())
        .map(|p| format!("{p} {}", &key[p.len()..]))
}

/// Longueur du préfixe de partie en tête d'un token — `[*]*(l|lo|lp|r|d|a)[*]*`,
/// casse indifférente. `None` si le token n'en porte pas.
fn part_prefix_len(token: &str) -> Option<usize> {
    let low = token.to_lowercase();
    let stars = low.chars().take_while(|c| *c == '*').count();
    GLUED_PREFIXES
        .iter()
        .copied()
        .filter(|p| low[stars..].starts_with(p))
        .map(|p| {
            let end = stars + p.len();
            end + low[end..].chars().take_while(|c| *c == '*').count()
        })
        .max()
}

static INLINE_LABEL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    let latin = "(?:(?:un|duo|ter|quater|quin|sex|sept|octo|novo)?(?:decies|vicies)|tricies\
                 |bis|ter|quater|quinquies|sexies|septies|octies|nonies)";
    regex::Regex::new(&format!(
        r"(?i)^\s*(?:art\.|article)\s+(\d+(?:er|re|ère|e|r)?{latin}?(?-i:[A-Z]{{0,3}})(?:[-.]\d+)*(?:\s+(?:{latin}(?-i:[A-Z]{{0,3}})|(?-i:[A-Z]{{1,3}})|[a-z])){{0,4}}|premier|unique)(?:\s*\((?:nouveau|ex-?\s*\d+|\d+)\))?\s*\.{{0,2}}\s*(?:[-:\x{{2013}}\x{{2014}}]|\n|$)",
    ))
    .expect("inline label")
});

/// The number an article prints at the head of its own text, `Art. 7. - …` or
/// `Article 1er` on its line, which the old JO stock leaves out of `NUM`.
pub fn inline_num(texte: &str) -> Option<String> {
    INLINE_LABEL.captures(texte).map(|c| {
        let num = c[1].split_whitespace().collect::<Vec<_>>().join(" ");
        match num.strip_suffix('r') {
            Some(n) if n.bytes().all(|b| b.is_ascii_digit()) => format!("{n}er"),
            _ => num,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_article_label_printed_inline_is_its_number() {
        assert_eq!(
            inline_num("Art. 7. - Le directeur est chargé").as_deref(),
            Some("7")
        );
        assert_eq!(
            inline_num("Article 1er\nLe présent arrêté").as_deref(),
            Some("1er")
        );
        assert_eq!(
            inline_num("Art. 4 bis. - Les mots").as_deref(),
            Some("4 bis")
        );
        assert_eq!(
            inline_num("Art. 5-1. - Il est inséré").as_deref(),
            Some("5-1")
        );
        assert_eq!(
            inline_num("Article unique. - La loi").as_deref(),
            Some("unique")
        );
        assert_eq!(
            inline_num("Article 1609 nonies B\nCet article est complété").as_deref(),
            Some("1609 nonies B")
        );
        assert_eq!(
            inline_num("Article 93 H ter\nAu 3o, après les mots").as_deref(),
            Some("93 H ter")
        );
        assert_eq!(
            inline_num("Article 164 F unvicies A\nLes termes").as_deref(),
            Some("164 F unvicies A")
        );
        assert_eq!(
            inline_num("Article 199 terdecies\nLe III est ainsi modifié").as_deref(),
            Some("199 terdecies")
        );
        assert_eq!(
            inline_num("Art. 2.. - Les dispositions du présent arrêté").as_deref(),
            Some("2")
        );
        assert_eq!(
            identity_key(&inline_num("Art. 1r. - Il est créé une zone").unwrap()),
            "1"
        );
        assert_eq!(
            inline_num("Article 1679bisB:\nAu 3, l'article").as_deref(),
            Some("1679bisB")
        );
        assert_eq!(
            inline_num("Article 20 (ex-19)\n\n1. Lorsque l'assuré").as_deref(),
            Some("20")
        );
        assert_eq!(
            inline_num("Article 32 ( 3)\nLe texte du paragraphe 3"),
            None
        );
        assert_eq!(
            inline_num("Article 3 du décret du 2 mai 1990 est abrogé."),
            None
        );
        assert_eq!(inline_num("Article 3 et 4 sont abrogés."), None);
        assert_eq!(inline_num("Art. L. 123-4. - Le maire"), None);
        assert_eq!(inline_num("« Art. 2. - En cas d'absence"), None);
        assert_eq!(inline_num("Art. 1l. - Toute infraction"), None);
        assert_eq!(
            identity_key(&inline_num("Article premier - X").unwrap()),
            "1"
        );
    }

    /// A model writes the word, the corpus sometimes carries it: the fallback
    /// only opens on a digit, and gives back nothing else.
    #[test]
    fn le_mot_article_tombe_devant_un_numero() {
        assert_eq!(without_spoken_word("article-1240"), Some("1240"));
        assert_eq!(without_spoken_word("art-l761-1"), None);
        assert_eq!(without_spoken_word("art-706-88"), Some("706-88"));
        assert_eq!(without_spoken_word("1240"), None);
        assert_eq!(without_spoken_word("article-annexe"), None);
        assert_eq!(article_key("Article 1240"), "article-1240");
    }

    #[test]
    fn display_inverse_les_prefixes_simples() {
        assert_eq!(display("l761-1"), "L. 761-1");
        assert_eq!(display("lo6213-1"), "LO. 6213-1");
        assert_eq!(display("r*212-4"), "r*212-4");
        assert_eq!(display("1240"), "1240");
        assert_eq!(display("106-bis"), "106-bis");
        assert_eq!(display("annexe-1"), "annexe-1");
    }

    #[test]
    fn prefixe_de_partie_colle_au_numero() {
        assert_eq!(article_key("L. 761-1"), "l761-1");
        assert_eq!(article_key("L761-1"), "l761-1");
        assert_eq!(article_key("l. 761-1"), "l761-1");
        assert_eq!(article_key("L 761-1"), "l761-1");
        assert_eq!(article_key("R. 313-4"), "r313-4");
        assert_eq!(article_key("LO. 6213-1"), "lo6213-1");
        assert_eq!(article_key("1240"), "1240");
    }

    #[test]
    fn parentheses_conservees() {
        assert_eq!(article_key("(1)"), "(1)");
    }

    /// L'étoile de Légifrance sort de la clé, où qu'elle soit et quel qu'en
    /// soit le nombre : elle ne distingue jamais deux articles applicables le
    /// même jour, et la forme citée ne la porte pas.
    #[test]
    fn l_etoile_tombe_et_la_forme_citee_rejoint_la_forme_officielle() {
        assert_eq!(article_key("R*212-4"), "r212-4");
        assert_eq!(article_key("R. 212-4"), "r212-4");
        assert_eq!(article_key("R**119-5"), "r119-5");
        assert_eq!(article_key("**R. 111-1"), "r111-1");
        assert_eq!(article_key("*R441-22"), "r441-22");
        assert_eq!(article_key("ANNEXE*"), "annexe");
        assert_eq!(article_key("10-**perime**"), "10-perime");
        // Effacée, pas pliée en séparateur : au milieu d'une clé, le pliage
        // laisserait un tiret que personne ne cite.
        assert_eq!(
            article_key("Annexe à l'article R*351-1 art. 1"),
            "annexe-a-l-article-r351-1-art-1"
        );
    }

    #[test]
    fn separateurs_plies_en_tiret() {
        assert_eq!(article_key("106 Bis"), "106-bis");
        assert_eq!(article_key("11 BIS"), "11-bis");
        assert_eq!(article_key("100/1"), "100-1");
        assert_eq!(article_key("Annexe 1"), "annexe-1");
        assert_eq!(article_key("annexe I"), "annexe-i");
        assert_eq!(article_key("ANNEXE, 2"), "annexe-2");
        assert_eq!(article_key("Préambule."), "preambule");
    }

    #[test]
    fn le_suffixe_de_lettres_colle_se_resepare() {
        assert_eq!(letter_suffix_separated("l80a").as_deref(), Some("l80-a"));
        assert_eq!(letter_suffix_separated("l64a").as_deref(), Some("l64-a"));
        assert_eq!(
            letter_suffix_separated("119bis").as_deref(),
            Some("119-bis")
        );
        assert_eq!(letter_suffix_separated("7ter").as_deref(), Some("7-ter"));
    }

    #[test]
    fn la_reseparation_ne_touche_pas_la_frontiere_chiffre_chiffre() {
        // `l312-12` et `l312-1-2` sont deux articles : la règle large les confondrait.
        assert_eq!(letter_suffix_separated("l312-12"), None);
        assert_eq!(letter_suffix_separated("l761-1"), None);
        assert_eq!(letter_suffix_separated("lo6213-1"), None);
    }

    #[test]
    fn la_reseparation_est_sans_effet_sur_une_cle_deja_separee() {
        assert_eq!(letter_suffix_separated("l80-a"), None);
        assert_eq!(letter_suffix_separated("119-bis"), None);
        assert_eq!(letter_suffix_separated("annexe"), None);
        assert_eq!(letter_suffix_separated(""), None);
    }

    #[test]
    fn identity_preserve_les_discriminants_que_la_citation_tronque() {
        assert_eq!(identity_key("199 duovicies"), "199-duovicies");
        assert_eq!(identity_key("1609 quatertricies"), "1609-quatertricies");
        assert_eq!(identity_key("46 quater-0 W"), "46-quater-0-w");
        assert_eq!(identity_key("41 duovicies-0 H bis"), "41-duovicies-0-h-bis");
        assert_eq!(identity_key("1.01"), "1-01");
        assert_eq!(identity_key("213-6.02 bis"), "213-6-02-bis");
        assert_eq!(identity_key("221-II-1/13"), "221-ii-1-13");
        assert_eq!(identity_key("A80 CB-3-1"), "a80-cb-3-1");
    }

    #[test]
    fn identity_plie_le_marqueur_ordinal_de_tete() {
        assert_eq!(identity_key("1er"), "1");
        assert_eq!(identity_key("1ER"), "1");
        assert_eq!(identity_key("1ère"), "1");
        assert_eq!(identity_key("premier"), "1");
        assert_eq!(identity_key("PREMIER"), "1");
        assert_eq!(identity_key("1er bis"), "1-bis");
        assert_eq!(identity_key("1er-1"), "1-1");
        assert_eq!(identity_key("1er (1)"), "1-(1)");
        // Lettre de division ≠ marqueur ordinal.
        assert_eq!(identity_key("124 E"), "124-e");
        assert_eq!(identity_key("2e"), "2e");
        // Marqueur suivi d'alphanumérique = pas un marqueur.
        assert_eq!(identity_key("1ers"), "1ers");
    }

    #[test]
    fn la_piece_qualifie_la_cle_sans_la_deformer() {
        assert_eq!(
            piece_key("KALITEXT000005675711", &identity_key("2")),
            "kalitext000005675711-2"
        );
        assert_eq!(
            piece_key("KALITEXT000005675425", &identity_key("1er bis")),
            "kalitext000005675425-1-bis"
        );
        // Point fixe : un rebasage se rejoue sans redéformer la clé.
        for piece in ["KALITEXT000005675711", "KALITEXT000024359407"] {
            for num in ["2", "L. 761-1", "Annexe 1", "8.1"] {
                let key = piece_key(piece, &identity_key(num));
                assert_eq!(article_key(&key), key, "piece={piece} num={num}");
            }
        }
        // Deux pièces du même conteneur ne se confondent plus sur le même numéro.
        assert_ne!(
            piece_key("KALITEXT000005675711", "2"),
            piece_key("KALITEXT000005675733", "2")
        );
    }

    #[test]
    fn identity_point_fixe() {
        for raw in [
            "199 duovicies",
            "46 quater-0 W",
            "1.01",
            "1er bis",
            "premier",
            "L. 761-1",
            "Annexe 1",
        ] {
            let key = identity_key(raw);
            assert_eq!(identity_key(&key), key, "raw={raw}");
        }
    }

    #[test]
    fn point_fixe() {
        for raw in ["L. 761-1", "R*212-4", "106 Bis", "(1)", "Annexe 1"] {
            let key = article_key(raw);
            assert_eq!(article_key(&key), key);
        }
    }

    #[test]
    fn requete_les_formes_citees_tombent_sur_la_cle() {
        for q in ["L423-7 ceseda", "L. 423-7 ceseda", "l.423-7 ceseda"] {
            assert_eq!(query_article_keys(q), ["l423-7"]);
        }
        assert_eq!(
            query_article_keys("\"L. 433-3-1\" titre de séjour pluriannuel"),
            ["l433-3-1"]
        );
        // Étoile tapée ou non, la requête tombe sur la même clé qu'en base.
        for q in [
            "R*212-4 permis de construire",
            "R. 212-4 permis de construire",
        ] {
            assert_eq!(query_article_keys(q), ["r212-4"]);
        }
        assert_eq!(
            query_article_keys("LO 6213-1 Saint-Barthélemy"),
            ["lo6213-1"]
        );
        assert_eq!(query_article_keys("garde à vue 706-88 du CPP"), ["706-88"]);
    }

    #[test]
    fn requete_le_numero_pointe_annonce_tombe_sur_la_cle() {
        // Convention collective, code étranger : le point sépare le numéro.
        assert_eq!(
            query_article_keys("article 8.1 convention collective du bâtiment"),
            ["8-1"]
        );
        assert_eq!(
            query_article_keys("art 286.2 code procédure civile"),
            ["286-2"]
        );
        // Le point détaché du préfixe garde son chemin : ni `art.` ni `l.` ne
        // sont encadrés de chiffres.
        assert_eq!(query_article_keys("art. 1240 code civil"), ["1240"]);
        assert_eq!(query_article_keys("l.423-7 ceseda"), ["l423-7"]);
        // Sans annonce, un décimal reste un décimal.
        assert!(query_article_keys("taux de 1.2 % sur douze mois").is_empty());
        // Un numéro d'instrument n'est pas un numéro d'article.
        assert!(query_article_keys("directive 2008/115 retour").is_empty());
    }

    #[test]
    fn la_graphie_pointee_du_corpus_se_reconstruit_depuis_la_cle() {
        assert_eq!(dotted_form("8-1").as_deref(), Some("8.1"));
        assert_eq!(dotted_form("286-2").as_deref(), Some("286.2"));
        assert_eq!(dotted_form("3-181").as_deref(), Some("3.181"));
        // Une lettre ou pas de séparateur : rien à reconstruire.
        assert_eq!(dotted_form("l423-7"), None);
        assert_eq!(dotted_form("7-bis"), None);
        assert_eq!(dotted_form("1240"), None);
    }

    /// In a field that takes article filters and nothing else, a bare number is
    /// one, ordinal and part prefix included.
    #[test]
    fn le_filtre_lit_un_nombre_nu_comme_un_article() {
        let k = |s: &str| filter_article_key(s);
        assert_eq!(k("700").as_deref(), Some("700"));
        assert_eq!(k("700 bis").as_deref(), Some("700-bis"));
        assert_eq!(k("code civil 1240").as_deref(), Some("1240"));
        assert_eq!(
            k("L761-2 du code de la consommation").as_deref(),
            Some("l761-2")
        );
        assert_eq!(k("L. 761-2 conso").as_deref(), Some("l761-2"));
        assert_eq!(k("code de la consommation"), None);
    }

    #[test]
    fn la_graphie_detachee_du_prefixe_se_reconstruit_depuis_la_cle() {
        assert_eq!(spaced_part("l761-2").as_deref(), Some("l 761-2"));
        assert_eq!(spaced_part("lo132-1").as_deref(), Some("lo 132-1"));
        assert_eq!(spaced_part("a402-1").as_deref(), Some("a 402-1"));
        // No prefix, or no digit behind it: nothing to detach.
        assert_eq!(spaced_part("1240"), None);
        assert_eq!(spaced_part("annexe-1"), None);
        assert_eq!(spaced_part("d-1"), None);
    }

    #[test]
    fn requete_un_nombre_nu_ne_cite_que_derriere_article() {
        assert_eq!(
            query_article_keys("article 21 loi 65-557 avis du conseil syndical sous 21 jours"),
            ["21", "65-557"]
        );
        assert!(query_article_keys("délai de 21 jours pour convoquer").is_empty());
        // « à » se plie en « a », préfixe de partie — sans capitale, ça ne cite rien.
        assert!(query_article_keys("retraite a 62 ans").is_empty());
        assert!(query_article_keys("le contrat de travail").is_empty());
    }

    #[test]
    fn le_retrait_emporte_la_citation_et_le_mot_qui_l_annonce() {
        let (keys, rest) = strip_query_article_keys("responsabilité article 1240 du code civil");
        assert_eq!(keys, ["1240"]);
        assert_eq!(rest, "responsabilité du code civil");
        let (keys, rest) = strip_query_article_keys("bail commercial L145-40 renouvellement");
        assert_eq!(keys, ["l145-40"]);
        assert_eq!(rest, "bail commercial renouvellement");
        let (keys, rest) = strip_query_article_keys("clause pénale");
        assert!(keys.is_empty());
        assert_eq!(rest, "clause pénale");
    }

    #[test]
    fn le_reste_nu_designe_des_articles_la_question_non() {
        let keys = |s: &str| designated_articles(s).map(|k| k.join(","));
        // What is left of "code civil 1240" once the named text is taken out.
        assert_eq!(keys("1240"), Some("1240".into()));
        assert_eq!(keys("L145-40"), Some("l145-40".into()));
        assert_eq!(keys("article 700"), Some("700".into()));
        assert_eq!(keys("7 bis"), Some("7-bis".into()));
        assert_eq!(keys("12"), Some("12".into()));
        assert_eq!(keys("articles 1240 et 1241"), Some("1240,1241".into()));
        assert_eq!(keys("1240, 1241"), Some("1240,1241".into()));
        // A remainder carrying anything but the numbers is a question, not a key.
        assert_eq!(keys("1240 responsabilité"), None);
        assert_eq!(keys("clause pénale"), None);
        assert_eq!(keys(""), None);
    }

    #[test]
    fn une_cle_nue_declare_sa_reference_un_entier_court_non() {
        let keys = |s: &str| bare_citation(s).map(|k| k.join(","));
        assert_eq!(keys("L752-2"), Some("l752-2".into()));
        assert_eq!(keys("L. 752-2"), Some("l752-2".into()));
        assert_eq!(keys("1240"), Some("1240".into()));
        assert_eq!(keys("article 1240"), Some("1240".into()));
        assert_eq!(keys("articles 1240 et 1241"), Some("1240,1241".into()));
        assert_eq!(keys("706-88"), Some("706-88".into()));
        // Two digits alone are a count or a day; a year is not an article either
        // once a word sits next to it.
        assert_eq!(keys("21"), None);
        assert_eq!(keys("2024 expulsion"), None);
        assert_eq!(keys("L752-2 licenciement"), None);
        assert_eq!(keys("code civil 1240"), None);
        assert_eq!(keys(""), None);
    }

    #[test]
    fn requete_l_ordinal_appartient_au_numero() {
        // « 7 » and « 7 bis » are two articles.
        assert_eq!(
            query_article_keys("article 7 bis accord franco-algérien 1968 certificat de résidence"),
            ["7-bis"]
        );
        assert_eq!(query_article_keys("L. 123-4 ter du code"), ["l123-4-ter"]);
        assert_eq!(query_article_keys("article 40 quinquies"), ["40-quinquies"]);
        // Un mot qui n'est pas un ordinal ne s'agrège pas.
        assert_eq!(query_article_keys("article 7 accord franco"), ["7"]);
    }

    #[test]
    fn ordinal_detache_pour_la_forme_du_corpus() {
        assert_eq!(spaced_ordinal("7-bis").as_deref(), Some("7 bis"));
        assert_eq!(spaced_ordinal("l123-4-ter").as_deref(), Some("l123-4 ter"));
        assert_eq!(spaced_ordinal("l423-7"), None);
        assert_eq!(spaced_ordinal("706-88"), None);
    }

    #[test]
    fn citation_coupee_du_nom_qui_la_porte() {
        use super::cited_reference;
        assert_eq!(
            cited_reference("code civil 1240"),
            Some(("1240".into(), "code civil".into()))
        );
        assert_eq!(
            cited_reference("article L. 1221-1 du code du travail"),
            Some(("l1221-1".into(), "du code du travail".into()))
        );
        // Nothing names a text, so there is no reference to confront with one.
        assert_eq!(cited_reference("article 1240"), None);
        assert_eq!(cited_reference("clause pénale"), None);
        // An instrument number reads as a key too, so the announced one is no
        // longer the only candidate and the citation stops being one.
        assert_eq!(cited_reference("loi 89-462 article 15"), None);
        // A second number stays in the rest, where no title carries it.
        assert_eq!(
            cited_reference("articles 1240 et 1241 du code civil"),
            Some(("1240".into(), "et 1241 du code civil".into()))
        );
    }
}
