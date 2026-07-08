//! Gazetteer de collocations juridiques — lexique de termes de l'art à
//! **stopword interne** (`code de la route`, `tribunal de commerce`,
//! `indemnité d'éviction`…), miné offline contre le corpus puis embarqué.
//!
//! Sert à la jambe BM25 body : `phrase_combo_parse` segmentait la requête en
//! coupant à chaque stopword (`content_chunks`), ce qui tranche pile au milieu
//! des collocations (« tribunal · commerce », « route · excès »…) et phrase des
//! fragments-déchet à fréquence nulle au lieu de la vraie collocation à dizaines
//! de milliers d'occurrences. Le gazetteer reconnaît ces spans et les phrase
//! tels quels (cf. note 2026-06-12 sur le split-au-stopword).
//!
//! Ce ne sont **pas** des entités nommées : un NER les raterait. C'est de la
//! terminologie — d'où un lexique miné, pas un modèle.
//!
//! Le lexique est produit par `lj-bench mine-collocations` (qui foldé le corpus
//! via [`crate::body_tok`], le MÊME tokenizer qu'à la query → pas de drift), et
//! écrit dans `data/collocations_fr.json`. Le matcher charge ce lexique, en
//! construit un automate Aho-Corasick et reconnaît les spans dans une requête
//! déjà tokenisée/foldée.

use std::collections::HashMap;
use std::sync::LazyLock;

use aho_corasick::{AhoCorasick, MatchKind};
use serde::Deserialize;

/// JSON brut embarqué (produit par `lj-bench mine-collocations`).
pub const COLLOCATIONS_JSON: &str = include_str!("../data/collocations_fr.json");

#[derive(Debug, Clone, Deserialize)]
struct LexiconFile {
    /// Phrases foldées (tokens joints par une espace, forme [`body_tok::tokenize`]),
    /// triées par df décroissante au minage. Le matcher n'utilise que la phrase.
    collocations: Vec<String>,
}

/// Matcher de collocations sur une séquence de tokens foldés.
pub struct Matcher {
    ac: AhoCorasick,
}

impl Matcher {
    /// Construit l'automate depuis des phrases foldées (tokens joints par espace).
    /// `LeftmostLongest` : on garde la collocation la plus longue à la position
    /// la plus à gauche, sans chevauchement — un span « code de la route » prime
    /// sur un éventuel sous-span.
    pub fn from_phrases<I, S>(phrases: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let patterns: Vec<String> = phrases
            .into_iter()
            .map(|p| p.as_ref().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        let ac = AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostLongest)
            .build(&patterns)
            .expect("automate collocations valide");
        Matcher { ac }
    }

    /// Spans `[start, end)` (indices de tokens) des collocations reconnues,
    /// non-chevauchants, leftmost-longest. `tokens` sont déjà foldés
    /// ([`crate::body_tok::tokenize`]).
    ///
    /// On matche sur `tokens.join(" ")` et on ne garde que les matches alignés
    /// sur des frontières de tokens (espace ou bord) — pour ne pas reconnaître
    /// « route » à l'intérieur de « déroute ».
    pub fn spans(&self, tokens: &[String]) -> Vec<(usize, usize)> {
        if tokens.is_empty() {
            return Vec::new();
        }
        // Haystack + table offset-octet → index de token.
        let mut hay = String::new();
        let mut start_at: HashMap<usize, usize> = HashMap::with_capacity(tokens.len());
        let mut end_at: HashMap<usize, usize> = HashMap::with_capacity(tokens.len());
        for (i, t) in tokens.iter().enumerate() {
            if i > 0 {
                hay.push(' ');
            }
            let s = hay.len();
            start_at.insert(s, i);
            hay.push_str(t);
            end_at.insert(hay.len(), i);
        }
        let bytes = hay.as_bytes();
        let mut spans = Vec::new();
        for m in self.ac.find_iter(&hay) {
            // Frontières de tokens : bord, ou espace de part et d'autre.
            let left_ok = m.start() == 0 || bytes[m.start() - 1] == b' ';
            let right_ok = m.end() == bytes.len() || bytes[m.end()] == b' ';
            if !left_ok || !right_ok {
                continue;
            }
            if let (Some(&i), Some(&j)) = (start_at.get(&m.start()), end_at.get(&m.end())) {
                spans.push((i, j + 1));
            }
        }
        spans
    }
}

/// Matcher global chargé depuis le lexique embarqué.
static MATCHER: LazyLock<Matcher> = LazyLock::new(|| {
    let lex: LexiconFile =
        serde_json::from_str(COLLOCATIONS_JSON).expect("collocations_fr.json embarqué valide");
    Matcher::from_phrases(lex.collocations)
});

/// Spans des collocations reconnues dans `tokens` (foldés), via le lexique
/// embarqué. Voir [`Matcher::spans`].
pub fn collocation_spans(tokens: &[String]) -> Vec<(usize, usize)> {
    MATCHER.spans(tokens)
}

/// Nombre de collocations dans le lexique embarqué (diagnostic).
pub fn lexicon_len() -> usize {
    serde_json::from_str::<LexiconFile>(COLLOCATIONS_JSON)
        .map(|l| l.collocations.len())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<String> {
        crate::body_tok::tokenize(s)
    }

    #[test]
    fn matches_bridge_stopword_collocation() {
        let m = Matcher::from_phrases(["tribunal de commerce", "code de la route"]);
        // « tribunal de commerce de paris » → span [0,3) (tribunal de commerce).
        let t = toks("tribunal de commerce de paris");
        assert_eq!(m.spans(&t), vec![(0, 3)]);
    }

    #[test]
    fn leftmost_longest_no_overlap() {
        let m = Matcher::from_phrases([
            "code de la route",
            "exces de vitesse",
            "tribunal de commerce",
        ]);
        // Deux collocations disjointes dans la même requête.
        let t = toks("code de la route exces de vitesse permis");
        assert_eq!(m.spans(&t), vec![(0, 4), (4, 7)]);
    }

    #[test]
    fn respects_token_boundaries() {
        let m = Matcher::from_phrases(["route"]);
        // « déroute » ne doit pas matcher « route » (frontière interne).
        let t = toks("deroute nationale");
        assert!(m.spans(&t).is_empty());
        // « route » seul matche.
        assert_eq!(m.spans(&toks("route nationale")), vec![(0, 1)]);
    }

    #[test]
    fn no_match_returns_empty() {
        let m = Matcher::from_phrases(["code de la route"]);
        assert!(m.spans(&toks("responsabilite commune voirie")).is_empty());
    }

    #[test]
    fn embedded_lexicon_loads_and_recognizes_stable_collocation() {
        // Le lexique embarqué se charge, l'automate se construit, et une
        // collocation à df énorme (présente dans tout lexique réel) est reconnue
        // sur les 3 tokens — garde aussi la parité de fold lexique ↔ query.
        assert!(lexicon_len() > 0, "lexique embarqué non vide");
        // Requête = la collocation seule (3 tokens) : aucun span plus long
        // possible, donc l'assertion ne dépend pas du reste du lexique.
        assert_eq!(
            collocation_spans(&toks("tribunal de commerce")),
            vec![(0, 3)]
        );
    }
}
