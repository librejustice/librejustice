//! Détection de mode, traduction booléenne et construction de la query
//! phrase-combo body (gazetteer + chunks).

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use lj_textsearch::body_tok::{is_stopword, tokenize as tokenize_body};
use regex::Regex;

use lj_dtos::QueryMode;

/// `\b(?:ET|OU|SAUF|AND|OR|NOT)\b|\b(?:PROCHE|NEAR)\d+\b|["*]` — détecteur de
/// query booléenne.
static BOOL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\b(?:ET|OU|SAUF|AND|OR|NOT)\b|\b(?:PROCHE|NEAR)\d+\b|[*"]"#).unwrap()
});

// ── Détection de mode / traduction booléenne ─────────────────────────────────

/// `lexical` when the query carries an operator or nothing outside its quotes:
/// a quote beside free words constrains the hybrid instead.
pub fn detect_query_mode(query: &str) -> QueryMode {
    let quoted_only = query.contains('"') && query_lacks_searchable_terms(&unquoted(query));
    if has_boolean_operator(query) || quoted_only {
        QueryMode::Lexical
    } else {
        QueryMode::Hybrid
    }
}

pub(crate) fn is_boolean_query(query: &str) -> bool {
    BOOL_RE.is_match(query)
}

/// `BOOL_RE` minus the quote: what the caller can only have typed on purpose.
/// A quote is how a docket number is usually written, an operator never is.
static OPERATOR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:ET|OU|SAUF|AND|OR|NOT)\b|\b(?:PROCHE|NEAR)\d+\b|\*").unwrap()
});

pub(crate) fn has_boolean_operator(query: &str) -> bool {
    OPERATOR_RE.is_match(query)
}

/// Collapse les guillemets doublés `""x""` → `"x"` : deux quotes de chaque
/// côté, pas trois.
static DOUBLE_QUOTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("\"\"([^\"]*?)\"\"").unwrap());
static ET_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bET\b").unwrap());
static OU_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bOU\b").unwrap());
static SAUF_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bSAUF\b").unwrap());
/// Un token = phrase quotée (slop `~N` collé compris), ou token nu.
static TOKEN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""[^"]*"(?:~\d+)?|\S+"#).unwrap());

/// Découpage pour la passe de proximité : phrase quotée, opérateur
/// `PROCHE`/`NEAR` suivi de son entier, ou token nu. L'opérateur passe avant
/// `\S+` pour être reconnu même collé à une phrase (`"a b"PROCHE3`).
static PROX_SCAN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""[^"]*"(?:~\d+)?|\b(?:PROCHE|NEAR)\d+\b|\S+"#).unwrap());
/// A quoted phrase with its slop, or a run holding neither space nor quote: a
/// parenthesis glued to a phrase comes apart from it.
static GROUP_SCAN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""[^"]*"(?:~\d+)?|[^\s"]+"#).unwrap());
/// Token qui *est* l'opérateur de proximité, avec son slop en capture.
static PROX_OP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:PROCHE|NEAR)(\d+)$").unwrap());

pub(crate) fn translate_boolean(query: &str) -> String {
    let q = DOUBLE_QUOTE_RE.replace_all(query, r#""$1""#).into_owned();
    let q = split_groups(&q);
    let q = rewrite_proximity(&q);
    let q = ET_RE.replace_all(&q, "AND").into_owned();
    let q = OU_RE.replace_all(&q, "OR").into_owned();
    let q = SAUF_RE.replace_all(&q, "NOT").into_owned();
    enforce_and_precedence(&q)
}

/// Sets apart as tokens of their own the `(` opening a token and the `)`
/// closing one, and drops the unmatched and the empty groups:
/// `("bail commercial" OU loyer)` becomes `( "bail commercial" OU loyer )`. A
/// parenthesis inside a word (`acte(s`) stays its text.
fn split_groups(query: &str) -> String {
    let mut toks: Vec<&str> = Vec::new();
    for m in GROUP_SCAN_RE.find_iter(query) {
        let mut tok = m.as_str();
        if tok.starts_with('"') {
            toks.push(tok);
            continue;
        }
        while let Some(rest) = tok.strip_prefix('(') {
            toks.push("(");
            tok = rest;
        }
        let mut closers = 0;
        while tok.ends_with(')') && tok.matches(')').count() > tok.matches('(').count() {
            tok = &tok[..tok.len() - 1];
            closers += 1;
        }
        if !tok.is_empty() {
            toks.push(tok);
        }
        toks.extend(std::iter::repeat_n(")", closers));
    }
    let mut out: Vec<&str> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    for tok in toks {
        match tok {
            "(" => {
                open.push(out.len());
                out.push(tok);
            }
            ")" => match open.pop() {
                Some(at) if at + 1 == out.len() => {
                    out.pop();
                }
                Some(_) => out.push(tok),
                None => {}
            },
            _ => out.push(tok),
        }
    }
    for at in open.into_iter().rev() {
        out.remove(at);
    }
    out.join(" ")
}

/// Slop d'un token opérateur (`PROCHE5` → 5), `None` si ce n'en est pas un.
fn proximity_slop(tok: &str) -> Option<u32> {
    PROX_OP_RE.captures(tok)?.get(1)?.as_str().parse().ok()
}

/// A proximity operand: no boolean keyword, no proximity operator, no parenthesis.
fn is_proximity_operand(tok: &str) -> bool {
    !matches!(tok, "ET" | "OU" | "SAUF" | "AND" | "OR" | "NOT" | "(" | ")")
        && proximity_slop(tok).is_none()
}

/// The words a proximity phrase gathers, with the widest slop its operands carry.
type Leg = (Vec<String>, u32);

/// The words of an operand, a quoted phrase flattened into its own, with the
/// slop it carries: the proximity phrase quotes them all again.
fn operand_leg(tok: &str) -> Leg {
    let (body, slop) = match tok.rfind('"') {
        Some(end) => (&tok[..end], tok[end + 1..].strip_prefix('~')),
        None => (tok, None),
    };
    let words = body
        .replace('"', " ")
        .split_whitespace()
        .map(str::to_string)
        .collect();
    (words, slop.and_then(|n| n.parse().ok()).unwrap_or(0))
}

/// The leg of each `OU` a group stands for, the `ET` of a leg flattened into
/// it. `None` when the group nests another, or holds a `SAUF` or a proximity.
fn group_legs<S: AsRef<str>>(inner: &[S]) -> Option<Vec<Leg>> {
    let mut legs: Vec<Leg> = vec![(Vec::new(), 0)];
    for tok in inner.iter().map(AsRef::as_ref) {
        match tok {
            "OU" | "OR" => legs.push((Vec::new(), 0)),
            "ET" | "AND" => {}
            t if !is_proximity_operand(t) => return None,
            t => {
                let (words, slop) = operand_leg(t);
                let leg = legs.last_mut()?;
                leg.0.extend(words);
                leg.1 = leg.1.max(slop);
            }
        }
    }
    legs.iter()
        .all(|(words, _)| !words.is_empty())
        .then_some(legs)
}

/// Index of the parenthesis that closes the group opened by the first token.
fn group_end<'a>(
    toks: impl Iterator<Item = (usize, &'a str)>,
    open: &str,
    close: &str,
) -> Option<usize> {
    let mut depth = 0usize;
    for (i, t) in toks {
        if t == open {
            depth += 1;
        } else if t == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// The operand ending `out`, a word, a phrase or a group, with the index it
/// starts at.
fn left_operand(out: &[String]) -> Option<(usize, Vec<Leg>)> {
    let last = out.last()?;
    if last != ")" {
        return is_proximity_operand(last).then(|| (out.len() - 1, vec![operand_leg(last)]));
    }
    let open = group_end(out.iter().map(String::as_str).enumerate().rev(), ")", "(")?;
    Some((open, group_legs(&out[open + 1..out.len() - 1])?))
}

/// The operand starting at `toks[at]`, with the index right after it.
fn right_operand(toks: &[&str], at: usize) -> Option<(usize, Vec<Leg>)> {
    let first = *toks.get(at)?;
    if first != "(" {
        return is_proximity_operand(first).then(|| (at + 1, vec![operand_leg(first)]));
    }
    let close = group_end(toks.iter().copied().enumerate().skip(at), "(", ")")?;
    Some((close + 1, group_legs(&toks[at + 1..close])?))
}

/// Rewrites the proximity chains `A PROCHE5 B [PROCHE3 C…]` into the tantivy
/// slop phrase `"A B C"~5` (the widest N of the chain), the only construct of
/// the `paradedb.parse` grammar that expresses proximity. Words and phrases
/// are flattened into it, and the slop forgives the order (`"commercial bail"~5`
/// matches « bail commercial »). A group is distributed, one phrase per leg:
/// `bail PROCHE5 (commercial OU rural)` → `( "bail commercial"~5 OR "bail rural"~5 )`.
///
/// An operator missing an operand on either side (`PROCHE5 bail`,
/// `"bail commercial"PROCHE5`, `bail ET PROCHE5`), or whose group does not
/// distribute, is dropped: nothing to bring together.
fn rewrite_proximity(query: &str) -> String {
    if !PROX_SCAN_RE
        .find_iter(query)
        .any(|m| proximity_slop(m.as_str()).is_some())
    {
        return query.to_string();
    }
    let toks: Vec<&str> = PROX_SCAN_RE.find_iter(query).map(|m| m.as_str()).collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let Some(mut slop) = proximity_slop(toks[i]) else {
            out.push(toks[i].to_string());
            i += 1;
            continue;
        };
        let (Some((start, mut legs)), Some((end, right))) =
            (left_operand(&out), right_operand(&toks, i + 1))
        else {
            i += 1;
            continue;
        };
        out.truncate(start);
        legs = cross(&legs, &right);
        i = end;
        // Chaîne : `A PROCHE5 B PROCHE3 C` → une seule phrase, slop = max.
        while let Some(next_slop) = toks.get(i).and_then(|t| proximity_slop(t)) {
            let Some((end, right)) = right_operand(&toks, i + 1) else {
                break;
            };
            slop = slop.max(next_slop);
            legs = cross(&legs, &right);
            i = end;
        }
        let phrases = legs
            .iter()
            .map(|(words, own)| format!("\"{}\"~{}", words.join(" "), slop.max(*own)));
        match legs.len() {
            1 => out.extend(phrases),
            _ => {
                out.push("(".to_string());
                for (n, phrase) in phrases.enumerate() {
                    if n > 0 {
                        out.push("OR".to_string());
                    }
                    out.push(phrase);
                }
                out.push(")".to_string());
            }
        }
    }
    out.join(" ")
}

/// Every leg of `left` followed by every leg of `right`.
fn cross(left: &[Leg], right: &[Leg]) -> Vec<Leg> {
    left.iter()
        .flat_map(|(l, ls)| {
            right
                .iter()
                .map(move |(r, rs)| ([l.as_slice(), r].concat(), *ls.max(rs)))
        })
        .collect()
}

/// Caractères que la grammaire tantivy interprète hors guillemets : un token nu
/// qui en porte un (`d'acte`, `acte(s`) fait rejeter la query entière par
/// `paradedb.parse` — l'apostrophe ouvre une phrase single-quote jamais fermée.
/// `' ( ) [ { : ^` cassent tous le parse ; `~` (slop) et `*` (wildcard) parsent
/// et restent porteurs de sens.
const TANTIVY_SYNTAX: &[char] = &['\'', '(', ')', '[', ']', '{', '}', ':', '^', '\\'];

/// Token composé uniquement d'étoiles : un wildcard sans préfixe n'est pas une
/// prefix-query — `+*` fait rejeter la query entière par `paradedb.parse`.
/// Droppé comme un stopword.
fn is_bare_star(tok: &str) -> bool {
    !tok.is_empty() && tok.chars().all(|c| c == '*')
}

/// Re-sert un token nu porteur de syntaxe tantivy en phrase double-quotée :
/// l'index la re-tokenise comme le texte source (l'élision `d'acte` se splitte
/// à l'identique des deux côtés), le match est préservé. Les tokens déjà
/// quotés et les wildcards passent tels quels.
fn quote_bare_syntax_token(tok: &str, syntax: &[char]) -> String {
    if tok.starts_with('"') || tok.contains('*') || !tok.contains(syntax) {
        tok.to_string()
    } else {
        format!("\"{}\"", tok.replace('"', " "))
    }
}

/// Force la précédence AND (Tantivy par défaut OR) en préfixant `+`/`-` ;
/// strippe les stopwords FR qui n'existent pas dans l'index.
fn enforce_and_precedence(query: &str) -> String {
    if query.contains(" OR ") {
        // `split_groups` left each grouping parenthesis a token of its own.
        let toks = TOKEN_RE
            .find_iter(query)
            .filter(|m| !is_bare_star(m.as_str().trim_start_matches(['+', '-'])))
            .map(|m| {
                let tok = m.as_str();
                match tok.as_bytes().first() {
                    _ if tok == "(" || tok == ")" => tok.to_string(),
                    Some(b'+') | Some(b'-') => {
                        let (prefix, body) = tok.split_at(1);
                        format!("{prefix}{}", quote_bare_syntax_token(body, TANTIVY_SYNTAX))
                    }
                    _ => quote_bare_syntax_token(tok, TANTIVY_SYNTAX),
                }
            });
        // Le drop d'une étoile nue peut rendre un opérateur pendant
        // (« congés OR » / « OR congés ») — un parse error tantivy.
        let is_op = |t: &str| matches!(t, "AND" | "OR" | "NOT");
        let mut out: Vec<String> = Vec::new();
        for t in toks {
            if is_op(&t) && out.last().is_none_or(|p| is_op(p) || p == "(") {
                continue;
            }
            if t == ")" {
                while out.last().is_some_and(|p| is_op(p)) {
                    out.pop();
                }
                if out.last().is_some_and(|p| p == "(") {
                    out.pop();
                    continue;
                }
            }
            out.push(t);
        }
        while out.last().is_some_and(|t| is_op(t)) {
            out.pop();
        }
        return out.join(" ");
    }
    let mut out: Vec<String> = Vec::new();
    let mut next_neg = false;
    for m in TOKEN_RE.find_iter(query) {
        let tok = m.as_str();
        if tok == "AND" {
            continue;
        }
        if tok == "NOT" {
            next_neg = true;
            continue;
        }
        if tok == "(" {
            out.push(format!("{}(", if next_neg { '-' } else { '+' }));
            next_neg = false;
            continue;
        }
        if tok == ")" {
            match out.last().map(String::as_str) {
                Some("+(" | "-(") => {
                    out.pop();
                }
                _ => out.push(tok.to_string()),
            }
            continue;
        }
        if is_bare_star(tok.trim_start_matches(['+', '-'])) {
            next_neg = false;
            continue;
        }
        if tok.starts_with('+') || tok.starts_with('-') {
            let (prefix, body) = tok.split_at(1);
            out.push(format!(
                "{prefix}{}",
                quote_bare_syntax_token(body, TANTIVY_SYNTAX)
            ));
        } else if is_stopword(&lj_fold::fold_chars(tok)) {
            next_neg = false;
            continue;
        } else if next_neg {
            out.push(format!("-{}", quote_bare_syntax_token(tok, TANTIVY_SYNTAX)));
        } else {
            out.push(format!("+{}", quote_bare_syntax_token(tok, TANTIVY_SYNTAX)));
        }
        next_neg = false;
    }
    out.join(" ")
}

// ── Phrase-combo body (gazetteer + chunks) ───────────────────────────────────

/// Génère la query `paradedb.parse` pour la jambe BM25 body (chemin prod =
/// [`BodyArm::Split`]).
pub(crate) fn phrase_combo_parse(query: &str) -> String {
    body_query_for_arm(query, BodyArm::Split)
}

/// Bras de construction de la jambe body BM25, pour l'A/B de ranking offline.
/// `Split` est le chemin prod : sac OR + phrase entière +
/// runs de contenu découpés aux stopwords (le gazetteer reste hors chemin).
/// `Bag` ne garde que le sac OR (aucune clause phrase). `Weighted(w)` part de
/// `Split` et pondère chaque clause phrase par `^w` (`w < 1` ⇒
/// sous-pondération).
///
/// `Forms(f)` is `Split` with each word of the bag joined by the forms `f` gives
/// it; `RequiredForms(f)` requires each content word or one of its forms, as the
/// lexical mode requires every word.
#[derive(Debug, Clone, Copy)]
pub enum BodyArm {
    Bag,
    Split,
    Weighted(f64),
    Forms(fn(&str) -> Vec<String>),
    RequiredForms(fn(&str) -> Vec<String>),
}

/// Query body d'un bras, à passer à `paradedb.parse_with_field('body', …)`.
/// Réutilise le builder pur [`phrase_combo_clauses`] : un seul lieu de vérité
/// pour la construction du corps, les bras n'en sélectionnant qu'une vue.
pub fn body_query_for_arm(query: &str, arm: BodyArm) -> String {
    let toks = tokenize_body(query);
    if toks.is_empty() {
        return query.to_string();
    }
    let parts = phrase_combo_clauses(&toks, &[]);
    match arm {
        // parts[0] = le sac OR (toujours en tête) ; les clauses phrase sont jetées.
        BodyArm::Bag => parts.into_iter().next().unwrap_or_default(),
        BodyArm::Split => parts.join(" "),
        BodyArm::Weighted(w) => parts
            .into_iter()
            .enumerate()
            .map(|(i, p)| if i == 0 { p } else { format!("{p}^{w}") })
            .collect::<Vec<_>>()
            .join(" "),
        BodyArm::Forms(forms) => {
            let mut seen = HashSet::new();
            let bag = toks
                .iter()
                .flat_map(|t| std::iter::once(t.clone()).chain(forms(t)))
                .filter(|t| seen.insert(t.clone()))
                .collect::<Vec<_>>()
                .join(" ");
            std::iter::once(bag)
                .chain(parts.into_iter().skip(1))
                .collect::<Vec<_>>()
                .join(" ")
        }
        BodyArm::RequiredForms(forms) => {
            let mut seen = HashSet::new();
            let required = toks
                .iter()
                .filter(|t| !is_stopword(t) && seen.insert((*t).clone()))
                .map(|t| match forms(t) {
                    f if f.is_empty() => format!("+{t}"),
                    f => format!("+({t} {})", f.join(" ")),
                });
            required
                .chain(parts.into_iter().skip(1))
                .collect::<Vec<_>>()
                .join(" ")
        }
    }
}

/// Construit les clauses (gazetteer injecté via `spans`, pour testabilité) :
///
/// 1. sac OR : tous les tokens dédupés (rappel maximal) ;
/// 2. phrase entière (si ≥ 2 tokens) : boost du verbatim ;
/// 3. clauses phrase, en un balayage gauche→droite : à chaque span de
///    collocation reconnue (peut contenir des stopwords internes — l'index
///    préserve les gaps de position, donc `"tribunal de commerce"` matche), on
///    émet la collocation ; entre/hors spans, on regroupe les runs de contenu
///    ≥ 2 tokens, bornés aux tokens non couverts.
///
/// La collocation à stopword interne remplace ainsi le fragment produit par le
/// split-au-stopword (« tribunal · commerce paris » → « tribunal de commerce »).
pub(crate) fn phrase_combo_clauses(toks: &[String], spans: &[(usize, usize)]) -> Vec<String> {
    // 1. Sac OR : dédup en préservant l'ordre.
    let mut seen = HashSet::new();
    let or_bag = toks
        .iter()
        .filter(|t| seen.insert((*t).clone()))
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let mut parts: Vec<String> = vec![or_bag];

    // 2. Phrase entière.
    if toks.len() >= 2 {
        parts.push(format!("\"{}\"", toks.join(" ")));
    }

    // 3. Balayage : collocations (spans) + runs de contenu sur les gaps.
    let span_end: HashMap<usize, usize> = spans.iter().copied().collect();
    let mut run: Vec<&str> = Vec::new();
    let flush = |run: &mut Vec<&str>, parts: &mut Vec<String>| {
        if run.len() >= 2 {
            parts.push(format!("\"{}\"", run.join(" ")));
        }
        run.clear();
    };
    let mut i = 0;
    while i < toks.len() {
        if let Some(&end) = span_end.get(&i) {
            flush(&mut run, &mut parts);
            parts.push(format!("\"{}\"", toks[i..end].join(" ")));
            i = end;
        } else if is_stopword(&toks[i]) {
            flush(&mut run, &mut parts);
            i += 1;
        } else {
            run.push(&toks[i]);
            i += 1;
        }
    }
    flush(&mut run, &mut parts);

    // Dédup des clauses : sans stopword ni collocation, l'unique run == la phrase
    // entière (double poids sinon).
    let mut seen_parts = HashSet::new();
    parts.retain(|p| seen_parts.insert(p.clone()));
    parts
}

static QUOTED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""[^"]*"(?:~\d+)?"#).unwrap());

/// The quoted phrases of an operator-free query, as a query of their own: what
/// every result of the hybrid must hold.
pub(crate) fn quoted_constraint(query: &str) -> Option<String> {
    if has_boolean_operator(query) {
        return None;
    }
    let query = DOUBLE_QUOTE_RE.replace_all(query, r#""$1""#);
    let phrases: Vec<&str> = QUOTED_RE
        .find_iter(&query)
        .map(|m| m.as_str())
        .filter(|p| !query_lacks_searchable_terms(p))
        .collect();
    (!phrases.is_empty()).then(|| phrases.join(" "))
}

fn unquoted(query: &str) -> String {
    let query = DOUBLE_QUOTE_RE.replace_all(query, r#""$1""#);
    QUOTED_RE.replace_all(&query, " ").replace('"', " ")
}

/// The question the hybrid embeds: the whole query, quote marks and slops dropped.
pub(crate) fn without_quotes(query: &str) -> String {
    let query = DOUBLE_QUOTE_RE.replace_all(query, r#""$1""#);
    QUOTED_RE
        .replace_all(&query, |c: &regex::Captures| {
            let m = &c[0];
            m[1..m.rfind('"').unwrap_or(m.len())].to_string()
        })
        .replace('"', " ")
}

/// The body query of a constrained hybrid: the quoted phrases required, the
/// other words ranking as a plain query's do.
pub(crate) fn constrained_parse(query: &str, constraint: &str) -> String {
    let required = translate_boolean(constraint);
    let rest = unquoted(query);
    match query_lacks_searchable_terms(&rest) {
        true => required,
        false => format!("{required} {}", phrase_combo_parse(&rest)),
    }
}

/// Every word required, as `ET` does; on a plain query, the phrase clauses only rank.
pub(crate) fn lexical_parse(query: &str) -> String {
    let required = translate_boolean(query);
    if is_boolean_query(query) {
        return required;
    }
    let phrases = phrase_combo_clauses(&tokenize_body(query), &[])
        .into_iter()
        .skip(1);
    std::iter::once(required)
        .chain(phrases)
        .collect::<Vec<_>>()
        .join(" ")
}

/// `true` si la requête ne porte aucun token de contenu indexable : vide, ou
/// 100 % stopwords (que le tokenizer `body` retire). Les wildcards `terme*`
/// survivent à la tokenisation comme prefix-query (jamais term-vides), donc une
/// requête à `*` n'est jamais court-circuitée. Sinon les jambes BM25 généreraient
/// une query tantivy sans clause, que `paradedb.parse` rejette (`body:()` → parse
/// error).
pub(crate) fn query_lacks_searchable_terms(query: &str) -> bool {
    let has_prefix_wildcard = query
        .split_whitespace()
        .any(|t| t.contains('*') && !is_bare_star(t.trim_start_matches(['+', '-'])));
    !has_prefix_wildcard && tokenize_body(query).iter().all(|t| is_stopword(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    use lj_textsearch::body_tok::content_chunks;

    #[test]
    fn detect_mode_boolean_operators() {
        assert_eq!(detect_query_mode("congés ET payés"), QueryMode::Lexical);
        assert_eq!(detect_query_mode("\"phrase exacte\""), QueryMode::Lexical);
        assert_eq!(detect_query_mode("expulsion locative"), QueryMode::Hybrid);
        assert_eq!(
            detect_query_mode("travail PROCHE3 dimanche"),
            QueryMode::Lexical
        );
        assert_eq!(
            detect_query_mode("travail NEAR3 dimanche"),
            QueryMode::Lexical
        );
    }

    #[test]
    fn translate_boolean_et_ou_sauf() {
        assert_eq!(translate_boolean("congés ET payés"), "+congés +payés");
        assert_eq!(translate_boolean("congés OU payés"), "congés OR payés");
        assert_eq!(translate_boolean("congés SAUF maladie"), "+congés -maladie");
    }

    #[test]
    fn proximity_infix_becomes_slop_phrase() {
        assert_eq!(
            translate_boolean("licenciement PROCHE5 harcèlement"),
            r#"+"licenciement harcèlement"~5"#
        );
        assert_eq!(
            translate_boolean("bail NEAR5 commercial"),
            translate_boolean("bail PROCHE5 commercial")
        );
        assert_eq!(
            translate_boolean(r#""travail dominical" PROCHE3 rupture"#),
            r#"+"travail dominical rupture"~3"#
        );
        assert_eq!(
            translate_boolean("bail PROCHE5 commercial PROCHE3 résiliation"),
            r#"+"bail commercial résiliation"~5"#
        );
        assert_eq!(
            translate_boolean("bail PROCHE5 commercial ET résiliation"),
            r#"+"bail commercial"~5 +résiliation"#
        );
        assert_eq!(
            translate_boolean("bail PROCHE5 commercial SAUF habitation"),
            r#"+"bail commercial"~5 -habitation"#
        );
        assert_eq!(
            translate_boolean("bail PROCHE5 commercial OU habitation"),
            r#""bail commercial"~5 OR habitation"#
        );
    }

    #[test]
    fn a_quote_beside_free_words_constrains_the_hybrid() {
        let q = r#""Dupont" licenciement"#;
        assert_eq!(detect_query_mode(q), QueryMode::Hybrid);
        assert_eq!(quoted_constraint(q).as_deref(), Some(r#""Dupont""#));
        assert_eq!(
            constrained_parse(q, r#""Dupont""#),
            r#"+"Dupont" licenciement"#
        );
        assert_eq!(
            without_quotes(r#""bail commercial"~3 résiliation"#),
            "bail commercial résiliation"
        );

        assert_eq!(
            detect_query_mode(r#""force majeure" "épidémie""#),
            QueryMode::Lexical
        );
        assert_eq!(
            detect_query_mode(r#""force majeure" ET épidémie"#),
            QueryMode::Lexical
        );
        assert_eq!(quoted_constraint(r#""force majeure" ET épidémie"#), None);
        assert_eq!(
            quoted_constraint(r#""de la" "faute lourde" employeur"#).as_deref(),
            Some(r#""faute lourde""#)
        );
    }

    #[test]
    fn lexical_parse_requires_every_word() {
        assert_eq!(
            lexical_parse("harcèlement moral"),
            r#"+harcèlement +moral "harcelement moral""#
        );
        assert_eq!(
            lexical_parse("faute de la victime"),
            r#"+faute +victime "faute de la victime""#
        );
        assert_eq!(lexical_parse("congés ET payés"), "+congés +payés");
        assert_eq!(
            lexical_parse(r#""force majeure" épidémie"#),
            r#"+"force majeure" +épidémie"#
        );
    }

    #[test]
    fn dangling_proximity_operator_is_dropped() {
        assert_eq!(
            translate_boolean(r#""travail dimanche"PROCHE3"#),
            r#"+"travail dimanche""#
        );
        assert_eq!(translate_boolean("PROCHE5 bail"), "+bail");
        assert_eq!(translate_boolean("bail ET PROCHE5"), "+bail");
        assert_eq!(translate_boolean("approche5 ET bail"), "+approche5 +bail");
        assert_eq!(detect_query_mode("approche5 bail"), QueryMode::Hybrid);
    }

    #[test]
    fn a_parenthesis_glued_to_a_phrase_groups() {
        assert_eq!(
            translate_boolean(r#"loyer ET ("prise d'effet" OU forfait)"#),
            r#"loyer AND ( "prise d'effet" OR forfait )"#
        );
        assert_eq!(
            translate_boolean(r#"vente SAUF ("hors magasin" ET particulier*)"#),
            r#"+vente -( +"hors magasin" +particulier* )"#
        );
        assert_eq!(translate_boolean("(bail commercial"), "+bail +commercial");
        assert_eq!(translate_boolean("bail) ET ()"), "+bail");
        assert_eq!(translate_boolean("contrat SAUF (de la)"), "+contrat");
        assert_eq!(
            translate_boolean("(congés OU *) ET payés"),
            "( congés ) AND payés"
        );
    }

    #[test]
    fn proximity_distributes_over_a_group() {
        assert_eq!(
            translate_boolean(r#""délais de grâce" PROCHE15 ("ne saurait" OU "ne permet pas")"#),
            r#"( "délais de grâce ne saurait"~15 OR "délais de grâce ne permet pas"~15 )"#
        );
        assert_eq!(
            translate_boolean("(mur OU sol) PROCHE5 (privé OU commun) ET lot"),
            r#"( "mur privé"~5 OR "mur commun"~5 OR "sol privé"~5 OR "sol commun"~5 ) AND lot"#
        );
        assert_eq!(
            translate_boolean("(note PROCHE5 Internet OU page) PROCHE3 notaire"),
            r#"( "note Internet notaire"~5 OR "page notaire"~3 )"#
        );
        assert_eq!(
            translate_boolean("bail PROCHE5 (commercial OU (rural ET fermage))"),
            "bail ( commercial OR ( rural AND fermage ) )"
        );
    }

    #[test]
    fn enforce_and_strips_stopwords() {
        assert_eq!(enforce_and_precedence("congés de payés"), "+congés +payés");
    }

    #[test]
    fn bare_syntax_tokens_are_quoted() {
        assert_eq!(
            translate_boolean(r#"prise d'acte "produit les effets d'une démission""#),
            r#"+prise +"d'acte" +"produit les effets d'une démission""#
        );
        assert_eq!(translate_boolean(r#"acte(s ET nul"#), r#"+"acte(s" +nul"#);
        assert_eq!(
            translate_boolean(r#"répar* ET "l'astreinte""#),
            r#"+répar* +"l'astreinte""#
        );
        assert_eq!(
            translate_boolean("congés OU d'aménagement"),
            r#"congés OR "d'aménagement""#
        );
        assert_eq!(
            translate_boolean(r#""construction nouvelle" OU annexe : la règle [x] ^2 {a acte(s"#),
            r#""construction nouvelle" OR annexe ":" la règle "[x]" "^2" "{a" "acte(s""#
        );
        assert_eq!(
            translate_boolean("(bail OU loyer) ET -art.:3"),
            r#"( bail OR loyer ) AND -"art.:3""#
        );
    }

    #[test]
    fn bare_star_token_is_dropped() {
        assert_eq!(translate_boolean("2 * 3"), "+2 +3");
        assert_eq!(translate_boolean("faute ** dommage"), "+faute +dommage");
        assert_eq!(translate_boolean("congés OU *"), "congés");
        assert_eq!(translate_boolean("répar* ET faute"), "+répar* +faute");
        assert!(query_lacks_searchable_terms("*"));
        assert!(query_lacks_searchable_terms("* **"));
    }

    #[test]
    fn stopword_only_query_lacks_searchable_terms() {
        assert!(query_lacks_searchable_terms("de la"));
        assert!(query_lacks_searchable_terms("le la les des du"));
        assert!(query_lacks_searchable_terms("de ET la"));
        assert!(query_lacks_searchable_terms(""));
        assert!(query_lacks_searchable_terms("   "));
        assert!(!query_lacks_searchable_terms("responsabilité"));
        assert!(!query_lacks_searchable_terms("code de la route"));
        assert!(!query_lacks_searchable_terms("de*"));
    }

    #[derive(serde::Deserialize)]
    struct QueryCase {
        query: String,
        mode: String,
        translate_boolean: String,
        phrase_combo_parse: String,
        tokenize_body: Vec<String>,
        content_chunks: Vec<Vec<String>>,
    }
    #[derive(serde::Deserialize)]
    struct QueryFixture {
        cases: Vec<QueryCase>,
    }

    #[test]
    fn query_construction_parity_oracle() {
        let raw = include_str!("../../tests/fixtures/oracle/query_strings.json");
        let fix: QueryFixture = serde_json::from_str(raw).expect("fixture query_strings");
        for c in &fix.cases {
            let mode = match detect_query_mode(&c.query) {
                QueryMode::Lexical => "lexical",
                QueryMode::Hybrid => "hybrid",
                QueryMode::Reference => "reference",
                QueryMode::Browse => "browse",
            };
            assert_eq!(mode, c.mode, "detect_query_mode({:?})", c.query);
            assert_eq!(
                translate_boolean(&c.query),
                c.translate_boolean,
                "translate_boolean({:?})",
                c.query
            );
            assert_eq!(
                phrase_combo_clauses(&tokenize_body(&c.query), &[]).join(" "),
                c.phrase_combo_parse,
                "phrase_combo_clauses({:?}, [])",
                c.query
            );
            assert_eq!(
                tokenize_body(&c.query),
                c.tokenize_body,
                "tokenize_body({:?})",
                c.query
            );
            assert_eq!(
                content_chunks(&tokenize_body(&c.query), 2),
                c.content_chunks,
                "content_chunks({:?})",
                c.query
            );
        }
    }

    #[test]
    fn phrase_combo_dedups_or_leg() {
        let out = phrase_combo_parse("podologie podologie");
        assert!(out.starts_with("podologie "), "got: {out}");
    }

    #[test]
    fn phrase_combo_gazetteer_replaces_split_fragment() {
        let toks = tokenize_body("tribunal de commerce de paris competence litige");
        let m = lj_textsearch::collocations::Matcher::from_phrases(["tribunal de commerce"]);
        let clauses = phrase_combo_clauses(&toks, &m.spans(&toks));
        assert!(
            clauses.iter().any(|c| c == "\"tribunal de commerce\""),
            "{clauses:?}"
        );
        assert!(
            !clauses.iter().any(|c| c == "\"commerce paris\""),
            "{clauses:?}"
        );
        assert_eq!(clauses[0], "tribunal de commerce paris competence litige");
        assert!(clauses
            .iter()
            .any(|c| c == "\"tribunal de commerce de paris competence litige\""));
    }

    #[test]
    fn phrase_combo_gazetteer_two_disjoint_collocations() {
        let toks = tokenize_body("code de la route exces de vitesse permis");
        let m = lj_textsearch::collocations::Matcher::from_phrases([
            "code de la route",
            "exces de vitesse",
        ]);
        let clauses = phrase_combo_clauses(&toks, &m.spans(&toks));
        assert!(
            clauses.iter().any(|c| c == "\"code de la route\""),
            "{clauses:?}"
        );
        assert!(
            clauses.iter().any(|c| c == "\"exces de vitesse\""),
            "{clauses:?}"
        );
        assert!(
            !clauses.iter().any(|c| c == "\"route exces\""),
            "{clauses:?}"
        );
    }

    #[test]
    fn phrase_combo_uncovered_content_run_survives() {
        let toks = tokenize_body("tribunal de commerce detention provisoire");
        let m = lj_textsearch::collocations::Matcher::from_phrases(["tribunal de commerce"]);
        let clauses = phrase_combo_clauses(&toks, &m.spans(&toks));
        assert!(
            clauses.iter().any(|c| c == "\"tribunal de commerce\""),
            "{clauses:?}"
        );
        assert!(
            clauses.iter().any(|c| c == "\"detention provisoire\""),
            "{clauses:?}"
        );
    }
}
