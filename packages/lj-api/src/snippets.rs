//! Surlignage de snippets — port natif de `snippets.py`, **sans index inversé**.
//!
//! On réplique 1-pour-1 le tokenizer ParadeDB (regex `[\p{L}\p{N}-]+` +
//! ascii_folding + lowercase + stopwords FR optionnels) via le `TextAnalyzer` de
//! tantivy (version EPINGLEE `=0.26.0`, le core exact wrappé par py-tantivy 0.26),
//! puis on porte la sélection « best fragment » de tantivy (`search_fragments` +
//! `select_best_fragment_combination` de `snippet/mod.rs`) sur des tokens
//! **précalculés**. Aucun `Index`/`writer`/`commit`/segment : un index in-RAM
//! squelette (bâti UNE fois pour le process, cf. [`ENGINE`]) sert seulement à
//! construire le `QueryParser` (parsing de la requête comme tantivy le ferait) ;
//! le `doc_freq` — pour le score `1/(1+df)` par terme
//! — est compté à la main sur les mêmes tokens. On tokenise chaque doc UNE fois
//! et on ne paie jamais la sérialisation d'un index inversé (poste dominant :
//! ~12 ms de `commit` par appel, indépendant du volume — cf. docs/working-notes).
//!
//! Résiduel de parité connu (cosmétique) : quand le meilleur fragment est le
//! TOUT PREMIER du texte, py-tantivy l'étend jusqu'à l'offset 0 (inclut les
//! caractères non-tokens de tête, p.ex. `\n\n`) là où notre port démarre au
//! premier token. Le contenu marqué est identique ; seul le bord gauche de la
//! fenêtre differe — cf. docs/working-notes/2026-06-05_api-parity-rust-vs-python.md.
//!
//! Le HTML `<mark>` est reconstruit à la main (ranges en octets UTF-8) pour ne
//! PAS échapper les entités HTML — comme `_to_mark_html` côté Python : le
//! consommateur React rend chaque fragment comme text node, donc un échappement
//! façon `Snippet::to_html` (qui appelle `encode_minimal`) afficherait des
//! entités littérales. L'XSS est géré par React.
//!
//! Les configs ``SNIPPET_TOKENIZER`` sont la source de vérité ; elles doivent
//! rester synchronisées avec le ``CREATE INDEX ... USING bm25`` de
//! ``decisions_bm25`` (champs ``full_text`` + ``search_title``, ADR 0084).

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;
use std::sync::LazyLock;

use tantivy::query::{Query, QueryParser};
use tantivy::schema::{Field, IndexRecordOption, Schema, TextFieldIndexing, TEXT};
use tantivy::tokenizer::{
    AsciiFoldingFilter, Language, LowerCaser, RegexTokenizer, StopWordFilter, TextAnalyzer, Token,
    TokenStream,
};
use tantivy::{Index, Score, Term};

/// Budget de caractères par défaut d'un snippet (= `_DEFAULT_MAX_CHARS`).
pub const DEFAULT_MAX_CHARS: usize = 500;

/// Pattern du tokenizer ParadeDB répliqué (= `TokenizerConfig.pattern`).
const TOKENIZER_PATTERN: &str = r"[\p{L}\p{N}-]+";

/// Nom du tokenizer custom enregistré dans l'index (= `"t"` côté Python).
const TOKENIZER_NAME: &str = "t";

/// Tokenizer ParadeDB répliqué : pattern Unicode `[\p{L}\p{N}-]+`,
/// ascii-folding + lowercase, stopwords FR optionnels.
///
/// À synchroniser avec les ``text_fields`` JSON dans les migrations
/// ``CREATE INDEX ... USING bm25``. Aucun stemming côté ParadeDB → on n'en
/// ajoute pas non plus ici.
#[derive(Debug, Clone, Copy)]
pub struct TokenizerConfig {
    pub ascii_folding: bool,
    pub use_french_stopwords: bool,
}

/// Tokenizer unique pour les snippets (full_text et titre). Stopwords FR pour que
/// la sélection de fragment centre la fenêtre sur les termes content — pas « de ».
/// Même esprit que l'index DB ``decisions_bm25`` (stopwords FR + ``["a", "à"]``,
/// ADR 0084) ; seul écart : la liste custom ``["a", "à"]`` n'est pas portée ici
/// (l'API `StopWordFilter` de tantivy ne prend que la langue) — acceptable : le
/// snippet est un rendu visuel, pas un signal de ranking.
pub const SNIPPET_TOKENIZER: TokenizerConfig = TokenizerConfig {
    ascii_folding: true,
    use_french_stopwords: true,
};

/// Construit le `TextAnalyzer` répliquant le tokenizer ParadeDB (= `_build_analyzer`).
///
/// Ordre des filtres calqué sur Python : regex → ascii_fold → lowercase →
/// stopword. Tantivy applique les filtres dans l'ordre d'ajout.
fn build_analyzer(cfg: TokenizerConfig) -> TextAnalyzer {
    let tokenizer =
        RegexTokenizer::new(TOKENIZER_PATTERN).expect("pattern regex tokenizer ParadeDB valide");
    let mut builder = TextAnalyzer::builder(tokenizer).dynamic();
    if cfg.ascii_folding {
        builder = builder.filter_dynamic(AsciiFoldingFilter);
    }
    builder = builder.filter_dynamic(LowerCaser);
    if cfg.use_french_stopwords {
        builder = builder.filter_dynamic(
            StopWordFilter::new(Language::French).expect("stopwords français tantivy disponibles"),
        );
    }
    builder.build()
}

/// Composants tantivy partagés, construits UNE fois pour tout le process. Évite
/// de reconstruire par appel un index in-RAM squelette + un `QueryParser` + de
/// recompiler la regex Unicode du tokenizer (~0,5 ms de coût fixe par appel,
/// mesuré release sur 2 cœurs). `parse_query`/`token_stream` ne mutent rien de
/// partagé : le parser se prête par `&`, l'analyzer se clone (clone d'`Arc` sur
/// la regex compilée — bien moins cher que `RegexTokenizer::new`).
struct SnippetEngine {
    parser: QueryParser,
    /// Prototype à cloner par appel (le `token_stream` exige `&mut`).
    analyzer: TextAnalyzer,
    body: Field,
}

/// Index squelette (schéma + tokenizer enregistré) bâti seulement pour en
/// extraire le `QueryParser`. Aucun writer/commit : on ne matérialise jamais
/// d'index inversé. La config est `SNIPPET_TOKENIZER`, source de vérité unique.
static ENGINE: LazyLock<SnippetEngine> = LazyLock::new(|| {
    let mut schema_builder = Schema::builder();
    let body = schema_builder.add_text_field(
        "body",
        TEXT.set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(TOKENIZER_NAME)
                .set_index_option(IndexRecordOption::Basic),
        ),
    );
    let index = Index::create_in_ram(schema_builder.build());
    index
        .tokenizers()
        .register(TOKENIZER_NAME, build_analyzer(SNIPPET_TOKENIZER));
    // `for_index` capture une copie du schéma + un `Arc` du tokenizer manager :
    // le parser survit à la libération de l'index.
    let parser = QueryParser::for_index(&index, vec![body]);
    SnippetEngine {
        parser,
        analyzer: build_analyzer(SNIPPET_TOKENIZER),
        body,
    }
});

/// Reconstruit le HTML `<mark>` d'un fragment + ses ranges surlignés (= `_to_mark_html`).
///
/// `highlighted` est en **octets UTF-8** relatifs à `fragment` : on slice
/// directement les bytes pour que les accents (``considère``, ``Île``…) ne
/// décalent pas les marks. Pas d'échappement HTML : le fragment est rendu comme
/// text node côté React. Les ranges sont disjoints (un token = un mark), donc un
/// simple tri suffit — pas de fusion d'intervalles.
fn to_mark_html(fragment: &str, highlighted: &[Range<usize>]) -> String {
    let mut ranges: Vec<(usize, usize)> = highlighted.iter().map(|r| (r.start, r.end)).collect();
    if ranges.is_empty() {
        return fragment.to_string();
    }
    ranges.sort_unstable();
    let mut out = String::with_capacity(fragment.len() + ranges.len() * 13);
    let mut cursor = 0usize;
    for (start, end) in ranges {
        out.push_str(&fragment[cursor..start]);
        out.push_str("<mark>");
        out.push_str(&fragment[start..end]);
        out.push_str("</mark>");
        cursor = end;
    }
    out.push_str(&fragment[cursor..]);
    out
}

/// Retourne `{doc_id: snippet_html}` pour chaque doc qui matche `query` (= `highlight`).
///
/// Les ids absents du retour n'ont pas matché — l'appelant doit fallback sur le
/// texte brut. `query` vide ou contenant uniquement des stopwords → `{}`.
pub fn highlight(docs: &[(i64, String)], query: &str, max_chars: usize) -> HashMap<i64, String> {
    if docs.is_empty() || query.trim().is_empty() {
        return HashMap::new();
    }

    // Parser partagé (cf. [`ENGINE`]) : on parse la requête comme tantivy le
    // ferait, sans reconstruire d'index ni recompiler le tokenizer par appel.
    let parsed = match ENGINE.parser.parse_query(query) {
        Ok(q) => q,
        // Query invalide pour le tokenizer (ex. uniquement des stopwords).
        Err(_) => return HashMap::new(),
    };

    // Termes de la requête pour le champ body (= `SnippetGenerator::create`).
    let mut query_terms: BTreeSet<&Term> = BTreeSet::new();
    parsed.query_terms(&mut |term, _| {
        if term.field() == ENGINE.body {
            query_terms.insert(term);
        }
    });

    // UNE tokenisation par doc, conservée : elle sert au comptage du `doc_freq`
    // PUIS à la sélection de fragment (le tokenizer regex est le poste dominant —
    // on évite la double passe). Clone du prototype partagé (cheap vs recompile).
    let mut analyzer = ENGINE.analyzer.clone();
    let doc_tokens: Vec<Vec<Token>> = docs
        .iter()
        .map(|(_, text)| {
            let mut tokens = Vec::new();
            let mut stream = analyzer.token_stream(text);
            while let Some(token) = stream.next() {
                tokens.push(token.clone());
            }
            tokens
        })
        .collect();

    // `doc_freq` maison → score `1/(1+df)` par terme (formule exacte de
    // `SnippetGenerator::create`). Le tokenizer ayant déjà lowercasé/foldé, le
    // texte des tokens est directement comparable au `term_str` de la requête.
    let mut terms_text: BTreeMap<String, Score> = BTreeMap::new();
    for term in query_terms {
        let term_value = term.value();
        let Some(term_str) = term_value.as_str() else {
            continue;
        };
        let doc_freq = doc_tokens
            .iter()
            .filter(|toks| toks.iter().any(|t| t.text == term_str))
            .count() as u64;
        if doc_freq > 0 {
            terms_text.insert(term_str.to_string(), 1.0 / (1.0 + doc_freq as Score));
        }
    }
    if terms_text.is_empty() {
        return HashMap::new();
    }

    // Sélection best-fragment sur les tokens précalculés. Un doc sans match (aucun
    // fragment) est absent du retour → l'appelant fallback sur le texte brut.
    let mut out = HashMap::new();
    for ((doc_id, text), tokens) in docs.iter().zip(&doc_tokens) {
        let fragments = search_fragments(tokens, &terms_text, max_chars);
        if let Some((fragment, highlighted)) = select_best_fragment(&fragments, text) {
            out.insert(*doc_id, to_mark_html(&fragment, &highlighted));
        }
    }
    out
}

/// Fragment candidat — port de `FragmentCandidate` (tantivy 0.26 `snippet/mod.rs`).
struct Fragment {
    score: Score,
    start_offset: usize,
    stop_offset: usize,
    highlighted: Vec<Range<usize>>,
}

/// Découpe le texte en fragments candidats — port verbatim de `search_fragments`
/// (tantivy 0.26), avec `try_add_token` inliné, mais sur des tokens **déjà
/// calculés** (au lieu de re-tokeniser le texte). Verrouillé par la fixture
/// oracle (`snippet_highlight_parity_oracle`).
fn search_fragments(
    tokens: &[Token],
    terms: &BTreeMap<String, Score>,
    max_num_chars: usize,
) -> Vec<Fragment> {
    let mut fragment = Fragment {
        score: 0.0,
        start_offset: 0,
        stop_offset: 0,
        highlighted: vec![],
    };
    let mut fragments: Vec<Fragment> = vec![];
    for token in tokens {
        if (token.offset_to - fragment.start_offset) > max_num_chars {
            if fragment.score > 0.0 {
                fragments.push(fragment);
            }
            fragment = Fragment {
                score: 0.0,
                start_offset: token.offset_from,
                stop_offset: token.offset_from,
                highlighted: vec![],
            };
        }
        fragment.stop_offset = token.offset_to;
        if let Some(&score) = terms.get(&token.text.to_lowercase()) {
            fragment.score += score;
            fragment
                .highlighted
                .push(token.offset_from..token.offset_to);
        }
    }
    if fragment.score > 0.0 {
        fragments.push(fragment);
    }
    fragments
}

/// Choisit le meilleur fragment et renvoie `(texte, ranges relatifs au fragment)`
/// — port de `select_best_fragment_combination` (tantivy 0.26). `None` si aucun
/// fragment (= aucun terme de la requête dans le doc).
fn select_best_fragment(fragments: &[Fragment], text: &str) -> Option<(String, Vec<Range<usize>>)> {
    let best = fragments.iter().max_by(|left, right| {
        let cmp_score = left
            .score
            .partial_cmp(&right.score)
            .unwrap_or(Ordering::Equal);
        if cmp_score == Ordering::Equal {
            (right.start_offset, right.stop_offset).cmp(&(left.start_offset, left.stop_offset))
        } else {
            cmp_score
        }
    })?;
    let fragment_text = text[best.start_offset..best.stop_offset].to_string();
    let highlighted = best
        .highlighted
        .iter()
        .map(|item| item.start - best.start_offset..item.end - best.start_offset)
        .collect();
    Some((fragment_text, highlighted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_returns_empty() {
        let docs = vec![(1i64, "un texte".to_string())];
        assert!(highlight(&docs, "", DEFAULT_MAX_CHARS).is_empty());
        assert!(highlight(&docs, "   ", DEFAULT_MAX_CHARS).is_empty());
    }

    #[test]
    fn stopword_only_query_returns_empty() {
        let docs = vec![(1i64, "le tribunal".to_string())];
        let out = highlight(&docs, "le de la", DEFAULT_MAX_CHARS);
        assert!(out.is_empty());
    }

    #[test]
    fn marks_matched_token_diacritics_insensitive() {
        let docs = vec![(7i64, "La cour considère le moyen".to_string())];
        let out = highlight(&docs, "considere", DEFAULT_MAX_CHARS);
        let html = out.get(&7).expect("doc 7 should match");
        assert!(html.contains("<mark>considère</mark>"), "got: {html}");
    }

    #[test]
    fn non_matching_doc_absent_from_result() {
        let docs = vec![(1i64, "congés payés".to_string())];
        let out = highlight(&docs, "expulsion", DEFAULT_MAX_CHARS);
        assert!(!out.contains_key(&1));
    }

    #[test]
    fn hyphenated_token_kept_as_one() {
        let docs = vec![(1i64, "le référé-suspension prononcé".to_string())];
        let out = highlight(&docs, "référé-suspension", DEFAULT_MAX_CHARS);
        let html = out.get(&1).unwrap();
        assert!(
            html.contains("<mark>référé-suspension</mark>"),
            "got: {html}"
        );
    }

    // Parité snippets tantivy ↔ oracle Python (apps/api snippets.py `highlight`).
    // GT figée dans tests/fixtures/oracle/snippets.json : sélection best-fragment,
    // fenêtrage, `<mark>` non échappé, folding d'accents, tokens composés, absence
    // sur non-match, query stopword-only → {}, multi-doc. Les cas sont conçus avec
    // le match HORS offset 0 pour éviter le résiduel cosmétique « premier
    // fragment » documenté (bord gauche de fenêtre). Verrouille le rendu HTML
    // byte-à-byte sans Postgres ni scan BM25.
    #[derive(serde::Deserialize)]
    struct SnippetCase {
        name: String,
        docs: Vec<(i64, String)>,
        query: String,
        max_chars: usize,
        expected: std::collections::HashMap<i64, String>,
    }
    #[derive(serde::Deserialize)]
    struct SnippetFixture {
        cases: Vec<SnippetCase>,
    }

    #[test]
    fn snippet_highlight_parity_oracle() {
        let raw = include_str!("../tests/fixtures/oracle/snippets.json");
        let fix: SnippetFixture = serde_json::from_str(raw).expect("fixture snippets");
        for c in &fix.cases {
            let out = highlight(&c.docs, &c.query, c.max_chars);
            assert_eq!(out, c.expected, "snippet case {:?}", c.name);
        }
    }
}
