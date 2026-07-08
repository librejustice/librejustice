//! Tokenizer body — parité exacte avec le tokenizer de l'index `chunks_bm25`
//! (ParadeDB `regex([\p{L}\p{N}-]+) + ascii_folding`, stopwords tantivy `French`
//! + `a`/`à` ; ADR 0073, migration 0048).
//!
//! Vit ici (PUR) parce qu'il est partagé par DEUX chemins qui doivent folder
//! et segmenter à l'identique, sous peine de drift silencieux :
//! - la jambe BM25 body de `lj-api` (génération de la query `paradedb.parse`),
//! - le mineur de collocations de `lj-bench` (qui foldé le corpus pour produire
//!   le lexique embarqué par [`crate::collocations`]).
//!
//! Si les deux foldent différemment, une entrée de lexique ne matche jamais la
//! query correspondante. Une seule implémentation → pas de drift.

use std::collections::HashSet;
use std::sync::LazyLock;

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

/// `true` si `folded` (un token déjà passé par [`fold`]) est un stopword de
/// l'index.
pub fn is_stopword(folded: &str) -> bool {
    FR_STOPWORDS.contains(folded)
}

/// Tokens body (Unicode-first puis ascii-fold + lowercase) — matche le
/// body-tokenizer ParadeDB (`[\p{L}\p{N}-]+`). On tokenise AVANT de fold.
pub fn tokenize(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut chars = query.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch.is_alphanumeric() {
            cur.push(ch);
        } else if ch == '-' && !cur.is_empty() {
            // Conserve les composés `[^\W_]+(?:-[^\W_]+)*` : un `-` n'est gardé
            // que s'il est suivi d'un autre caractère mot.
            if matches!(chars.peek(), Some(c) if c.is_alphanumeric()) {
                cur.push('-');
            } else if !cur.is_empty() {
                tokens.push(fold(&cur));
                cur.clear();
            }
        } else if !cur.is_empty() {
            tokens.push(fold(&cur));
            cur.clear();
        }
    }
    if !cur.is_empty() {
        tokens.push(fold(&cur));
    }
    tokens
}

/// NFKD ascii-fold + lowercase (réplique `unicodedata.NFKD + ascii ignore + lower`).
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        for lc in ch.to_lowercase() {
            out.push(ascii_fold_char(lc));
        }
    }
    out
}

fn ascii_fold_char(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'ö' | 'õ' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        _ => c,
    }
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
        // « de » splitte ; run gauche < 2 (tribunal seul) drop, run droit ≥ 2 gardé.
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
    fn stopwords_are_folded_forms() {
        assert!(is_stopword("de"));
        assert!(is_stopword("la"));
        assert!(is_stopword("a")); // ajout maison (a/à)
        assert!(!is_stopword("commerce"));
    }
}
