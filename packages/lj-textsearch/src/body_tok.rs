//! Tokenizer body — parité exacte avec le tokenizer des index BM25 (ParadeDB
//! `regex([`TOKEN_PATTERN`]) + ascii_folding`, stopwords tantivy `French` +
//! `a`/`à`).
//!
//! Vit ici (PUR) parce qu'il est partagé par TROIS chemins qui doivent folder
//! et segmenter à l'identique, sous peine de drift silencieux :
//! - la jambe BM25 body de `lj-api` (génération de la query `paradedb.parse`),
//! - le tokenizer tantivy des snippets (`lj_api::snippets`),
//! - le mineur de collocations hors ligne (qui folde le corpus pour produire le
//!   lexique embarqué par [`crate::collocations`]).
//!
//! Si deux d'entre eux foldent différemment, une entrée de lexique ne matche
//! jamais la query correspondante, et un snippet surligne à côté du terme trouvé.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

/// Pattern du tokenizer BM25, source unique : les `text_fields` des migrations,
/// le tokenizer tantivy des snippets et [`tokenize`] portent tous celui-ci.
/// `legal_article_usage_bm25.terms` y ajoute `_`, seule variation du dépôt.
///
/// The first alternative keeps a number whole (`2008/115`, `19-20.312`, `3.000`),
/// because on a European instrument the number IS the identifier, and splitting it
/// costs precision. The alternation is leftmost-first: at a numeric position, the
/// glued form wins. The hyphen was already glued by the second alternative.
pub const TOKEN_PATTERN: &str = r"\p{N}+(?:[./-]\p{N}+)+|[\p{L}\p{N}-]+";

static TOKEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(TOKEN_PATTERN).expect("TOKEN_PATTERN compile"));

/// Stopwords du tokenizer de l'index `chunks_bm25` (liste tantivy `French` +
/// `a`/`à`), foldés ascii comme la sortie de [`tokenize`]. Les lookups se font
/// sur tokens foldés — c'est ce qui rend le split des chunks et le strip du mode
/// booléen cohérents avec ce que l'index contient réellement.
static FR_STOPWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "a", "ai", "aie", "aient", "aies", "ait", "au", "aurai", "auraient", "aurais", "aurait",
        "aurez", "auriez", "aurions", "aurons", "auront", "aux", "avaient", "avais", "avait",
        "avec", "avez", "aviez", "avons", "ayant", "ayez", "ayons", "c", "ce", "ceci", "cela",
        "ces", "cet", "cette", "d", "dans", "de", "des", "du", "elle", "en", "es", "et", "etaient",
        "etais", "etait", "etant", "etee", "etees", "etes", "etiez", "etions", "eu", "eue", "eues",
        "eumes", "eurent", "eus", "eusse", "eussent", "eusses", "eussiez", "eussions", "eut",
        "eutes", "eux", "fumes", "furent", "fus", "fusse", "fussent", "fusses", "fussiez",
        "fussions", "fut", "futes", "ici", "il", "ils", "j", "je", "l", "la", "le", "les", "leur",
        "leurs", "lui", "m", "ma", "mais", "me", "meme", "mes", "moi", "mon", "n", "ne", "nos",
        "notre", "nous", "on", "ont", "ou", "par", "pas", "pour", "qu", "que", "quel", "quelle",
        "quelles", "quels", "qui", "s", "sa", "sans", "se", "sera", "serai", "seraient", "serais",
        "serait", "seras", "serez", "seriez", "serions", "serons", "seront", "ses", "soi",
        "soient", "sois", "soit", "sont", "soyez", "soyons", "suis", "sur", "t", "ta", "te", "tes",
        "toi", "ton", "tu", "un", "une", "vos", "votre", "vous", "y",
    ]
    .into_iter()
    .collect()
});

/// Whether `folded`, a token already through `lj_fold::fold_chars`, is an index
/// stopword.
pub fn is_stopword(folded: &str) -> bool {
    FR_STOPWORDS.contains(folded)
}

/// Tokens body (Unicode-first puis ascii-fold + lowercase) — matche le
/// body-tokenizer ParadeDB ([`TOKEN_PATTERN`]). On tokenise AVANT de fold.
pub fn tokenize(query: &str) -> Vec<String> {
    tokenize_with(query, lj_fold::fold_chars)
}

/// Mêmes frontières de tokens que [`tokenize`], mais lowercase **sans** fold des
/// accents : la forme d'affichage des suggestions. Les deux sorties sont alignées
/// index à index.
pub fn tokenize_lower(query: &str) -> Vec<String> {
    tokenize_with(query, |s| s.to_lowercase())
}

/// Mêmes frontières que [`tokenize`], sur le texte brut : ni fold, ni minuscule.
/// Pour les chemins qui recomposent une requête et doivent rendre la forme telle
/// qu'elle a été tapée, tout en coupant comme l'index.
pub fn tokens_raw(text: &str) -> Vec<&str> {
    TOKEN_RE
        .find_iter(text)
        .map(|m| m.as_str())
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .collect()
}

fn tokenize_with(query: &str, norm: impl Fn(&str) -> String) -> Vec<String> {
    TOKEN_RE
        .find_iter(query)
        .map(|m| m.as_str())
        // Un tiret seul est un token de l'index comme un autre, mais il ne porte
        // rien et la grammaire tantivy lit un `-` de tête comme une négation.
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .map(norm)
        .collect()
}

/// Split au stopword FR, garde les runs ≥ `min_len`.
pub fn content_chunks(tokens: &[String], min_len: usize) -> Vec<Vec<String>> {
    let mut chunks: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    for t in tokens {
        if is_stopword(t) {
            if cur.len() >= min_len {
                chunks.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        } else {
            cur.push(t.clone());
        }
    }
    if cur.len() >= min_len {
        chunks.push(cur);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_chunks_split_on_stopword() {
        let toks: Vec<String> = ["tribunal", "de", "commerce", "paris"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let chunks = content_chunks(&toks, 2);
        assert_eq!(
            chunks,
            vec![vec!["commerce".to_string(), "paris".to_string()]]
        );
    }

    #[test]
    fn tokenize_folds_and_keeps_hyphen() {
        assert_eq!(tokenize("Référé-suspension"), vec!["refere-suspension"]);
        assert_eq!(tokenize("l'instruction"), vec!["l", "instruction"]);
    }

    #[test]
    fn ligatures_expand_like_the_index() {
        assert_eq!(tokenize("Mise en Œuvre"), vec!["mise", "en", "oeuvre"]);
        assert_eq!(tokenize("curriculum vitæ"), vec!["curriculum", "vitae"]);
    }

    #[test]
    fn un_numero_reste_entier() {
        // L'instrument européen : le numéro EST l'identifiant de la norme.
        assert_eq!(
            tokenize("directive 2008/115/CE"),
            vec!["directive", "2008/115", "ce"]
        );
        // Pourvoi, rôle, montant à la française.
        assert_eq!(tokenize("pourvoi 19-20.312"), vec!["pourvoi", "19-20.312"]);
        assert_eq!(tokenize("22/01772"), vec!["22/01772"]);
        assert_eq!(tokenize("3.000 euros"), vec!["3.000", "euros"]);
        // A slashed date is swallowed whole, as numbers are kept whole.
        assert_eq!(tokenize("du 16/12/2008"), vec!["du", "16/12/2008"]);
        // Un séparateur non encadré de chiffres ne colle rien.
        assert_eq!(tokenize("fin. Alors"), vec!["fin", "alors"]);
        assert_eq!(tokenize("article L. 423-7"), vec!["article", "l", "423-7"]);
    }

    #[test]
    fn stopwords_are_folded_forms() {
        assert!(is_stopword("de"));
        assert!(is_stopword("la"));
        assert!(is_stopword("a"));
        assert!(!is_stopword("commerce"));
    }
}
