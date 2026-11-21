//! Moteur d'extraction des citations. L'automate porte les FORMES de citation et
//! les alias embarqués ; chaque ancre positionne un lexer sur un petit span qui
//! borne la mention, le span est snappé sur le catalogue, et le lien est délégué
//! à [`link_citation`].
//!
//! Deux phases strictement séparées :
//!
//! 1. **scan** — Aho-Corasick leftmost-longest sur texte plié longueur-stable,
//!    lexers positionnés par classe d'ancre → [`Tok`]s triés par position ;
//! 2. **compose** — rejoue le flux sans relire le texte : rattachement
//!    article→instrument, anaphores (« du même code »), antécédent validé par
//!    existence au catalogue, articles nus résolus par UNICITÉ.
//!
//! PUR : opère sur un [`LinkSnapshot`] et des [`CatalogText`] hydratés.

use std::collections::BTreeMap;
use std::sync::{LazyLock, OnceLock};

use regex::Regex;
use rustc_hash::{FxHashMap, FxHashSet};

use lj_core::address::CaseAddress;
use lj_core::docket_key::{POURVOI, ROLE, ROLE_SLASHED};

use crate::data::instrument_aliases;
use crate::data::LINK_ALIASES_TSV;
use crate::extract::common::ART_NUM_SUFFIX;
use crate::extract::common::FOREIGN_NATIONALITY_STEMS;
use crate::extract::common::{names_territory, territory_qualifier_len};
use crate::extract::key_signals::Citability;
use crate::extract::{normalize_article, normalize_instrument};
use crate::link::{link_citation_analyzed, KeyAnalysis};
use crate::link::{CatalogText, Forum, LinkSnapshot, LinkTarget};

use lj_fold::fold_chars;

/// `fold_chars(s).starts_with(prefix)` without allocating, `prefix` already folded.
fn starts_with_folded(s: &str, prefix: &str) -> bool {
    let mut it = s.chars().flat_map(lj_fold::fold_char);
    prefix.chars().all(|pc| it.next() == Some(pc))
}

/// A numeral in brackets right after an article's own, « l'article 23 (1) des statuts »: a
/// paragraph, not an article.
fn bracketed_paragraph(chars: &[char], s: usize, e: usize) -> bool {
    let mut before = chars[..s].iter().rev().filter(|c| **c != ' ');
    e - s <= 2
        && chars[s..e].iter().all(char::is_ascii_digit)
        && chars.get(e) == Some(&')')
        && before.next() == Some(&'(')
        && before.next().is_some_and(char::is_ascii_digit)
}

/// Pliage canonique de comparaison : `fold_chars` + espaces réduits à un.
/// Forme des clés de `full_titles` / `code_titles` et des candidats du walker.
fn canon_fold(s: &str) -> String {
    fold_chars(s)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// ── vocabulaire compilé ─────────────────────────────────────────────────────

/// Classe structurelle d'une ancre : décide du lexer positionné qui borne la
/// mention.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Anchor {
    /// « code … » / « livre … » : chaîne génitive snappée sur les titres de
    /// codes du catalogue (+ gentilé étranger pour le droit comparé).
    Code,
    /// Acte FR daté/numéroté : « loi n° 91-647 du 10 juillet 1991 ».
    Dated,
    /// Droit dérivé UE : « règlement (CE) n° 44/2001 », « directive
    /// 2008/115/CE » — le numéro à barre oblique est l'identité.
    Eu,
    /// Conventionnel : « convention/accord/traité… » + qualificatifs, lieu,
    /// matière, date — queue structurelle bornée par les mots de prose.
    Treaty,
    /// « Constitution » nue (majuscule exigée dans l'original) ou datée.
    Constitution,
    /// « article(s) », « art. » : tête d'énumération de numéraux.
    ArtWord,
    /// Connecteur d'anaphore (« du même », « dudit », « précité »…).
    SameConn,
    /// Référence BOFiP « BOI-IR-BASE-10-10 » : le code EST l'identité — il égale
    /// le `text_uid` catalogue. Majuscules exigées dans l'original.
    Boi,
    /// Citation de jurisprudence : juridiction, « pourvoi », « affaire », « RG »…
    /// — le lexer borne le span au token identifiant et dérive la clé pendante par
    /// famille. Flux séparé (`CompiledCase`), jamais composé avec les instruments.
    Case,
}

#[derive(Clone, Copy)]
enum Pat {
    /// Alias embarqué (link_aliases.tsv, instrument_aliases) : la surface pliée
    /// EST la clé de linking. `sigle` = surface courte (≤ 6 chars) exigée TOUT EN
    /// MAJUSCULES dans l'original.
    Alias {
        is_code: bool,
        sigle: bool,
    },
    Anchor(Anchor),
}

/// Rôle(s) d'un motif du flux unifié : une surface peut être à la fois
/// ancre/alias citations ET marqueur structurel — le même token porte les deux
/// rôles, chaque composeur filtre les siens. `mk_len` > 0 quand le rôle
/// marqueur vient de la clôture par préfixe : la surface du marqueur est le
/// préfixe de cette longueur (en octets pliés), et le gate (`marker_token`)
/// s'applique à SES frontières.
#[derive(Clone, Copy)]
struct Roles {
    cite: Option<Pat>,
    marker: Option<crate::scan::Mk>,
    mk_len: usize,
}

const ANCHORS: &[(&str, Anchor)] = &[
    ("code", Anchor::Code),
    ("nouveau code", Anchor::Code),
    ("livre", Anchor::Code),
    ("loi", Anchor::Dated),
    ("decret", Anchor::Dated),
    ("decret-loi", Anchor::Dated),
    ("ordonnance", Anchor::Dated),
    ("arrete", Anchor::Dated),
    ("deliberation", Anchor::Dated),
    ("circulaire", Anchor::Dated),
    ("boi", Anchor::Boi),
    ("reglement", Anchor::Eu),
    ("directive", Anchor::Eu),
    ("decision", Anchor::Eu),
    ("convention", Anchor::Treaty),
    ("accord", Anchor::Treaty),
    ("accord-cadre", Anchor::Treaty),
    ("traite", Anchor::Treaty),
    ("protocole", Anchor::Treaty),
    ("charte", Anchor::Treaty),
    ("pacte", Anchor::Treaty),
    ("declaration", Anchor::Treaty),
    ("constitution", Anchor::Constitution),
    ("article", Anchor::ArtWord),
    ("articles", Anchor::ArtWord),
    ("art.", Anchor::ArtWord),
    ("du meme", Anchor::SameConn),
    ("de la meme", Anchor::SameConn),
    ("du present", Anchor::SameConn),
    ("au present", Anchor::SameConn),
    ("de la presente", Anchor::SameConn),
    ("dudit", Anchor::SameConn),
    ("de ladite", Anchor::SameConn),
    ("de ce", Anchor::SameConn),
    ("de cette", Anchor::SameConn),
    ("son article", Anchor::SameConn),
    ("ses articles", Anchor::SameConn),
    ("precite", Anchor::SameConn),
    ("precitee", Anchor::SameConn),
    ("pourvoi", Anchor::Case),
    ("pourvois", Anchor::Case),
    ("cass.", Anchor::Case),
    ("cass", Anchor::Case),
    ("cass.com.", Anchor::Case),
    ("cass.civ.", Anchor::Case),
    ("cass.soc.", Anchor::Case),
    ("cass.crim.", Anchor::Case),
    ("cassation", Anchor::Case),
    ("cour de cassation", Anchor::Case),
    ("civ.", Anchor::Case),
    ("civ", Anchor::Case),
    ("soc.", Anchor::Case),
    ("soc", Anchor::Case),
    ("com.", Anchor::Case),
    ("crim.", Anchor::Case),
    ("ass. plen.", Anchor::Case),
    ("conseil d'etat", Anchor::Case),
    ("ce", Anchor::Case),
    ("conseil constitutionnel", Anchor::Case),
    ("cons. const.", Anchor::Case),
    ("tribunal des conflits", Anchor::Case),
    ("tc", Anchor::Case),
    ("cour europeenne des droits de l'homme", Anchor::Case),
    ("cjue", Anchor::Case),
    ("cjce", Anchor::Case),
    ("tue", Anchor::Case),
    ("cour de justice", Anchor::Case),
    ("cour de justice de l'union europeenne", Anchor::Case),
    ("cour de justice de 1'union europeenne", Anchor::Case),
    ("cour de justice des communautes europeennes", Anchor::Case),
    ("affaire", Anchor::Case),
    ("affaires", Anchor::Case),
    ("aff.", Anchor::Case),
    ("arret", Anchor::Case),
    ("arrets", Anchor::Case),
    ("jugement", Anchor::Case),
    ("jugements", Anchor::Case),
    ("requete", Anchor::Case),
    ("requetes", Anchor::Case),
    ("rg", Anchor::Case),
    ("r.g.", Anchor::Case),
    ("r.g", Anchor::Case),
    ("r. g", Anchor::Case),
    ("role", Anchor::Case),
    ("repertoire general", Anchor::Case),
    ("ca", Anchor::Case),
    ("deferee", Anchor::Case),
    ("enregistre", Anchor::Case),
    ("enregistree", Anchor::Case),
    ("enregistres", Anchor::Case),
    ("enregistrees", Anchor::Case),
    ("enrole", Anchor::Case),
    ("enrolee", Anchor::Case),
    ("enroles", Anchor::Case),
    ("enrolees", Anchor::Case),
];

/// L'automate structurel + les index de snap, bâtis UNE fois par run. Les
/// motifs sont STATIQUES (ancres + alias embarqués) ; seuls les index de
/// titres et l'index inversé des articles dépendent du catalogue.
pub struct CompiledVocab {
    /// `canon_fold(title | title_key)` → `title_key` — snap des titres épelés
    /// verbatim (borne EXACTE du span, text_key sans re-normalisation).
    full_titles: FxHashMap<String, String>,
    /// Sous-ensemble codes (« code … », « livre … ») : snap de la chaîne
    /// génitive des codes au plus long préfixe catalogue.
    code_titles: FxHashMap<String, String>,
    /// `num_key` → uids porteurs — résolution des articles nus par unicité.
    by_num_key: FxHashMap<String, Vec<String>>,
    /// Cross-decision cache of `link_citation_analyzed`, a pure function of
    /// `(instrument, text_key, article_key, owner_place)` over the run's snapshot.
    link_cache: std::sync::RwLock<FxHashMap<LinkCacheKey, LinkTarget>>,
}

/// Key of the cross-decision link cache: `(instrument, text_key, article_key, owner_place)`,
/// the citing body's place included.
type LinkCacheKey = (String, String, Option<String>, Option<String>);

/// Mots trop génériques pour être des surfaces d'alias seules (les ancres les
/// portent déjà, avec lexer).
const GENERIC_BARE: &[&str] = &[
    "accord",
    "convention",
    "declaration",
    "charte",
    "protocole",
    "reglement",
    "directive",
    "ordonnance",
    "code",
    "loi",
    "decret",
    "arrete",
    "circulaire",
    "deliberation",
    "traite",
    "avenant",
    "annexe",
    "decision",
];

const BLOCKED_SURFACES: &[&str] = &["conseil d'etat", "accord collectif", "loi du pays"];

/// Articles au sens jurisprudentiel univoque, cités nus dans toutes les
/// juridictions judiciaires (« condamne X au titre de l'article 700 »).
const CANON_BARE_ARTICLES: &[(&str, &str)] = &[
    ("700", "LEGITEXT000006070716"),
    ("699", "LEGITEXT000006070716"),
];

/// Automate FUSIONNÉ (statique, un par process) : ancres + alias embarqués
/// (citations) + marqueurs structurels (`crate::scan`). UN flux de tokens à
/// discipline de consommation unique : une passe leftmost-longest non
/// chevauchante, un motif = un token porteur de son (ses) rôle(s).
struct Fused {
    ac: aho_corasick::AhoCorasick,
    roles: Vec<Roles>,
}

fn fused() -> &'static Fused {
    static F: OnceLock<Fused> = OnceLock::new();
    F.get_or_init(|| {
        let mut patterns: Vec<String> = Vec::new();
        let mut roles: Vec<Roles> = Vec::new();
        let mut idx: FxHashMap<String, usize> = FxHashMap::default();
        for (s, class) in ANCHORS {
            idx.insert((*s).to_string(), patterns.len());
            patterns.push((*s).to_string());
            roles.push(Roles {
                cite: Some(Pat::Anchor(*class)),
                marker: None,
                mk_len: 0,
            });
        }
        let mut add_alias = |surface: &str, allow_short: bool| {
            let f = canon_fold(surface.trim());
            let min_len = if allow_short { 3 } else { 4 };
            if f.chars().count() < min_len
                || GENERIC_BARE.contains(&f.as_str())
                || BLOCKED_SURFACES.contains(&f.as_str())
                || idx.contains_key(&f)
                || RE_BARE_DATED_ALIAS.is_match(&f)
            {
                return;
            }
            let is_code = f.starts_with("code ") || f == "constitution";
            let sigle = f.chars().count() <= 6;
            idx.insert(f.clone(), patterns.len());
            patterns.push(f);
            roles.push(Roles {
                cite: Some(Pat::Alias { is_code, sigle }),
                marker: None,
                mk_len: 0,
            });
        };
        for line in LINK_ALIASES_TSV.lines() {
            if let Some(tk) = line.split('\t').next() {
                add_alias(tk, true);
            }
        }
        for alias in instrument_aliases().aliases.keys() {
            add_alias(alias, false);
        }
        let (mk_surfaces, mk_kinds) = crate::scan::marker_patterns();
        for (s, k) in mk_surfaces.iter().zip(&mk_kinds) {
            match idx.get(s.as_str()) {
                Some(&i) => {
                    if roles[i].marker.is_none() {
                        roles[i].marker = Some(*k);
                    }
                }
                None => {
                    idx.insert(s.clone(), patterns.len());
                    patterns.push(s.clone());
                    roles.push(Roles {
                        cite: None,
                        marker: Some(*k),
                        mk_len: 0,
                    });
                }
            }
        }
        // Clôture de rôles par préfixe : un motif citations qu'un marqueur préfixe à
        // la frontière de mot porte AUSSI ce rôle marqueur. Le gate (`marker_token`)
        // s'applique au token entier, en aval.
        let mk_prefix: FxHashMap<&str, crate::scan::Mk> = mk_surfaces
            .iter()
            .zip(&mk_kinds)
            .map(|(s, k)| (s.as_str(), *k))
            .collect();
        for (i, p) in patterns.iter().enumerate() {
            if roles[i].marker.is_some() {
                continue;
            }
            let b = p.as_bytes();
            for l in (1..p.len()).rev() {
                if !p.is_char_boundary(l)
                    || (b[l].is_ascii_alphanumeric() && b[l - 1].is_ascii_alphanumeric())
                {
                    continue;
                }
                if let Some(&k) = mk_prefix.get(&p[..l]) {
                    roles[i].marker = Some(k);
                    roles[i].mk_len = l;
                    break;
                }
            }
        }
        let ac = aho_corasick::AhoCorasick::builder()
            .match_kind(aho_corasick::MatchKind::LeftmostLongest)
            .kind(Some(aho_corasick::AhoCorasickKind::DFA))
            .build(&patterns)
            .expect("automate fusionné marqueurs + citations");
        Fused { ac, roles }
    })
}

impl CompiledVocab {
    /// Bâtit les index de snap catalogue (titres, articles) — l'automate est
    /// STATIQUE ([`fused`]).
    pub fn build(texts: &[CatalogText], snap: &LinkSnapshot) -> Self {
        let mut full_titles: FxHashMap<String, String> = FxHashMap::default();
        let mut code_titles: FxHashMap<String, String> = FxHashMap::default();
        for t in texts {
            let ftk = canon_fold(&t.title_key);
            if ftk.starts_with("code ") || ftk.starts_with("livre ") {
                code_titles
                    .entry(ftk.clone())
                    .or_insert_with(|| t.title_key.clone());
            }
            full_titles
                .entry(ftk)
                .or_insert_with(|| t.title_key.clone());
            let ft = canon_fold(&t.title);
            full_titles.entry(ft).or_insert_with(|| t.title_key.clone());
        }

        let mut by_num_key: FxHashMap<String, Vec<String>> = FxHashMap::default();
        for (uid, nks) in snap.article_sets() {
            for nk in nks {
                by_num_key
                    .entry(nk.clone())
                    .or_default()
                    .push(uid.to_string());
            }
        }
        // Triés pour le test d'appartenance par dichotomie.
        for v in by_num_key.values_mut() {
            v.sort_unstable();
            v.dedup();
        }

        Self {
            full_titles,
            code_titles,
            by_num_key,
            link_cache: std::sync::RwLock::new(FxHashMap::default()),
        }
    }

    pub fn stats(&self) -> String {
        format!(
            "surfaces={} titres={} codes={} num_keys={}",
            fused().roles.len(),
            self.full_titles.len(),
            self.code_titles.len(),
            self.by_num_key.len()
        )
    }
}

// ── flux de tokens (le « cache » par document) ──────────────────────────────

/// Un token typé et positionné (offsets en CHARS du texte original).
enum Tok {
    /// Mention d'instrument, bornée par son lexer. `weak` = mention purement
    /// structurelle sans identité chiffrée ni snap catalogue : n'émet un span que
    /// résolue. `treaty_short` = forme conventionnelle non datée, candidate à
    /// l'anaphore de préfixe. `nested` = mention imbriquée dans une autre :
    /// antécédent d'anaphore valide, jamais de span propre.
    Instr {
        s: usize,
        e: usize,
        text_key: String,
        is_code: bool,
        weak: bool,
        treaty_short: bool,
        nested: bool,
    },
    /// Numéral d'article (span = numéro, préfixe L/R/D inclus).
    Art {
        s: usize,
        e: usize,
        surface: String,
        num_key: String,
    },
    /// Anaphore « du même code / dudit règlement / de cette convention » —
    /// `nature` = le mot de nature cité (code, livre, loi, convention…).
    Same { s: usize, nature: String },
}

fn tok_start(t: &Tok) -> usize {
    match t {
        Tok::Instr { s, .. } | Tok::Art { s, .. } | Tok::Same { s, .. } => *s,
    }
}

/// Longueur maximale d'une mention lexée.
const MAX_SPAN: usize = 250;

/// Mots de prose qui n'apparaissent jamais À L'INTÉRIEUR d'un nom d'instrument
/// cité : bornes du walker, sur texte plié (« à » ⇒ « a » n'est PAS une borne,
/// seule la forme « a ete » l'est).
const WALK_STOP: &[&str] = &[
    "que",
    "qui",
    "dont",
    "lorsque",
    "puisque",
    "cependant",
    "tandis",
    "alors",
    "ainsi",
    "ne",
    "est",
    "sont",
    "etant",
    "etait",
    "etaient",
    "sera",
    "seront",
    "serait",
    "seraient",
    "fut",
    "au",
    "dans",
    "pour",
    "ou",
    "selon",
    "vise",
    "visee",
    "vises",
    "visees",
    "resulte",
    "resultent",
    "precise",
    "precisent",
    "enonce",
    "enoncent",
    "indique",
    "indiquent",
    "prescrit",
    "prescrivent",
    "prohibe",
    "prohibent",
    "stipule",
    "stipulent",
    "dispose",
    "disposent",
    "prevoit",
    "prevoient",
    "garantit",
    "garantissent",
    "enumere",
    "permet",
    "permettent",
    "dire",
    "doit",
    "doivent",
    "peut",
    "peuvent",
    "avait",
    "avaient",
    "ayant",
];

/// Mots pendants coupés en fin de span (connecteurs, participes d'apparat).
const TRAIL_TRIM: &[&str] = &[
    "de",
    "du",
    "des",
    "d'",
    "la",
    "le",
    "les",
    "l'",
    "et",
    "a",
    "aux",
    "sur",
    "entre",
    "en",
    "relative",
    "relatif",
    "relatives",
    "relatifs",
    "portant",
    "concernant",
    "faite",
    "fait",
    "signe",
    "signee",
    "conclue",
    "conclu",
    "adoptee",
    "adopte",
    "ouverte",
    "modifie",
    "modifiee",
    "precite",
    "precitee",
    "susvise",
    "susvisee",
];

/// Mots de nature : un « et » qui les introduit ferme la mention en cours.
const NATURE_WORDS: &[&str] = &[
    "code",
    "loi",
    "decret",
    "ordonnance",
    "arrete",
    "convention",
    "accord",
    "traite",
    "reglement",
    "directive",
    "decision",
    "charte",
    "protocole",
    "pacte",
    "declaration",
    "constitution",
    "circulaire",
    "deliberation",
];

fn is_stop_word(w: &str, next: Option<&str>) -> bool {
    if WALK_STOP.contains(&w) {
        return true;
    }
    if w.starts_with("qu'")
        || w.starts_with("n'")
        || w.starts_with("lorsqu'")
        || w.starts_with("puisqu'")
    {
        return true;
    }
    w == "a" && next == Some("ete")
}

/// Mots suivant `from` (offsets bytes sur `folded`), bornés à [`MAX_SPAN`] et
/// à la première ponctuation dure. Les mots de prose ([`is_stop_word`]) sont
/// tranchés par les appelants (certains lexers veulent la position du stop).
fn walk_words(folded: &str, from: usize) -> Vec<(usize, usize)> {
    let mut lim = folded.len().min(from + MAX_SPAN);
    while !folded.is_char_boundary(lim) {
        lim -= 1;
    }
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut cur: Option<usize> = None;
    for (off, c) in folded[from..lim].char_indices() {
        let hard_stop = matches!(
            c,
            ',' | ';' | ':' | '.' | '(' | ')' | '!' | '?' | '"' | '«' | '»'
        );
        if c == ' ' || hard_stop {
            if let Some(ws) = cur.take() {
                out.push((from + ws, from + off));
            }
            if hard_stop {
                return out;
            }
        } else if cur.is_none() {
            cur = Some(off);
        }
    }
    if let Some(ws) = cur {
        out.push((from + ws, lim));
    }
    out
}

/// Résultat d'un lexer d'ancre.
struct Mention {
    /// Fin de mention (byte sur `folded`).
    e: usize,
    text_key: String,
    is_code: bool,
    weak: bool,
    treaty_short: bool,
}

/// Gentilé étranger APRÈS la mention (« code civil suisse ») : étend la
/// surface. Renvoie la nouvelle fin de span si extension.
fn gentile_ext(folded: &str, be: usize) -> Option<usize> {
    let tail = &folded[be..];
    let next_word: String = tail
        .chars()
        .skip_while(|c| *c == ' ')
        .take_while(|c| c.is_ascii_alphabetic() || *c == '-')
        .collect();
    if !next_word.is_empty()
        && FOREIGN_NATIONALITY_STEMS
            .iter()
            .any(|st| next_word.starts_with(st))
    {
        let skip = tail.chars().take_while(|c| *c == ' ').count();
        Some(be + skip + next_word.len())
    } else {
        None
    }
}

/// Territory named after the title (« code de procédure civile de la
/// Polynésie française »): the mention extends over it, as over a foreign
/// demonym.
fn territory_ext(folded: &str, be: usize) -> Option<usize> {
    territory_qualifier_len(&folded[be..]).map(|len| be + len)
}

/// Surface originale d'un span plié (offsets bytes → chars).
fn span_surface(chars: &[char], byte2char: &[usize], bs: usize, be: usize) -> String {
    chars[byte2char[bs]..byte2char[be]].iter().collect()
}

/// `normalize_instrument` mémoïsé par span plié : fonction PURE de la surface.
static NORM_KEYS: OnceLock<std::sync::RwLock<FxHashMap<String, String>>> = OnceLock::new();

fn norm_key(memo: &mut FxHashMap<String, String>, folded_span: &str, surface: &str) -> String {
    if let Some(k) = memo.get(folded_span) {
        return k.clone();
    }
    let cache = NORM_KEYS.get_or_init(|| std::sync::RwLock::new(FxHashMap::default()));
    let hit = cache.read().unwrap().get(surface).cloned();
    let k = hit.unwrap_or_else(|| {
        let k = normalize_instrument(surface);
        cache
            .write()
            .unwrap()
            .entry(surface.to_string())
            .or_insert_with(|| k.clone());
        k
    });
    memo.insert(folded_span.to_string(), k.clone());
    k
}

/// Plus long titre catalogue épelé verbatim depuis `bs` : candidats aux
/// frontières de mots (joints en espaces simples = `canon_fold`), bornés par
/// les mots de prose. Renvoie (fin byte, title_key).
fn longest_title_end<'a>(
    map: &'a FxHashMap<String, String>,
    folded: &str,
    bs: usize,
    be: usize,
    skip_head_words: usize,
) -> Option<(usize, &'a String)> {
    let words = walk_words(folded, be);
    let mut cand = String::with_capacity(64);
    for (i, w) in folded[bs..be].split_whitespace().enumerate() {
        if i >= skip_head_words {
            if !cand.is_empty() {
                cand.push(' ');
            }
            cand.push_str(w);
        }
    }
    let mut hit: Option<(usize, &String)> = None;
    for (i, (ws, we)) in words.iter().enumerate() {
        let w = &folded[*ws..*we];
        let next = words.get(i + 1).map(|(s, e)| &folded[*s..*e]);
        if is_stop_word(w, next) {
            break;
        }
        cand.push(' ');
        cand.push_str(w);
        if let Some(tk) = map.get(&cand) {
            hit = Some((*we, tk));
        }
    }
    hit
}

/// Recule la fin de mention sur les mots pendants ([`TRAIL_TRIM`]).
fn trim_trailing(folded: &str, be: usize, mut end: usize) -> usize {
    loop {
        let span = folded[be..end].trim_end();
        end = be + span.len();
        let Some(last) = span.rsplit(' ').next() else {
            return end;
        };
        if !TRAIL_TRIM.contains(&last) || last.len() >= span.len() {
            return end;
        }
        end -= last.len();
    }
}

/// « code / livre / nouveau code … » : snap au plus long titre de code du
/// catalogue, gentilé étranger, sinon chaîne structurelle (mention faible).
fn lex_code(
    vocab: &CompiledVocab,
    folded: &str,
    bs: usize,
    be: usize,
) -> Option<(Mention, bool /* catalogue hit */)> {
    let skip = usize::from(&folded[bs..be] == "nouveau code");
    let hit = longest_title_end(&vocab.code_titles, folded, bs, be, skip);
    if let Some((e, tk)) = hit {
        if let Some(e2) = gentile_ext(folded, e).or_else(|| territory_ext(folded, e)) {
            return Some((
                Mention {
                    e: e2,
                    text_key: String::new(),
                    is_code: false,
                    weak: false,
                    treaty_short: false,
                },
                false,
            ));
        }
        return Some((
            Mention {
                e,
                text_key: tk.clone(),
                is_code: true,
                weak: false,
                treaty_short: false,
            },
            true,
        ));
    }
    let words = walk_words(folded, be);
    let mut end = be;
    for (i, (ws, we)) in words.iter().enumerate() {
        let w = &folded[*ws..*we];
        let next = words.get(i + 1).map(|(s, e)| &folded[*s..*e]);
        let bare = w.trim_start_matches("l'").trim_start_matches("d'");
        if is_stop_word(w, next)
            || matches!(
                w,
                "le" | "la"
                    | "les"
                    | "un"
                    | "une"
                    | "ce"
                    | "cette"
                    | "son"
                    | "sa"
                    | "ses"
                    | "leur"
                    | "leurs"
            )
            || matches!(bare, "article" | "articles")
        {
            break;
        }
        end = *we;
        if FOREIGN_NATIONALITY_STEMS.iter().any(|st| w.starts_with(st)) {
            break;
        }
    }
    let end = trim_trailing(folded, be, end);
    (end > be).then_some((
        Mention {
            e: end,
            text_key: String::new(),
            is_code: true,
            weak: true,
            treaty_short: false,
        },
        false,
    ))
}

/// Acte FR daté/numéroté : qualificatifs d'identité, n°, date, puis extension
/// éventuelle au titre catalogue épelé verbatim (queue officielle).
fn lex_dated(vocab: &CompiledVocab, folded: &str, bs: usize, be: usize) -> Option<Mention> {
    let mut pos = be;
    if &folded[bs..be] == "circulaire" {
        if let Some(c) = RE_LEX_CIRC_AUTHOR.captures(&folded[be..]) {
            pos += c.get(1).expect("author group").end();
        }
    }
    if !case_window(folded, pos, 40)
        .bytes()
        .any(|b| b.is_ascii_digit())
    {
        return None;
    }
    if let Some(m) = RE_LEX_DATED_QUAL.find(&folded[pos..]) {
        pos += m.end();
    }
    let mut has_id = false;
    if let Some(e) = lex_nor_at(folded, pos) {
        pos = e;
        has_id = true;
    }
    if let Some(m) = RE_LEX_NUM.find(&folded[pos..]) {
        pos += m.end();
        has_id = true;
    }
    if let Some(m) = RE_LEX_DATE.find(&folded[pos..]) {
        pos += m.end();
        has_id = true;
    }
    if has_id {
        if let Some(e) = lex_nor_at(folded, pos) {
            pos = e;
        }
    }
    if has_id {
        if let Some((e, tk)) = longest_title_end(&vocab.full_titles, folded, bs, be, 0) {
            if e > pos {
                return Some(Mention {
                    e,
                    text_key: tk.clone(),
                    is_code: false,
                    weak: false,
                    treaty_short: false,
                });
            }
        }
    }
    has_id.then_some(Mention {
        e: pos,
        text_key: String::new(),
        is_code: false,
        weak: false,
        treaty_short: false,
    })
}

/// Token NOR à `pos` : « nor jusk1140023c », « nor : intk9700174c »,
/// « norintk1207286c » (graphie collée), « (nor : devp1134619c) ». Retourne la
/// fin (fermante incluse si l'ouvrante a été consommée), frontière franche
/// exigée.
fn lex_nor_at(folded: &str, pos: usize) -> Option<usize> {
    let m = RE_LEX_NOR.captures(&folded[pos..])?;
    let mut end = pos + m.get(0).expect("match complet").end();
    if end < folded.len() && folded.as_bytes()[end].is_ascii_alphanumeric() {
        return None;
    }
    if m.get(1).is_some() {
        if let Some(close) = RE_LEX_NOR_CLOSE.find(&folded[end..]) {
            end += close.end();
        }
    }
    Some(end)
}

static RE_LEX_NOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+(\(\s*)?nor\s*:?\s*[a-z]{4}\d{7}[a-z]").unwrap());
static RE_LEX_NOR_CLOSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\)").unwrap());

/// Droit dérivé UE : le numéro à barre oblique est l'identité. Sans lui, la
/// mention nue génitive suivie de ponctuation est une anaphore (« les
/// dispositions du règlement, »).
enum EuLex {
    Instr(Mention),
    Same(usize),
    None,
}

fn lex_eu(vocab: &CompiledVocab, folded: &str, bs: usize, be: usize) -> EuLex {
    let eu_id = case_window(folded, be, 48)
        .bytes()
        .any(|b| b.is_ascii_digit())
        .then(|| RE_LEX_EU.find(&folded[be..]))
        .flatten();
    if let Some(m) = eu_id {
        let pos = be + m.end();
        if let Some((e, tk)) = longest_title_end(&vocab.full_titles, folded, bs, be, 0) {
            if e > pos {
                return EuLex::Instr(Mention {
                    e,
                    text_key: tk.clone(),
                    is_code: false,
                    weak: false,
                    treaty_short: false,
                });
            }
        }
        return EuLex::Instr(Mention {
            e: pos,
            text_key: String::new(),
            is_code: false,
            weak: false,
            treaty_short: false,
        });
    }
    if matches!(&folded[bs..be], "reglement" | "directive") {
        if let Some(m) = RE_LEX_EU_DATED.find(&folded[be..]) {
            let mut end = be + m.end();
            for (ws, we) in walk_words(folded, end) {
                let w = &folded[ws..we];
                if is_stop_word(w, None) || NATURE_WORDS.contains(&w) {
                    break;
                }
                end = we;
            }
            let end = trim_trailing(folded, be + m.end(), end);
            return EuLex::Instr(Mention {
                e: end,
                text_key: String::new(),
                is_code: false,
                weak: true,
                treaty_short: false,
            });
        }
    }
    let word = &folded[bs..be];
    let head = folded[..bs].trim_end();
    let bare_conn = (word == "reglement" && head.ends_with(" du"))
        || (word == "directive" && head.ends_with(" de la"));
    if bare_conn
        && folded[be..]
            .trim_start()
            .starts_with([',', '.', ';', ':', ')'])
    {
        let conn = if word == "reglement" { "du" } else { "de la" };
        return EuLex::Same(head.len() - conn.len());
    }
    EuLex::None
}

/// Conventionnel : qualificatifs + lieu/matière/date, queue bornée par les
/// mots de prose ; snap au titre catalogue verbatim quand il est épelé.
fn lex_treaty(vocab: &CompiledVocab, folded: &str, bs: usize, be: usize) -> Option<Mention> {
    let words = walk_words(folded, be);
    let mut end = be;
    let mut cand = folded[bs..be].to_string();
    let mut title_hit: Option<(usize, &String)> = None;
    let mut prev = "";
    for (i, (ws, we)) in words.iter().enumerate() {
        let w = &folded[*ws..*we];
        let next = words.get(i + 1).map(|(s, e)| &folded[*s..*e]);
        if is_stop_word(w, next) {
            break;
        }
        let bare = w.trim_start_matches("l'").trim_start_matches("d'");
        if matches!(prev, "et" | "ou")
            && (NATURE_WORDS.contains(&bare)
                || matches!(bare, "article" | "articles")
                || bare.starts_with(|c: char| c.is_ascii_digit()))
        {
            break;
        }
        if matches!(bare, "article" | "articles" | "art.") {
            break;
        }
        cand.push(' ');
        cand.push_str(w);
        if let Some(tk) = vocab.full_titles.get(&cand) {
            title_hit = Some((*we, tk));
        }
        end = *we;
        prev = w;
    }
    let end = trim_trailing(folded, be, end);
    let end = match title_hit {
        Some((e, _)) if e > end => e,
        _ => end,
    };
    if end <= be {
        return None;
    }
    let text_key = title_hit
        .filter(|(e, _)| *e == end)
        .map(|(_, tk)| tk.clone())
        .unwrap_or_default();
    let weak = title_hit.is_none() && !folded[bs..end].bytes().any(|b| b.is_ascii_digit());
    Some(Mention {
        e: end,
        text_key,
        is_code: false,
        weak,
        treaty_short: weak,
    })
}

/// « Constitution » : majuscule exigée dans l'original, date optionnelle.
fn lex_constitution(
    folded: &str,
    chars: &[char],
    byte2char: &[usize],
    bs: usize,
    be: usize,
) -> Option<Mention> {
    if chars[byte2char[bs]] != 'C' {
        return None;
    }
    let mut pos = be;
    if let Some(m) = RE_LEX_DATE.find(&folded[pos..]) {
        pos += m.end();
    } else if let Some(m) = RE_LEX_CONST_YEAR.find(&folded[pos..]) {
        pos += m.end();
    }
    Some(Mention {
        e: pos,
        text_key: String::new(),
        is_code: true,
        weak: false,
        treaty_short: false,
    })
}

// ── citations de jurisprudence ──────────────────────────────────────────────

/// Citation de jurisprudence : span borné au token identifiant (« n°
/// [18-23.954] », « [C-561/19] », « RG n° [21/04532] ») et clé pendante par
/// famille — `cc|1823954`, `ce|412412`, `constit|2020-800`, `cjue|c-561/19`,
/// `cedh|30010/10`, `rg|{jurisdiction_code}|21/04532`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledCase {
    pub char_start: usize,
    pub char_end: usize,
    pub address: CaseAddress,
}

/// Fenêtre avale bornée (frontière de char sûre) pour les sondes de contexte.
fn case_window(folded: &str, be: usize, len: usize) -> &str {
    let mut end = (be + len).min(folded.len());
    while !folded.is_char_boundary(end) {
        end += 1;
    }
    &folded[be..end]
}

fn push_case(
    out: &mut Vec<CompiledCase>,
    byte2char: &[usize],
    abs_s: usize,
    abs_e: usize,
    address: CaseAddress,
) {
    out.push(CompiledCase {
        char_start: byte2char[abs_s],
        char_end: byte2char[abs_e],
        address,
    });
}

/// Référence BOFiP « BOI-IR-BASE-10-10 » : segments lettres/chiffres après
/// « BOI- ». Majuscules exigées dans l'original ; la clé émise est le code
/// remonté en majuscules — il égale le `text_uid` BOFiP du catalogue, résolu
/// par alias direct du snapshot.
fn lex_boi(
    folded: &str,
    chars: &[char],
    byte2char: &[usize],
    bs: usize,
    be: usize,
) -> Option<Mention> {
    let m = RE_LEX_BOI.find(&folded[be..])?;
    if m.start() != 0 {
        return None;
    }
    let end = be + m.end();
    if end < folded.len() && folded.as_bytes()[end].is_ascii_alphanumeric() {
        return None;
    }
    if chars[byte2char[bs]..byte2char[be]]
        .iter()
        .any(|c| c.is_lowercase())
    {
        return None;
    }
    let mut key = folded[bs..end].to_uppercase();
    if let Some(m) = RE_LEX_BOI_DATED.find(&key) {
        key.truncate(m.start());
    }
    Some(Mention {
        e: end,
        text_key: key,
        is_code: false,
        weak: false,
        treaty_short: false,
    })
}

static RE_LEX_BOI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:-[a-z0-9]{1,20})+").unwrap());
static RE_LEX_BOI_DATED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-\d{8}$").unwrap());

fn lex_case(
    folded: &str,
    chars: &[char],
    byte2char: &[usize],
    chrono: Option<&crate::chrono::ChronoSnapshot>,
    bs: usize,
    be: usize,
    out: &mut Vec<CompiledCase>,
) {
    let upper = |n: usize| {
        let ci = byte2char[bs];
        chars[ci..].iter().take(n).all(|c| c.is_uppercase())
    };
    match &folded[bs..be] {
        "pourvoi" | "pourvois" => case_cc(folded, byte2char, be, out, true),
        "cass." | "cass" | "cassation" | "cour de cassation" | "cass.com." | "cass.civ."
        | "cass.soc." | "cass.crim." => case_cc(folded, byte2char, be, out, false),
        "civ." | "civ" | "soc." | "soc" | "com." | "crim." | "ass. plen." => {
            if upper(1) {
                case_cc(folded, byte2char, be, out, false);
            }
        }
        "conseil d'etat" => case_ce(folded, byte2char, bs, be, out),
        "ce" => {
            if upper(2) {
                case_ce(folded, byte2char, bs, be, out);
            }
        }
        "conseil constitutionnel" => case_constit(folded, byte2char, bs, be, out, false),
        "cons. const." => case_constit(folded, byte2char, bs, be, out, false),
        "tribunal des conflits" => case_tc(folded, byte2char, bs, be, out),
        "tc" => {
            if upper(2) {
                case_tc(folded, byte2char, bs, be, out);
            }
        }
        "cour europeenne des droits de l'homme" => {
            case_cedh(folded, byte2char, be, out, false);
        }
        "requete" | "requetes" => {
            if !case_cedh(folded, byte2char, be, out, true) {
                let win = case_window(folded, be, 40);
                if let Some(c) = RE_CASE_REF_CE.captures(win) {
                    let g = c.get(1).expect("groupe n° CE");
                    if !cut_by_window(folded, be, win, g.end()) {
                        let key = CaseAddress::new("ce", g.as_str(), "");
                        push_case(out, byte2char, be + g.start(), be + g.end(), key);
                    }
                }
            }
        }
        "affaire" | "affaires" | "aff." => case_cjue_aff(folded, chars, byte2char, be, out),
        "cjue"
        | "cjce"
        | "cour de justice"
        | "cour de justice de l'union europeenne"
        | "cour de justice de 1'union europeenne"
        | "cour de justice des communautes europeennes" => {
            case_cjue_window(folded, chars, byte2char, be, out);
        }
        "tue" => {
            if upper(3) {
                case_cjue_window(folded, chars, byte2char, be, out);
            }
        }
        "arret" | "arrets" => {
            if case_window(folded, be, 60)
                .bytes()
                .any(|b| b.is_ascii_digit())
            {
                case_cjue_window(folded, chars, byte2char, be, out);
                case_cc(folded, byte2char, be, out, false);
                case_cedh_arret(folded, chars, byte2char, be, out);
                case_admin(folded, chars, byte2char, chrono, bs, be, out);
                case_ce_arret(folded, byte2char, bs, be, out);
                case_arret_ord(folded, chars, byte2char, be, out);
            }
        }
        "jugement" | "jugements" => {
            case_admin(folded, chars, byte2char, chrono, bs, be, out);
        }
        "rg" | "r.g." | "r.g" | "r. g" => {
            case_rg(folded, chars, byte2char, chrono, bs, be, out);
        }
        "role" => {
            if upper(1) {
                case_rg(folded, chars, byte2char, chrono, bs, be, out);
            }
        }
        "repertoire general" => case_rg(folded, chars, byte2char, chrono, bs, be, out),
        "ca" => {
            if upper(2) {
                case_ca_sigle(folded, chars, byte2char, chrono, bs, be, out);
            }
        }
        "deferee" => case_deferee(folded, chars, byte2char, chrono, bs, be, out),
        "enregistre" | "enregistree" | "enregistres" | "enregistrees" | "enrole" | "enrolee"
        | "enroles" | "enrolees" => {
            case_enrol(folded, chars, byte2char, chrono, bs, be, out);
        }
        _ => {}
    }
}

/// Une capture qui touche la fin de fenêtre alors que le texte continue en
/// chiffres est TRONQUÉE (« n° 498887 » coupé en « 49888 ») : pas un token.
fn cut_by_window(folded: &str, be: usize, win: &str, end: usize) -> bool {
    end == win.len()
        && folded[be + end..]
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_digit())
}

/// Digits only, the normalisation the resolution probe applies to the stored
/// graphy: `18-23.954` and `1823954` must reach the same decision.
fn cc_key(num: &str) -> CaseAddress {
    let digits: String = num.chars().filter(char::is_ascii_digit).collect();
    CaseAddress::new("cc", &digits, "")
}

fn case_cc(
    folded: &str,
    byte2char: &[usize],
    be: usize,
    out: &mut Vec<CompiledCase>,
    anchored: bool,
) {
    let win = case_window(folded, be, if anchored { 120 } else { 200 });
    let re = if anchored {
        &RE_CASE_POURVOI
    } else {
        &RE_CASE_POURVOI_WIN
    };
    if let Some(c) = re.captures(win) {
        let g = c.get(1).expect("groupe pourvoi");
        if cut_by_window(folded, be, win, g.end()) {
            return;
        }
        push_case(
            out,
            byte2char,
            be + g.start(),
            be + g.end(),
            cc_key(g.as_str()),
        );
        let mut pos = g.end();
        while let Some(c) = RE_CASE_POURVOI_NEXT.captures(&win[pos..]) {
            let g = c.get(1).expect("groupe pourvoi");
            if cut_by_window(folded, be, win, pos + g.end()) {
                break;
            }
            let (s, e) = (be + pos + g.start(), be + pos + g.end());
            push_case(out, byte2char, s, e, cc_key(g.as_str()));
            pos += g.end();
        }
        return;
    }
    let ce_probe = if anchored {
        RE_CASE_CE_WIN
            .captures(win)
            .filter(|c| c.get(0).expect("match CE").start() < 25)
    } else {
        RE_CASE_CASSATION_CE.captures(win)
    };
    if let Some(c) = ce_probe {
        let g = c.get(1).expect("groupe n° CE");
        if !cut_by_window(folded, be, win, g.end()) {
            push_ce_enum(folded, byte2char, be, win, &c, out);
        }
    }
}

/// Émet un n° CE + son énumération (« n° 48296, 448305, 454144 et 455519 »).
fn push_ce_enum(
    folded: &str,
    byte2char: &[usize],
    off: usize,
    win: &str,
    c: &regex::Captures,
    out: &mut Vec<CompiledCase>,
) {
    let g = c.get(1).expect("groupe n° CE");
    push_case(
        out,
        byte2char,
        off + g.start(),
        off + g.end(),
        CaseAddress::new("ce", g.as_str(), ""),
    );
    let mut pos = g.end();
    while let Some(c) = RE_CASE_CE_NEXT.captures(&win[pos..]) {
        let g = c.get(1).expect("groupe n° CE");
        if cut_by_window(folded, off, win, pos + g.end()) {
            break;
        }
        let (s, e) = (off + pos + g.start(), off + pos + g.end());
        push_case(out, byte2char, s, e, CaseAddress::new("ce", g.as_str(), ""));
        pos += g.end();
    }
}

fn case_ce(folded: &str, byte2char: &[usize], bs: usize, be: usize, out: &mut Vec<CompiledCase>) {
    let mut ws = bs.saturating_sub(160);
    while !folded.is_char_boundary(ws) {
        ws += 1;
    }
    let up = &folded[ws..bs];
    if let Some(c) = RE_CASE_CE_BACK.captures_iter(up).last() {
        let g = c.get(1).expect("groupe n° CE");
        let mut nums = vec![(g.start(), g.end())];
        let mut pos = g.end();
        while let Some(c) = RE_CASE_CE_NEXT.captures(&up[pos..]) {
            let g = c.get(1).expect("groupe n° CE");
            nums.push((pos + g.start(), pos + g.end()));
            pos += g.end();
        }
        let tail = &up[nums.last().expect("non vide").1..];
        if !RE_CASE_OTHER_JUR.is_match(tail) {
            for (s, e) in nums {
                let key = CaseAddress::new("ce", &up[s..e], "");
                push_case(out, byte2char, ws + s, ws + e, key);
            }
        }
    }
    let win = case_window(folded, be, 100);
    let Some(c) = RE_CASE_CE_WIN.captures(win) else {
        return;
    };
    let g = c.get(1).expect("groupe n° CE");
    if cut_by_window(folded, be, win, g.end()) {
        return;
    }
    push_ce_enum(folded, byte2char, be, win, &c, out);
}

fn case_constit(
    folded: &str,
    byte2char: &[usize],
    bs: usize,
    be: usize,
    out: &mut Vec<CompiledCase>,
    anchored: bool,
) {
    let mut ws = bs.saturating_sub(80);
    while !folded.is_char_boundary(ws) {
        ws += 1;
    }
    if let Some(c) = RE_CASE_CONSTIT_BACK.captures(&folded[ws..bs]) {
        let g = c.get(1).expect("groupe n° constit");
        // Clé sans le suffixe DC/QPC : les `docket_numbers` CONSTIT sont nus.
        let key = CaseAddress::new("constit", g.as_str(), "");
        push_case(out, byte2char, ws + g.start(), ws + g.end(), key);
    }
    let win = case_window(folded, be, 100);
    let re = if anchored {
        &RE_CASE_CONSTIT
    } else {
        &RE_CASE_CONSTIT_WIN
    };
    let Some(c) = re.captures(win) else { return };
    let g = c.get(1).or_else(|| c.get(2)).expect("groupe n° constit");
    push_case(
        out,
        byte2char,
        be + g.start(),
        be + g.end(),
        CaseAddress::new("constit", g.as_str(), ""),
    );
    let mut pos = g.end();
    while let Some(c) = RE_CASE_CONSTIT_NEXT.captures(&win[pos..]) {
        let g = c.get(1).expect("groupe n° constit");
        let (s, e) = (be + pos + g.start(), be + pos + g.end());
        push_case(
            out,
            byte2char,
            s,
            e,
            CaseAddress::new("constit", g.as_str(), ""),
        );
        pos += g.end();
    }
}

/// Clé Tribunal des conflits : espaces et points ôtés du texte plié. « C 4174 »
/// tombe sur `c4174`, « 14-03.959 » sur `14-03959`, graphie année-numéro.
fn tc_key(num: &str) -> CaseAddress {
    CaseAddress::new("tc", &lj_core::docket_key::tc_ordinal(num), "")
}

/// Citation du Tribunal des conflits, « TC, 8 février 1873, [U], n° 00012 » ou
/// « TC, 7 juillet 2014, pourvoi n° 14-03.959 » : le tribunal écrit « pourvoi »
/// là où il tranche un conflit, et la clé reste `tc`.
///
/// La date est le seul discriminant, jamais la place du numéro : sans elle,
/// « TC de Caen » est un tribunal de commerce et « TC n° 2 » un terme de
/// comparaison d'expropriation.
fn case_tc(folded: &str, byte2char: &[usize], bs: usize, be: usize, out: &mut Vec<CompiledCase>) {
    let win = case_window(folded, be, 200);
    let date = RE_CASE_TC_DATE.find(win);
    if let Some(d) = date {
        if tc_ahead(folded, byte2char, be, win, d.end(), out) {
            return;
        }
    } else if let Some(c) = RE_CASE_TC_NUM_DATED.captures(win) {
        let g = c.get(1).expect("groupe n° TC");
        if !tc_cut(folded, be, win, g.end()) {
            push_case(
                out,
                byte2char,
                be + g.start(),
                be + g.end(),
                tc_key(g.as_str()),
            );
            return;
        }
    }
    tc_back(folded, byte2char, bs, date.is_some(), out);
}

/// Le millésime d'un décret visé derrière la citation (« n° 2015-233 »), ou un
/// numéro que la fenêtre a coupé : le numéro du fonds ne porte ni tiret de
/// millésime ni barre de rôle.
fn tc_cut(folded: &str, be: usize, win: &str, e: usize) -> bool {
    cut_by_window(folded, be, win, e) || folded[be + e..].starts_with(['-', '/'])
}

/// Les numéros derrière la date : le premier, puis ceux que l'énumération
/// enchaîne, « n° 3789, 3790, 3791 » comme « n° 03086, du 12 décembre 2005,
/// Commune de Cestas, n° 3485 ».
fn tc_ahead(
    folded: &str,
    byte2char: &[usize],
    be: usize,
    win: &str,
    from: usize,
    out: &mut Vec<CompiledCase>,
) -> bool {
    let Some(c) = RE_CASE_TC_NUM.captures(&win[from..]) else {
        return false;
    };
    let g = c.get(1).expect("groupe n° TC");
    let mut pos = from + g.end();
    if tc_cut(folded, be, win, pos) {
        return false;
    }
    push_case(
        out,
        byte2char,
        be + from + g.start(),
        be + pos,
        tc_key(g.as_str()),
    );
    while let Some(c) = tc_next(&win[pos..]) {
        let g = c.get(1).expect("groupe n° TC");
        let (s, e) = (pos + g.start(), pos + g.end());
        if tc_cut(folded, be, win, e) {
            break;
        }
        push_case(out, byte2char, be + s, be + e, tc_key(g.as_str()));
        pos = e;
    }
    true
}

/// Maillon suivant de l'énumération. Le maillon nu, sans « n° », n'est retenu
/// que devant une ponctuation de liste : « n° 4283, 1500 euros » n'énumère pas.
fn tc_next(rest: &str) -> Option<regex::Captures<'_>> {
    if let Some(c) = RE_CASE_TC_NEXT_DATED.captures(rest) {
        return Some(c);
    }
    let c = RE_CASE_TC_NEXT_BARE.captures(rest)?;
    let after = rest[c.get(1).expect("groupe n° TC").end()..].trim_start();
    let liste = after.is_empty()
        || after.starts_with([',', ';', ')', '.'])
        || after
            .strip_prefix("et")
            .is_some_and(|r| !r.starts_with(char::is_alphanumeric));
    liste.then_some(c)
}

/// Le numéro devant l'ancre : « Vu la décision n° 3124 du Tribunal des Conflits
/// du 23 novembre 1998 ». La fenêtre s'arrête à la ponctuation de phrase, et la
/// date reste exigée, portée par l'arrière ou par l'avant.
fn tc_back(
    folded: &str,
    byte2char: &[usize],
    bs: usize,
    date_ahead: bool,
    out: &mut Vec<CompiledCase>,
) {
    let mut ws = bs.saturating_sub(90);
    while !folded.is_char_boundary(ws) {
        ws += 1;
    }
    if let Some(i) = folded[ws..bs].rfind(['.', ';', ':']) {
        ws += i + 1;
    }
    let Some(c) = RE_CASE_TC_BACK.captures(&folded[ws..bs]) else {
        return;
    };
    if c.get(2).is_none() && !date_ahead {
        return;
    }
    let g = c.get(1).expect("groupe n° TC");
    let (s, e) = (ws + g.start(), ws + g.end());
    if folded[e..].starts_with(['-', '/']) {
        return;
    }
    push_case(out, byte2char, s, e, tc_key(g.as_str()));
}

/// Une requête CEDH suivie d'une barre oblique est un acte UE gaufré
/// (« 2004/38/CE ») — pas une requête.
fn cedh_slash_after(folded: &str, abs_e: usize) -> bool {
    folded[abs_e..].bytes().next() == Some(b'/')
}

fn case_cedh(
    folded: &str,
    byte2char: &[usize],
    be: usize,
    out: &mut Vec<CompiledCase>,
    anchored: bool,
) -> bool {
    let win = case_window(folded, be, 120);
    let re = if anchored {
        &RE_CASE_CEDH
    } else {
        &RE_CASE_CEDH_WIN
    };
    let Some(c) = re.captures(win) else {
        return false;
    };
    let g = c.get(1).expect("groupe requête CEDH");
    if cedh_slash_after(folded, be + g.end()) {
        return false;
    }
    push_cedh(out, byte2char, be + g.start(), be + g.end(), g.as_str());
    let mut pos = g.end();
    while let Some(c) = RE_CASE_CEDH_NEXT.captures(&win[pos..]) {
        let g = c.get(1).expect("groupe requête CEDH");
        if cedh_slash_after(folded, be + pos + g.end()) {
            break;
        }
        let (s, e) = (be + pos + g.start(), be + pos + g.end());
        push_cedh(out, byte2char, s, e, g.as_str());
        pos += g.end();
    }
    true
}

fn push_cedh(out: &mut Vec<CompiledCase>, byte2char: &[usize], s: usize, e: usize, num: &str) {
    let num: String = num.chars().filter(|ch| !ch.is_whitespace()).collect();
    push_case(out, byte2char, s, e, CaseAddress::new("cedh", &num, ""));
}

/// « son arrêt n° 29217/12, Tarakhel c./ Suisse » : requête CEDH derrière
/// « arrêt » — 4-5 chiffres avant la barre, n° exigé.
fn case_cedh_arret(
    folded: &str,
    chars: &[char],
    byte2char: &[usize],
    be: usize,
    out: &mut Vec<CompiledCase>,
) {
    let win = case_window(folded, be, 100);
    let Some(c) = RE_CASE_CEDH_ARRET.captures(win) else {
        return;
    };
    let g = c.get(1).expect("groupe requête CEDH");
    if cedh_slash_after(folded, be + g.end()) || cut_by_window(folded, be, win, g.end()) {
        return;
    }
    let m = c.get(0).expect("match");
    if RE_CASE_EU_ACT_BEFORE.is_match(&win[..m.start()]) {
        return;
    }
    let year_first = matches!(&g.as_str()[..2], "19" | "20");
    if year_first {
        let num = rg_num_key(chars, byte2char, be + g.start(), be + g.end());
        push_case(
            out,
            byte2char,
            be + g.start(),
            be + g.end(),
            CaseAddress::new("", &num, ""),
        );
    } else {
        push_cedh(out, byte2char, be + g.start(), be + g.end(), g.as_str());
    }
}

/// A bare number (`aff. 6/64`) keeps no role prefix, and the resolution tries
/// `C-` against it. The dash separator (`C-631-13`) comes back to a slash.
fn cjue_key(prefix: Option<&str>, num: &str) -> CaseAddress {
    let num: String = num
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| if c == '-' { '/' } else { c })
        .collect();
    let docket = match prefix {
        Some(p) => format!("{p}-{num}"),
        None => num,
    };
    CaseAddress::new("cjue", &docket, "")
}

fn case_cjue_aff(
    folded: &str,
    chars: &[char],
    byte2char: &[usize],
    be: usize,
    out: &mut Vec<CompiledCase>,
) {
    let win = case_window(folded, be, 260);
    let Some(c) = RE_CASE_AFF.captures(win) else {
        cjue_scan(folded, chars, byte2char, be, win, 0, out);
        return;
    };
    let n = c.get(2).expect("groupe slashnum");
    let (first, second) = n.as_str().split_once('/').expect("slashnum");
    let (s, e, key) = match c.get(1) {
        Some(p) if over_series_ceiling(p.as_str(), n.as_str()) => {
            cjue_scan(folded, chars, byte2char, be, win, n.end(), out);
            return;
        }
        Some(p) => (p.start(), n.end(), cjue_key(Some(p.as_str()), n.as_str())),
        None if first.trim().len() >= 4 => {
            push_cedh(out, byte2char, be + n.start(), be + n.end(), n.as_str());
            cjue_scan(folded, chars, byte2char, be, win, n.end(), out);
            return;
        }
        // A case number of the Court carries a two-digit year after the slash,
        // `6/64` as `561/19`; longer is a French docket written short, `affaire
        // n° 20/515`.
        None if second.trim().len() > 2 => {
            let num = rg_num_key(chars, byte2char, be + n.start(), be + n.end());
            push_case(
                out,
                byte2char,
                be + n.start(),
                be + n.end(),
                CaseAddress::new("", &num, ""),
            );
            return;
        }
        None => (n.start(), n.end(), cjue_key(None, n.as_str())),
    };
    push_case(out, byte2char, be + s, be + e, key);
    cjue_scan(folded, chars, byte2char, be, win, e, out);
}

fn case_cjue_window(
    folded: &str,
    chars: &[char],
    byte2char: &[usize],
    be: usize,
    out: &mut Vec<CompiledCase>,
) {
    let win = case_window(folded, be, 260);
    cjue_scan(folded, chars, byte2char, be, win, 0, out);
}

/// Balaye la fenêtre : numéros PRÉFIXÉS où qu'ils soient, plage à membre nu
/// héritant du préfixe (« C-338/11 à 347/11 »). Le préfixe séparé par espace
/// seul exige la MAJUSCULE.
fn cjue_scan(
    folded: &str,
    chars: &[char],
    byte2char: &[usize],
    off: usize,
    win: &str,
    start: usize,
    out: &mut Vec<CompiledCase>,
) {
    let mut pos = start;
    while let Some(c) = RE_CASE_CJUE_WIN.captures(&win[pos..]) {
        let (p, n) = (c.get(1).expect("préfixe"), c.get(2).expect("slashnum"));
        let sep = &win[pos + p.end()..pos + n.start()];
        let upper_ok = !sep.is_empty() && sep.trim().is_empty();
        if upper_ok && !chars[byte2char[off + pos + p.start()]].is_uppercase() {
            pos += n.end();
            continue;
        }
        if pourvoi_tail(folded, n.as_str(), off + pos + n.end()) {
            pos += n.end();
            continue;
        }
        if over_series_ceiling(p.as_str(), n.as_str()) {
            pos += n.end();
            continue;
        }
        if cut_by_window(folded, off, win, pos + n.end()) {
            return;
        }
        let key = cjue_key(Some(p.as_str()), n.as_str());
        push_case(
            out,
            byte2char,
            off + pos + p.start(),
            off + pos + n.end(),
            key,
        );
        let prefix = p.as_str().to_string();
        pos += n.end();
        while let Some(c) = RE_CASE_CJUE_RANGE.captures(&win[pos..]) {
            let n = c.get(1).expect("slashnum");
            if pourvoi_tail(folded, n.as_str(), off + pos + n.end())
                || over_series_ceiling(&prefix, n.as_str())
            {
                break;
            }
            let key = cjue_key(Some(&prefix), n.as_str());
            push_case(
                out,
                byte2char,
                off + pos + n.start(),
                off + pos + n.end(),
                key,
            );
            pos += n.end();
        }
    }
}

/// Only the General Court numbers past three digits, `T-1193/23`. A four-digit
/// `c` is a flattened ECLI or a French docket, never a case.
fn over_series_ceiling(prefix: &str, num: &str) -> bool {
    prefix != "t"
        && num
            .split(['/', '-'])
            .next()
            .is_some_and(|first| first.trim().len() > 3)
}

/// A `.NNN` behind a slashnum ends a Cassation pourvoi (`T 17-21.405`), not a
/// CJEU docket. Spaced, the guard only holds without a slash (`F 12-13. 669`):
/// after one, the dot closes the sentence and the digits number a paragraph
/// (`C-495/10. 31`).
fn pourvoi_tail(folded: &str, num: &str, abs_e: usize) -> bool {
    let rest = &folded.as_bytes()[abs_e..];
    if rest.first() != Some(&b'.') {
        return false;
    }
    match rest.get(1) {
        Some(b) if b.is_ascii_digit() => true,
        Some(b'\x20') if !num.contains('/') => rest.get(2).is_some_and(u8::is_ascii_digit),
        _ => false,
    }
}

/// Reconstruit le numéro depuis le texte ORIGINAL (les dockets TCOM/CAA
/// portent des MAJUSCULES — « 2015F00459 », « 20NT01234 » — que le pli
/// écrase), espaces retirés.
fn rg_num_key(chars: &[char], byte2char: &[usize], abs_s: usize, abs_e: usize) -> String {
    chars[byte2char[abs_s]..byte2char[abs_e]]
        .iter()
        .filter(|ch| !ch.is_whitespace())
        .collect()
}

/// Code référentiel de la juridiction au voisinage d'un RG : DERNIÈRE forme en
/// amont, sinon PREMIÈRE en aval. `want` restreint au type attendu (jugement
/// 7 chiffres = TA, arrêt AAXX99999 = CAA). Sans `want`, la forme la plus
/// proche DÉCIDE : si sa ville ne mappe pas, pas de code.
fn rg_jurisdiction(
    folded: &str,
    chrono: &crate::chrono::ChronoSnapshot,
    up: (usize, usize),
    down: (usize, usize),
    want: Option<&str>,
) -> Option<String> {
    let up_last = RE_CASE_RG_JUR
        .captures_iter(&folded[up.0..up.1])
        .last()
        .map(|c| (up.0, c));
    let down_first = RE_CASE_RG_JUR
        .captures(&folded[down.0..down.1])
        .map(|c| (down.0, c));
    for (off, c) in up_last.into_iter().chain(down_first) {
        let g = c.get(1).expect("groupe ville");
        let m0 = c.get(0).expect("match forme");
        let form = &folded[off + m0.start()..off + g.start()];
        let jt = if form.starts_with("cour d'appel") {
            "CA"
        } else if form.starts_with("cour administrative") {
            "CAA"
        } else if form.starts_with("tribunal administratif") {
            "TA"
        } else if form.contains("commerce") {
            "TCOM"
        } else if form.contains("prud'hommes") {
            "CPH"
        } else {
            "TJ"
        };
        if let Some(w) = want {
            if jt != w {
                continue;
            }
        }
        let region = crate::identity::normalize_component(&folded[off + g.start()..off + g.end()]);
        let words: Vec<&str> = region.split_whitespace().collect();
        for k in (1..=words.len().min(6)).rev() {
            let city = words[..k].join(" ");
            if let Some(code) = chrono.code(jt, &city) {
                return Some(code.to_string());
            }
        }
        want?;
    }
    None
}

/// Fenêtre amont à frontière de char, bornée à `len` octets.
fn upstream(folded: &str, bs: usize, len: usize) -> usize {
    let mut ws = bs.saturating_sub(len);
    while !folded.is_char_boundary(ws) {
        ws += 1;
    }
    ws
}

/// « RG n° 21/04532 », « Rôle N° 16/03071 », « enregistré au répertoire
/// général sous le n° 11/00094 » : n° en aval de l'ancre, juridiction au
/// voisinage mappée au référentiel via [`crate::chrono::ChronoSnapshot`].
/// Sans juridiction, clé nue `rg||NUM` — jamais résolue.
fn case_rg(
    folded: &str,
    chars: &[char],
    byte2char: &[usize],
    chrono: Option<&crate::chrono::ChronoSnapshot>,
    bs: usize,
    be: usize,
    out: &mut Vec<CompiledCase>,
) {
    let Some(chrono) = chrono else { return };
    let win = case_window(folded, be, 60);
    let Some(c) = RE_CASE_RG.captures(win) else {
        return;
    };
    rg_emit(folded, chars, byte2char, chrono, bs, be, win, &c, out);
}

/// Tronc commun RG : gardes propre-en-tête, juridiction au voisinage, clé
/// canonique, énumération. `c` = capture de la forme (groupe 1 = numéro) sur
/// `win` (fenêtre aval de `be`).
#[allow(clippy::too_many_arguments)]
fn rg_emit(
    folded: &str,
    chars: &[char],
    byte2char: &[usize],
    chrono: &crate::chrono::ChronoSnapshot,
    bs: usize,
    be: usize,
    win: &str,
    c: &regex::Captures,
    out: &mut Vec<CompiledCase>,
) {
    let g = c.get(1).expect("groupe RG");
    if cut_by_window(folded, be, win, g.end()) {
        return;
    }
    if folded[be + g.end()..].starts_with('.')
        && folded[be + g.end() + 1..]
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_digit())
    {
        return;
    }
    if case_window(folded, be + g.end(), 60).contains("portalis") {
        return;
    }
    let ws = upstream(folded, bs, 160);
    if RE_CASE_RG_OWN.is_match(&folded[ws..bs]) {
        return;
    }
    let down_end = case_window(folded, be + g.end(), 160).len() + be + g.end();
    let code = rg_jurisdiction(folded, chrono, (ws, bs), (be + g.end(), down_end), None)
        .unwrap_or_default();
    let num = rg_canon(rg_num_key(chars, byte2char, be + g.start(), be + g.end()));
    if code.is_empty() && !num.contains('/') {
        return;
    }
    push_case(
        out,
        byte2char,
        be + g.start(),
        be + g.end(),
        CaseAddress::new(&code, &num, ""),
    );
    let mut pos = g.end();
    while let Some(c) = RE_CASE_RG_NEXT.captures(&win[pos..]) {
        let g = c.get(1).expect("groupe RG");
        if cut_by_window(folded, be, win, pos + g.end()) {
            break;
        }
        let num = rg_canon(rg_num_key(
            chars,
            byte2char,
            be + pos + g.start(),
            be + pos + g.end(),
        ));
        push_case(
            out,
            byte2char,
            be + pos + g.start(),
            be + pos + g.end(),
            CaseAddress::new(&code, &num, ""),
        );
        pos += g.end();
    }
}

/// Un RG « NN-NNNNN » à tiret unique est la graphie tiret du rôle canonique à
/// barre (« 14-08937 » → « 14/08937 » en base) — canonisé dans la CLÉ, le
/// span reste verbatim.
fn rg_canon(num: String) -> String {
    let one_dash = num.bytes().filter(|b| *b == b'-').count() == 1;
    if one_dash && num.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
        num.replace('-', "/")
    } else {
        num
    }
}

/// « décision attaquée …, enregistrée sous le no 20/00325 », « procédures
/// enrôlées sous les numéros 18/02640 et 18/02641 » : la forme épelée des
/// bandeaux CA, sans ancre « RG ».
fn case_enrol(
    folded: &str,
    chars: &[char],
    byte2char: &[usize],
    chrono: Option<&crate::chrono::ChronoSnapshot>,
    bs: usize,
    be: usize,
    out: &mut Vec<CompiledCase>,
) {
    let Some(chrono) = chrono else { return };
    let win = case_window(folded, be, 80);
    let Some(c) = RE_CASE_ENROL.captures(win) else {
        return;
    };
    rg_emit(folded, chars, byte2char, chrono, bs, be, win, &c, out);
}

/// « CA [Localité 8], 14 nov. 2019, n°18/04366 » : sigle CA (MAJUSCULES
/// gâtées au dispatch) + marqueur n° strict.
fn case_ca_sigle(
    folded: &str,
    chars: &[char],
    byte2char: &[usize],
    chrono: Option<&crate::chrono::ChronoSnapshot>,
    bs: usize,
    be: usize,
    out: &mut Vec<CompiledCase>,
) {
    let Some(chrono) = chrono else { return };
    let win = case_window(folded, be, 70);
    let Some(c) = RE_CASE_CA.captures(win) else {
        return;
    };
    rg_emit(folded, chars, byte2char, chrono, bs, be, win, &c, out);
}

/// « DÉCISION DÉFÉRÉE : 21/00287 » — bandeau CA sans « RG ».
fn case_deferee(
    folded: &str,
    chars: &[char],
    byte2char: &[usize],
    chrono: Option<&crate::chrono::ChronoSnapshot>,
    bs: usize,
    be: usize,
    out: &mut Vec<CompiledCase>,
) {
    let Some(chrono) = chrono else { return };
    let win = case_window(folded, be, 40);
    let Some(c) = RE_CASE_DEFEREE_NUM.captures(win) else {
        return;
    };
    rg_emit(folded, chars, byte2char, chrono, bs, be, win, &c, out);
}

/// « ARRÊT AU FOND DU 25 MARS 2011 N° 2011/183 » : l'ordinal année/n° d'un
/// arrêt CA — clé nue `rg||`, jamais résolue.
fn case_arret_ord(
    folded: &str,
    chars: &[char],
    byte2char: &[usize],
    be: usize,
    out: &mut Vec<CompiledCase>,
) {
    if be < 1200 {
        return;
    }
    let win = case_window(folded, be, 60);
    let Some(c) = RE_CASE_ARRET_ORD.captures(win) else {
        return;
    };
    let g = c.get(1).expect("groupe ordinal");
    if cut_by_window(folded, be, win, g.end()) {
        return;
    }
    let num = rg_num_key(chars, byte2char, be + g.start(), be + g.end());
    push_case(
        out,
        byte2char,
        be + g.start(),
        be + g.end(),
        CaseAddress::new("", &num, ""),
    );
}

/// « dans son arrêt n° 350095 du 28 mai 2014 » : requête CE derrière
/// « arrêt », validée par un « Conseil d'État » au voisinage (amont 120 ou
/// aval 40).
fn case_ce_arret(
    folded: &str,
    byte2char: &[usize],
    bs: usize,
    be: usize,
    out: &mut Vec<CompiledCase>,
) {
    let win = case_window(folded, be, 60);
    let Some(c) = RE_CASE_ARRET_CE.captures(win) else {
        return;
    };
    let g = c.get(1).expect("groupe n° CE");
    if cut_by_window(folded, be, win, g.end()) {
        return;
    }
    let ws = upstream(folded, bs, 120);
    let near_ce = folded[ws..bs].contains("conseil d'etat")
        || case_window(folded, be + g.end(), 40).contains("conseil d'etat");
    if near_ce {
        push_case(
            out,
            byte2char,
            be + g.start(),
            be + g.end(),
            CaseAddress::new("ce", g.as_str(), ""),
        );
    }
}

/// Chaîne procédurale du fond administratif : « Par un jugement n° 1901563 du
/// 15 février 2023, le tribunal administratif de Nantes… » (clé
/// `af|ta_nantes|1901563`), « Par un arrêt n° 20NT01234 …, la cour
/// administrative d'appel de Nantes » (`af|caa_nantes|20NT01234`), « Jugement
/// (N° 14/00048) rendu … par le tribunal de grande instance de X ».
fn case_admin(
    folded: &str,
    chars: &[char],
    byte2char: &[usize],
    chrono: Option<&crate::chrono::ChronoSnapshot>,
    bs: usize,
    be: usize,
    out: &mut Vec<CompiledCase>,
) {
    let Some(chrono) = chrono else { return };
    let arret = folded[bs..be].starts_with("arret");
    let win = case_window(folded, be, 60);
    let (re, re_next, want): (&Regex, &Regex, &str) = if arret {
        (&RE_CASE_ADMIN_CAA, &RE_CASE_ADMIN_CAA_NEXT, "CAA")
    } else {
        (&RE_CASE_ADMIN_TA, &RE_CASE_ADMIN_TA_NEXT, "TA")
    };
    let ws = upstream(folded, bs, 160);
    if let Some(c) = re.captures(win) {
        let g = c.get(1).expect("groupe n° admin");
        if cut_by_window(folded, be, win, g.end()) {
            return;
        }
        let down_end = case_window(folded, be + g.end(), 200).len() + be + g.end();
        let code = rg_jurisdiction(
            folded,
            chrono,
            (ws, bs),
            (be + g.end(), down_end),
            Some(want),
        )
        .unwrap_or_default();
        let num = rg_num_key(chars, byte2char, be + g.start(), be + g.end());
        if code.is_empty() && !num.contains('/') {
            return;
        }
        push_case(
            out,
            byte2char,
            be + g.start(),
            be + g.end(),
            CaseAddress::new(&code, &num, ""),
        );
        let mut pos = g.end();
        while let Some(c) = re_next.captures(&win[pos..]) {
            let g = c.get(1).expect("groupe n° admin");
            if cut_by_window(folded, be, win, pos + g.end()) {
                break;
            }
            let num = rg_num_key(chars, byte2char, be + pos + g.start(), be + pos + g.end());
            push_case(
                out,
                byte2char,
                be + pos + g.start(),
                be + pos + g.end(),
                CaseAddress::new(&code, &num, ""),
            );
            pos += g.end();
        }
        return;
    }
    if let Some(c) = RE_CASE_ADMIN_RG.captures(win) {
        let g = c.get(1).expect("groupe RG");
        if g.as_str().split(['/', '-']).next().is_some_and(|first| {
            first.len() == 4 && (first.starts_with("19") || first.starts_with("20"))
        }) {
            return;
        }
        rg_emit(folded, chars, byte2char, chrono, bs, be, win, &c, out);
    }
}

/// Anaphore depuis un connecteur : nature après le connecteur (« du même
/// code »), ou nature AVANT un « précité » (« du code précité »). Retourne
/// (début, nature, fin couverte) — la fin couverte inclut le mot de nature qui
/// suit le connecteur.
fn lex_same(folded: &str, bs: usize, be: usize) -> Option<(usize, String, usize)> {
    match &folded[bs..be] {
        "son article" | "ses articles" => Some((bs, String::new(), be)),
        "precite" | "precitee" => {
            let mut ws = bs.saturating_sub(48);
            while !folded.is_char_boundary(ws) {
                ws += 1;
            }
            let c = RE_SAME_PRECITE.captures(&folded[ws..bs])?;
            Some((ws + c.get(0)?.start(), c[1].to_string(), be))
        }
        _ => {
            let c = RE_SAME_NAT.captures(&folded[be..])?;
            Some((bs, c[1].to_string(), be + c.get(1).unwrap().end()))
        }
    }
}

/// Scan : UNE passe de l'automate FUSIONNÉ + lexers positionnés. Produit le
/// flux citations ([`Tok`]) et, si demandé, le flux marqueurs (`marks`).
///
/// Discipline de consommation UNIQUE : un curseur (`tok_pos`),
/// leftmost-longest — un motif = un token porteur de son (ses) rôle(s), chaque
/// composeur filtre les siens. Les gates de légitimité
/// ([`crate::scan::marker_token`], frontière droite + repli d'ancre) sont des
/// filtres AVAL : ils rejettent le token, jamais la consommation.
fn scan_all(
    vocab: Option<&CompiledVocab>,
    chrono: Option<&crate::chrono::ChronoSnapshot>,
    folded: &str,
    byte2char: &[usize],
    chars: &[char],
    mut marks: Option<&mut Vec<crate::scan::PTok>>,
    cases: &mut Vec<CompiledCase>,
) -> Vec<Tok> {
    let f = fused();
    let bytes = folded.as_bytes();

    // Phase 1 : la passe automate — chaque match distribué vers ses rôles.
    let mut cit_matches: Vec<(usize, usize, Option<Pat>)> = Vec::new();
    let mut tok_pos = 0usize;
    let mut pos = 0usize;
    while let Some(m) = f.ac.find(&folded[pos..]) {
        let (bs, be) = (pos + m.start(), pos + m.end());
        if bs < tok_pos {
            pos = tok_pos.max(bs + folded[bs..].chars().next().map_or(1, |c| c.len_utf8()));
            continue;
        }
        if bs != 0 && bytes[bs].is_ascii_alphanumeric() && bytes[bs - 1].is_ascii_alphanumeric() {
            pos = bs + folded[bs..].chars().next().map_or(1, |c| c.len_utf8());
            continue;
        }
        pos = be;
        tok_pos = be;
        let roles = f.roles[m.pattern().as_usize()];
        if let (Some(kind), Some(marks)) = (roles.marker, marks.as_deref_mut()) {
            let mk_be = if roles.mk_len > 0 {
                bs + roles.mk_len
            } else {
                be
            };
            if let Some(t) = crate::scan::marker_token(chars, folded, byte2char, bs, mk_be, kind) {
                marks.push(t);
            }
        }
        if vocab.is_some() {
            cit_matches.push((bs, be, roles.cite));
        }
    }
    let Some(vocab) = vocab else {
        return Vec::new();
    };

    // Phase 2 : dispatch citations (lexers, snap, anaphores).
    let mut toks: Vec<Tok> = Vec::new();
    let mut instr_spans: BTreeMap<usize, usize> = BTreeMap::new(); // byte s → byte e
    let mut artwords: Vec<(usize, usize)> = Vec::new(); // (start, end) bytes
    let mut memo: FxHashMap<String, String> = FxHashMap::default();
    let mut last_instr_end = 0usize;

    let push_instr = |toks: &mut Vec<Tok>,
                      instr_spans: &mut BTreeMap<usize, usize>,
                      memo: &mut FxHashMap<String, String>,
                      bs: usize,
                      be: usize,
                      text_key: String,
                      is_code: bool,
                      weak: bool,
                      treaty_short: bool,
                      nested: bool| {
        let folded_span = canon_fold(&folded[bs..be]);
        if BLOCKED_SURFACES.contains(&folded_span.as_str()) {
            return false;
        }
        let text_key = if text_key.is_empty() {
            let surface = span_surface(chars, byte2char, bs, be);
            norm_key(memo, &folded_span, &surface)
        } else {
            text_key
        };
        let mut hs = bs.saturating_sub(48);
        while !folded.is_char_boundary(hs) {
            hs += 1;
        }
        if let Some(c) = RE_BARE_ART_GEN.captures(&folded[hs..bs]) {
            let g = c.get(1).expect("groupe numéral");
            let (abs, abe) = (hs + g.start(), hs + g.end());
            let surface: String = chars[byte2char[abs]..byte2char[abe]].iter().collect();
            let num_key = normalize_article(&surface);
            if !num_key.is_empty() {
                toks.push(Tok::Art {
                    s: byte2char[abs],
                    e: byte2char[abe],
                    surface,
                    num_key,
                });
            }
        }
        instr_spans.insert(bs, be);
        toks.push(Tok::Instr {
            s: byte2char[bs],
            e: byte2char[be],
            text_key,
            is_code,
            weak,
            treaty_short,
            nested,
        });
        true
    };

    let anchor_at = |bs: usize| -> Option<(usize, Pat)> {
        ANCHORS
            .iter()
            .enumerate()
            .filter(|(_, (s, _))| {
                folded[bs..].starts_with(s)
                    && (bs + s.len() >= bytes.len() || !bytes[bs + s.len()].is_ascii_alphanumeric())
            })
            .max_by_key(|(_, (s, _))| s.len())
            .map(|(_, (s, class))| (bs + s.len(), Pat::Anchor(*class)))
    };

    for &(bs, be, pat) in &cit_matches {
        let (bs, mut be, mut pat) = (bs, be, pat);
        if bs != 0 && bytes[bs - 1].is_ascii_alphanumeric() {
            continue;
        }
        if pat.is_none() {
            for w in ["articles", "article"] {
                if folded[bs..be].ends_with(w)
                    && (be - w.len() == bs || !bytes[be - w.len() - 1].is_ascii_alphanumeric())
                {
                    artwords.push((be - w.len(), be));
                    break;
                }
            }
            if be >= bytes.len() || !bytes[be].is_ascii_alphanumeric() {
                if let Some((s, _)) = ANCHORS
                    .iter()
                    .filter(|(s, class)| {
                        matches!(class, Anchor::Case)
                            && folded[bs..be].ends_with(s)
                            && (be - s.len() == bs
                                || !bytes[be - s.len() - 1].is_ascii_alphanumeric())
                    })
                    .max_by_key(|(s, _)| s.len())
                {
                    lex_case(folded, chars, byte2char, chrono, be - s.len(), be, cases);
                }
            }
            let Some((abe, apat)) = anchor_at(bs) else {
                continue;
            };
            (be, pat) = (abe, Some(apat));
        }
        if be < bytes.len() && bytes[be].is_ascii_alphanumeric() {
            let Some((abe, apat)) = anchor_at(bs) else {
                continue;
            };
            (be, pat) = (abe, Some(apat));
        }
        match pat.expect("rôle citations dérivé ci-dessus") {
            Pat::Alias { is_code, sigle } => {
                if &folded[bs..be] == "cedh" && case_cedh(folded, byte2char, be, cases, false) {
                    continue;
                }
                if bs < last_instr_end {
                    let head = folded[..bs].trim_end();
                    if !is_code && !sigle && (head.ends_with(" a la") || head.ends_with(" a l'")) {
                        toks.push(Tok::Instr {
                            s: byte2char[bs],
                            e: byte2char[be],
                            text_key: folded[bs..be].to_string(),
                            is_code: false,
                            weak: false,
                            treaty_short: false,
                            nested: true,
                        });
                    }
                    continue;
                }
                if sigle
                    && be - bs < 4
                    && chars[byte2char[bs]..byte2char[be]]
                        .iter()
                        .any(|c| c.is_alphabetic() && c.is_lowercase())
                {
                    continue;
                }
                let mut end = be;
                let mut tk = folded[bs..be].to_string();
                let mut icode = is_code;
                if is_code {
                    if let Some((abe, _)) = anchor_at(bs) {
                        if let Some((m2, true)) = lex_code(vocab, folded, bs, abe) {
                            if m2.e > end {
                                end = m2.e;
                                tk = m2.text_key;
                                icode = m2.is_code;
                            }
                        }
                    }
                }
                if !is_code {
                    if let Some(nat_len) = ["reglement", "directive", "decision"]
                        .iter()
                        .find_map(|n| folded[bs..be].starts_with(n).then_some(n.len()))
                    {
                        if let Some(m) = RE_LEX_EU.find(&folded[bs + nat_len..]) {
                            end = end.max(bs + nat_len + m.end());
                        }
                    }
                }
                if let Some(e2) = gentile_ext(folded, end).or_else(|| territory_ext(folded, end)) {
                    end = e2;
                    tk = String::new();
                    icode = false;
                }
                if push_instr(
                    &mut toks,
                    &mut instr_spans,
                    &mut memo,
                    bs,
                    end,
                    tk,
                    icode,
                    false,
                    false,
                    false,
                ) {
                    last_instr_end = end;
                }
            }
            Pat::Anchor(Anchor::ArtWord) => artwords.push((bs, be)),
            Pat::Anchor(Anchor::Case) => {
                lex_case(folded, chars, byte2char, chrono, bs, be, cases);
            }
            Pat::Anchor(Anchor::SameConn) => {
                if let Some((s, nature, covered)) = lex_same(folded, bs, be) {
                    toks.push(Tok::Same {
                        s: byte2char[s],
                        nature,
                    });
                    last_instr_end = last_instr_end.max(covered);
                    if folded[bs..be].ends_with("article") || folded[bs..be].ends_with("articles") {
                        artwords.push((bs, be));
                    }
                }
            }
            Pat::Anchor(class) => {
                if bs < last_instr_end {
                    if matches!(class, Anchor::Treaty) {
                        let head = folded[..bs].trim_end();
                        if head.ends_with(" a la") || head.ends_with(" a l'") {
                            if let Some(m) = lex_treaty(vocab, folded, bs, be) {
                                if !m.text_key.is_empty() {
                                    toks.push(Tok::Instr {
                                        s: byte2char[bs],
                                        e: byte2char[m.e],
                                        text_key: m.text_key,
                                        is_code: false,
                                        weak: false,
                                        treaty_short: false,
                                        nested: true,
                                    });
                                }
                            }
                        }
                    }
                    continue;
                }
                let lexed = match class {
                    Anchor::Code => {
                        let lexed = lex_code(vocab, folded, bs, be).map(|(m, _)| m);
                        if lexed.is_none()
                            && matches!(&folded[bs..be], "code" | "livre")
                            && folded[..bs].trim_end().ends_with(" du")
                        {
                            let rest = folded[be..].trim_start();
                            let named = ["de ", "du ", "des ", "d'"]
                                .iter()
                                .any(|c| rest.starts_with(c));
                            if !named {
                                let head = folded[..bs].trim_end();
                                toks.push(Tok::Same {
                                    s: byte2char[head.len() - 2],
                                    nature: folded[bs..be].to_string(),
                                });
                            }
                        }
                        lexed
                    }
                    Anchor::Dated => {
                        let mut lexed = lex_dated(vocab, folded, bs, be);
                        if &folded[bs..be] == "ordonnance" {
                            if lexed.is_none() {
                                case_cjue_window(folded, chars, byte2char, be, cases);
                            }
                            if lexed.as_ref().is_none_or(|m| m.text_key.is_empty()) {
                                let before = cases.len();
                                case_admin(folded, chars, byte2char, chrono, bs, be, cases);
                                let win = case_window(folded, be, 40);
                                if let Some(c) = RE_CASE_ORD_CE.captures(win) {
                                    let g = c.get(1).expect("groupe n° CE");
                                    if !cut_by_window(folded, be, win, g.end()) {
                                        let key = CaseAddress::new("ce", g.as_str(), "");
                                        push_case(
                                            cases,
                                            byte2char,
                                            be + g.start(),
                                            be + g.end(),
                                            key,
                                        );
                                    }
                                }
                                if cases.len() > before {
                                    lexed = None;
                                }
                            }
                        }
                        lexed
                    }
                    Anchor::Eu => match lex_eu(vocab, folded, bs, be) {
                        EuLex::Instr(m) => Some(m),
                        EuLex::Same(s) => {
                            toks.push(Tok::Same {
                                s: byte2char[s],
                                nature: folded[bs..be].to_string(),
                            });
                            None
                        }
                        EuLex::None => {
                            if &folded[bs..be] == "decision" {
                                case_constit(folded, byte2char, bs, be, cases, true);
                            }
                            None
                        }
                    },
                    Anchor::Treaty => {
                        let lexed = lex_treaty(vocab, folded, bs, be);
                        if lexed.is_none() {
                            let word = &folded[bs..be];
                            let head = folded[..bs].trim_end();
                            let conn = match word {
                                "convention" | "charte" | "declaration" => {
                                    head.ends_with(" de la").then_some("de la")
                                }
                                "pacte" | "protocole" | "traite" | "accord" => {
                                    head.ends_with(" du").then_some("du")
                                }
                                _ => None,
                            };
                            if let Some(conn) = conn {
                                if folded[be..]
                                    .trim_start()
                                    .starts_with([',', '.', ';', ':', ')'])
                                {
                                    toks.push(Tok::Same {
                                        s: byte2char[head.len() - conn.len()],
                                        nature: word.to_string(),
                                    });
                                }
                            }
                        }
                        lexed
                    }
                    Anchor::Constitution => lex_constitution(folded, chars, byte2char, bs, be),
                    Anchor::Boi => lex_boi(folded, chars, byte2char, bs, be),
                    Anchor::ArtWord | Anchor::SameConn | Anchor::Case => unreachable!(),
                };
                if let Some(mention) = lexed {
                    if push_instr(
                        &mut toks,
                        &mut instr_spans,
                        &mut memo,
                        bs,
                        mention.e,
                        mention.text_key,
                        mention.is_code,
                        mention.weak,
                        mention.treaty_short,
                        false,
                    ) {
                        last_instr_end = mention.e;
                    }
                }
            }
        }
    }

    for (&s, &e) in &instr_spans {
        for w in ["articles", "article"] {
            if folded[s..e].ends_with(w)
                && !folded.as_bytes()[e - w.len() - 1].is_ascii_alphanumeric()
            {
                artwords.push((e - w.len(), e));
                break;
            }
        }
    }

    // Énumérations d'articles : à partir de chaque « article(s) », consommer les
    // numéraux — l'énumération TRAVERSE les instruments.
    for &(aws, awe) in &artwords {
        let mut pos = awe;
        while let Some(sep) = RE_SEP.find(&folded[pos..]) {
            let mut numpos = pos + sep.end();
            if let Some(m) = RE_ARABIC_PARA.find(&folded[numpos..]) {
                pos = numpos + m.end();
                continue;
            }
            if RE_NUM.captures(&folded[numpos..]).is_none() {
                if let Some(m) = RE_NAMED_ART.find(&folded[numpos..]) {
                    let (nbs, nbe) = (numpos + m.start(), numpos + m.end());
                    let surface: String = chars[byte2char[nbs]..byte2char[nbe]].iter().collect();
                    let num_key = normalize_article(&surface);
                    if !num_key.is_empty() {
                        toks.push(Tok::Art {
                            s: byte2char[nbs],
                            e: byte2char[nbe],
                            surface,
                            num_key,
                        });
                    }
                    pos = nbe;
                    continue;
                }
                if let Some(m) = RE_ROMAN_PARA.find(&folded[numpos..]) {
                    numpos += m.end();
                }
                let probe = &folded[numpos..];
                let jump = ["du ", "de la ", "de l'", "des ", "de l' "]
                    .iter()
                    .find_map(|p| probe.starts_with(p).then_some(p.len()))
                    .map(|off| (numpos + off, numpos + off + 3))
                    .or_else(|| {
                        RE_REDACTION
                            .find(probe)
                            .map(|m| (numpos + m.end(), numpos + m.end() + 3))
                    })
                    .or(Some((numpos, numpos + 2)));
                if let Some((ipos, ilim)) = jump {
                    if let Some((_, &iend)) = (ipos < bytes.len())
                        .then(|| instr_spans.range(ipos..ilim.min(bytes.len())).next())
                        .flatten()
                    {
                        if let Some(sep2) = RE_SEP.find(&folded[iend..]) {
                            let cand = iend + sep2.end();
                            if sep2.end() > 1
                                && (RE_NUM.captures(&folded[cand..]).is_some()
                                    || RE_NAMED_ART.find(&folded[cand..]).is_some())
                            {
                                numpos = cand;
                            }
                        }
                        if RE_NUM.captures(&folded[numpos..]).is_none() {
                            if let Some(t) = RE_TITLE_TAIL.find(&folded[iend..]) {
                                let tail_start = iend + t.end();
                                let mut lim = (tail_start + 90).min(folded.len());
                                while !folded.is_char_boundary(lim) {
                                    lim -= 1;
                                }
                                let lim = folded[tail_start..lim]
                                    .find(['.', ';', ':', '('])
                                    .map_or(lim, |i| tail_start + i);
                                for m in RE_ET.find_iter(&folded[tail_start..lim]) {
                                    let cand = tail_start + m.end();
                                    if RE_NUM.captures(&folded[cand..]).is_some() {
                                        numpos = cand;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let Some(c) = RE_NUM.captures(&folded[numpos..]) else {
                break;
            };
            let full = c.get(0).unwrap();
            let (bs, mut be) = (numpos + full.start(), numpos + full.end());
            if be + 1 < bytes.len()
                && bytes[be] == b' '
                && bytes[be + 1].is_ascii_lowercase()
                && bytes[be + 1] != b'i'
                && chars[byte2char[be + 1]].is_uppercase()
                && (be + 2 == bytes.len()
                    || (!bytes[be + 2].is_ascii_alphanumeric()
                        && bytes[be + 2] != b'.'
                        && bytes[be + 2] != b'\''))
            {
                be += 2;
                if let Some(m) = RE_NUM_ORDINAL.find(&folded[be..]) {
                    be += m.end();
                }
            }
            if be < bytes.len()
                && bytes[be].is_ascii_alphanumeric()
                && !(folded[be..].starts_with("et ") || folded[be..].starts_with("ou "))
            {
                break;
            }
            let after = folded[be..].trim_start();
            let paren_self_ref = aws > 0 && bytes[aws - 1] == b'(' && after.starts_with(')');
            // « DECIDE : Article 1er. - La requête est rejetée », the older dispositif,
            // its article opening a clause where a cited one follows « l' » or « de ».
            let before = folded[..aws].trim_end();
            let dash_heading = folded[aws..awe].starts_with("article")
                && bytes[bs].is_ascii_digit()
                && (before.is_empty()
                    || before.ends_with([':', '.', ';'])
                    || before.ends_with("decide"))
                && ["- ", ". - ", ".- "].iter().any(|d| {
                    after.strip_prefix(d).is_some_and(|ruling| {
                        ["la ", "le ", "les ", "l'", "il ", "expedition "]
                            .iter()
                            .any(|w| ruling.starts_with(w))
                    })
                });
            if after.starts_with(':')
                || after.starts_with('\u{2013}')
                || dash_heading
                || paren_self_ref
            {
                pos = be;
                continue;
            }
            let surface: String = chars[byte2char[bs]..byte2char[be]].iter().collect();
            let num_key = normalize_article(&surface);
            if !num_key.is_empty() {
                toks.push(Tok::Art {
                    s: byte2char[bs],
                    e: byte2char[be],
                    surface,
                    num_key,
                });
            }
            pos = be;
        }
    }

    toks.sort_by_key(tok_start);
    toks.dedup_by(|a, b| match (a, b) {
        (Tok::Art { s: s1, e: e1, .. }, Tok::Art { s: s2, e: e2, .. }) => s1 == s2 && e1 == e2,
        _ => false,
    });

    cases.sort_by_key(|c| c.char_start);
    let mut prev_end = 0usize;
    cases.retain(|c| {
        if c.char_start < prev_end {
            return false;
        }
        prev_end = c.char_end;
        true
    });
    toks
}

// ── composition ─────────────────────────────────────────────────────────────

/// Une citation composée, prête pour la projection prod
/// (`CitationOccurrenceRow`) ou le scoring banc.
pub struct CompiledCitation {
    pub char_start: usize,
    pub char_end: usize,
    /// Forme de surface de l'instrument (capture ou antécédent résolu).
    pub instrument: String,
    pub article: Option<String>,
    pub text_key: String,
    pub article_key: Option<String>,
    pub target: LinkTarget,
    /// La mention porte la locution « et suivants » : le span la couvre, la cible
    /// désigne une famille d'articles à partir de l'ancre.
    pub suivants: bool,
    /// Article NU (cible None par construction) : aucun instrument rattaché ni
    /// tenté — ni génitif, ni anaphore, ni antécédent, ni unicité, ni canon.
    pub bare: bool,
}

/// A document prepared for the automaton scans: the source text as chars, the
/// folded text (`lj_fold`, one char giving several or none) and the
/// byte(folded) to char(source) tables. Built once per document and shared by
/// the automata (citation vocabulary here, markers in `crate::scan`).
///
/// `chars` maps every whitespace (line break, tab, nbsp) to a plain space, one
/// to one with the input: composers test the space uniformly, verbatim slices
/// come out on one line, and offsets stay those of the input. Every folded
/// offset goes back through `byte2char`.
pub struct Norm {
    pub chars: Vec<char>,
    pub folded: String,
    pub byte2char: Vec<usize>,
    pub char2byte: Vec<usize>,
    /// Where the input text broke the line. `chars` maps that break to a plain
    /// space, and a source separating its paragraphs with a single break would
    /// leave no trace of it, the break being the only paragraph boundary an
    /// administrative judgment has.
    pub breaks: Vec<bool>,
}

impl Norm {
    /// One pass over the text: mapped chars, fold and both tables together.
    /// Every folded byte points to the source char that produced it; a char the
    /// fold drops has no byte, and `char2byte` merges it with the next one.
    pub fn new(text: &str) -> Self {
        let n = text.len();
        let mut chars: Vec<char> = Vec::with_capacity(n);
        let mut folded = String::with_capacity(n);
        let mut byte2char: Vec<usize> = Vec::with_capacity(n + 1);
        let mut char2byte: Vec<usize> = Vec::with_capacity(n + 1);
        let mut breaks: Vec<bool> = Vec::with_capacity(n);
        for (ci, c) in text.chars().enumerate() {
            chars.push(if c.is_whitespace() { ' ' } else { c });
            breaks.push(c == '\n' || c == '\r');
            char2byte.push(folded.len());
            lj_fold::push_folded(c, &mut folded);
            byte2char.resize(folded.len(), ci);
        }
        byte2char.push(chars.len());
        char2byte.push(folded.len());
        Self {
            chars,
            folded,
            byte2char,
            char2byte,
            breaks,
        }
    }
}

/// Extraction compilée d'un document : scan (une passe) + composition sur le
/// flux de tokens. Sortie triée par position.
pub fn extract_citations(
    text: &str,
    vocab: &CompiledVocab,
    snap: &LinkSnapshot,
    forum: Option<Forum>,
) -> Vec<CompiledCitation> {
    extract_citations_norm(&Norm::new(text), vocab, snap, forum)
}

/// Variante à préparation fournie : l'appelant construit un [`Norm`] et le
/// partage entre les moteurs.
pub fn extract_citations_norm(
    norm: &Norm,
    vocab: &CompiledVocab,
    snap: &LinkSnapshot,
    forum: Option<Forum>,
) -> Vec<CompiledCitation> {
    let toks = scan_all(
        Some(vocab),
        None,
        &norm.folded,
        &norm.byte2char,
        &norm.chars,
        None,
        &mut Vec::new(),
    );
    compose(
        &toks,
        &norm.chars,
        &norm.folded,
        &norm.char2byte,
        &norm.breaks,
        vocab,
        snap,
        forum,
        None,
    )
}

/// Flux marqueurs seul (composeurs parties/conseil/outcome) — même automate
/// fusionné, motifs citations inertes.
pub(crate) fn scan_marks(norm: &Norm) -> Vec<crate::scan::PTok> {
    let mut marks = Vec::new();
    scan_all(
        None,
        None,
        &norm.folded,
        &norm.byte2char,
        &norm.chars,
        Some(&mut marks),
        &mut Vec::new(),
    );
    marks
}

/// Extraction par-document unifiée : UN [`Norm`], UNE passe de l'automate
/// fusionné → les deux flux de tokens, puis la composition citations.
pub struct DocExtract {
    pub scan: crate::scan::DocScan,
    pub citations: Vec<CompiledCitation>,
    /// Citations de jurisprudence — flux séparé, jamais composé.
    pub cases: Vec<CompiledCase>,
}

pub fn doc_extract(
    text: &str,
    vocab: &CompiledVocab,
    snap: &LinkSnapshot,
    chrono: &crate::chrono::ChronoSnapshot,
    forum: Option<Forum>,
    owner_text_uid: Option<&str>,
) -> DocExtract {
    let norm = Norm::new(text);
    let mut marks = Vec::new();
    let mut cases = Vec::new();
    let toks = scan_all(
        Some(vocab),
        Some(chrono),
        &norm.folded,
        &norm.byte2char,
        &norm.chars,
        Some(&mut marks),
        &mut cases,
    );
    let citations = compose(
        &toks,
        &norm.chars,
        &norm.folded,
        &norm.char2byte,
        &norm.breaks,
        vocab,
        snap,
        forum,
        owner_text_uid,
    );
    if let Some(Forum::Foreign(place, _)) = forum {
        let own = crate::foreign::foreign_cases(&norm.chars, place.as_str());
        cases.retain(|c| {
            !own.iter()
                .any(|o| c.char_start < o.char_end && o.char_start < c.char_end)
        });
        cases.extend(own);
        cases.sort_by_key(|c| c.char_start);
    }
    DocExtract {
        scan: crate::scan::docscan_from_parts(norm, marks),
        citations,
        cases,
    }
}

/// Le texte entre un numéral d'article et l'instrument suivant est-il un
/// connecteur génitif (« du », « de la », « , du même »…) ? C'est LA condition
/// de légitimité du rattachement-suivant.
fn genitive_gap(gap: &str) -> bool {
    // QUAL = qualificatif de subdivision entre le numéral et l'instrument :
    // « alinéa 2 », « paragraphe 1 », « point 2 », « (1) », « (ANCIEN) », chiffre
    // nu (« 39, 2, de la Convention »). Deux formes de gap : avec connecteur
    // (« (1) du code ») ou qualificatif seul devant un sigle (« 107,
    // paragraphe 1, TFUE »).
    static RE_GAP: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?x)^(?:
              [\s,]*(?:precite[es]?\s+)?(?:et\s+suivants?\s+)?
              # [a-z]{1,2}[0-9]{0,2} : lettre d'énumération (« 5-1 a) ») ou
              # suffixe fiscal du numéral (« L. 80 CA du livre des procédures
              # fiscales », « L. 76 B ») — l'article appartient à l'instrument
              # génitif qui suit.
              # -?[ivx] : paragraphe romain collé au numéral par tiret
              # (« l'article 75-I de la loi du 10 juillet 1991 »).
              (?:(?:et\s+)?(?:(?:premier|deuxieme|troisieme|second|dernier)\s+alineas?|(?:alineas?|alienas?|al\.?|§|paragraphes?|paragr\.?|points?)\s*(?:\d+(?:er)?|premiers?|seconds?)?|\(\s*[^)]{1,20}\)|(?:nouveaux?|nouvelles?|anciens?|anciennes?)?\s*\)|[a-z]{1,2}[0-9]{0,2}[)']?|-?[ivx]{1,3}(?:\s+\d{1,2}\s*°?)?|\d{1,2}\s*°?|°|er)\s*,?\s+)*
              (?:et\s+)?
              (?:anciens?\s+|anciennes?\s+|nouveaux?\s+|nouvelles?\s+)?
              # « of the » : annexes anglaises CEDH (« Article 3 of the
              # Convention for the Protection of Human Rights »).
              (?:du|de\s+la|de\s+l'\s*|des|de|dudit|de\s+ladite|du\s+meme|de\s+ce|of\s+the)\s*
              (?:le\s+|la\s+)?
              (?:'?(?:nouveau|nouvelle|ancien|ancienne)'?\s*(?:du\s+|de\s+la\s+|de\s+l'\s*)?)?
            |
              [\s,]*(?:(?:(?:premier|deuxieme|troisieme|second|dernier)\s+alineas?|(?:alineas?|alienas?|al\.?|§|paragraphes?|paragr\.?|points?)\s*(?:\d+(?:er)?|premiers?|seconds?)?|\(\s*[^)]{1,20}\)|[a-z][)']?|[ivx]{1,3}(?:\s+\d{1,2}\s*°?)?|\d{1,2}\s*°?)\s*,?\s+)+
            )$",
        )
        .unwrap()
    });
    if gap.chars().count() <= 3 && gap.chars().all(|c| c == ' ' || c == ',') {
        return true;
    }
    gap.chars().count() <= 30 && RE_GAP.is_match(gap)
}

/// N° d'acte à millésime-tiret 4 chiffres (« n° 2020-1733 ») : la marque des
/// actes RÉGLEMENTAIRES.
static RE_ACT_NORM_NUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"n\s*[°oº][\s.]*\d{4}\s*-\s*\d").unwrap());
/// NOR embarqué dans le span : la marque d'un acte ministériel PUBLIÉ.
static RE_ACT_NOR_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bnor\s*:?\s*[a-z]{4}\d{7}[a-z]\b").unwrap());
/// Queue de titre normative d'un acte daté (« arrêté du 27 décembre 2016
/// relatif aux conditions… », « portant application de… ») — sondée derrière
/// la mention ET dans la mention.
static RE_ACT_NORM_TAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*,?\s*(?:portant|relati(?:f|ve)s?|fixant|modifiant|completant|instituant|autorisant|approuvant|pris(?:e)?\s+(?:pour|en)|susvise|precite)\b",
    )
    .unwrap()
});
static RE_ACT_NORM_TAIL_IN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:portant|relati(?:f|ve)s?\s+a|fixant|modifiant|instituant)\b").unwrap()
});
/// Contexte AMONT qui introduit une norme (« en vertu de l'ordonnance… »,
/// « prévus par l'arrêté… », « figurant à l'annexe I de l'arrêté… »).
static RE_ACT_NORM_HEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)(?:en\s+vertu\s+de|resultant\s+de|prevue?s?\s+par|institue(?:es?|s)?\s+par
          |issue?s?\s+de|en\s+application\s+de|sur\s+le\s+fondement\s+de|au\s+sens\s+de
          |regie?s?\s+par|abrogee?s?\s+par|modifiee?s?\s+par|completee?s?\s+par
          |etendue?s?\s+par|annexe\s+[ivx0-9]+\s+de|conformement\s+a)\s*$",
    )
    .unwrap()
});
/// Contexte AVAL où l'acte est SUJET normatif (« L'ordonnance du 25 mars
/// 2020 a institué… », « prévoit que… »).
static RE_ACT_NORM_SUBJ: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^\s+(?:a\s+(?:institue|cree|modifie|abroge|prevu|introduit|etendu|complete)
          |institue|prevoit|dispose|fixe|precise|definit|organise|permet|impose)\b",
    )
    .unwrap()
});
/// Identité DATE-SEULE directement derrière la nature (« du 8 février 2022 »,
/// « n° 23-033 du 30 janvier 2023 », « d'appel du 4 juillet 2017 »).
static RE_ACT_DATE_IDENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:\s+(?:n\s*[°oº]\s*\S+|en\s+date|d'appel|de\s+refere))*\s+(?:du|des)\s+\d")
        .unwrap()
});

/// Acte daté INDIVIDUEL (arrêté préfectoral, ordonnance du juge, délibération
/// locale, déclaration d'appel, convention entre parties, acte attaqué…) :
/// hors périmètre citations. Un acte de ces natures à identité date-seule
/// n'émet que sur signal RÉGLEMENTAIRE : n° à millésime, queue de titre
/// officielle, contexte normatif amont/aval, ou position de visa (« Vu l'… »,
/// « ; - l'… ; »).
fn dated_individual_act(folded: &str, bs: usize, be: usize, resolved: bool) -> bool {
    let m = &folded[bs..be];
    let nature = &m[..m.find(' ').unwrap_or(m.len())];
    let gate = match nature {
        "arrete" | "ordonnance" | "deliberation" | "circulaire" => true,
        "declaration" | "convention" | "accord" => !resolved,
        _ => false,
    };
    if !gate || !RE_ACT_DATE_IDENT.is_match(&m[nature.len()..]) {
        return false;
    }
    if RE_ACT_NORM_NUM.is_match(m)
        || RE_ACT_NOR_TOKEN.is_match(m)
        || RE_ACT_NORM_TAIL_IN.is_match(m)
        || RE_ACT_NORM_TAIL.is_match(&folded[be..])
        || RE_ACT_NORM_SUBJ.is_match(&folded[be..])
    {
        return false;
    }
    let head = folded[..bs].trim_end();
    let head = head
        .strip_suffix("l'")
        .or_else(|| head.strip_suffix("1'"))
        .or_else(|| head.strip_suffix("la"))
        .or_else(|| head.strip_suffix("le"))
        .unwrap_or(head)
        .trim_end();
    let mut hs = head.len().saturating_sub(40);
    while !head.is_char_boundary(hs) {
        hs += 1;
    }
    if RE_ACT_NORM_HEAD.is_match(&head[hs..]) {
        return false;
    }
    let visa_head = head.ends_with(" vu") || head == "vu" || head.ends_with('-');
    let after_punct = folded[be..]
        .trim_start()
        .starts_with([';', ',', '.', ')', ':']);
    !(visa_head && after_punct)
}

/// Intervalles de quote textuelle (« Aux termes de l'article X du CODE :
/// "…" ») en offsets chars. Ouvrante = guillemet précédé d'un « : » ; les
/// paires ambiguës sans deux-points sont ignorées.
fn quote_spans(chars: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let closer = match chars[i] {
            '«' => Some('»'),
            '"' => Some('"'),
            _ => None,
        };
        let colon_before = || {
            chars[..i]
                .iter()
                .rev()
                .find(|c| !c.is_whitespace())
                .is_some_and(|c| *c == ':')
        };
        if let Some(closer) = closer {
            if colon_before() {
                if let Some(j) = chars[i + 1..].iter().position(|&c2| c2 == closer) {
                    let end = i + 1 + j;
                    out.push((i, end));
                    i = end + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// (surface, text_key, is_code) d'une mention d'instrument.
fn instr_keys<'a>(t: &'a Tok, chars: &[char]) -> Option<(String, &'a str, bool)> {
    let Tok::Instr {
        s,
        e,
        text_key,
        is_code,
        ..
    } = t
    else {
        return None;
    };
    let surface: String = chars[*s..*e].iter().collect();
    Some((surface, text_key.as_str(), *is_code))
}

/// Clé d'appariement de l'adoption intra-document d'une forme datée nue : pli
/// stable + qualificatif de circulaire raboté.
fn dated_adoption_key(text_key: &str) -> String {
    fold_chars(
        &crate::link::strip_circulaire_qualifier(text_key).unwrap_or_else(|| text_key.to_string()),
    )
}

/// « la circulaire du 28 novembre 2012 relative aux conditions d'examen… » :
/// mention datée NUE (le lexer borne à la date), queue-objet dans le texte
/// après la mention — auteur (« du ministre de l'intérieur ») enjambé,
/// connecteur explicite exigé. Départage via
/// [`LinkSnapshot::circulaire_by_object_tail`].
fn circ_tail_target(
    t: &Tok,
    folded: &str,
    char2byte: &[usize],
    snap: &LinkSnapshot,
) -> Option<String> {
    let Tok::Instr { s, e, .. } = t else {
        return None;
    };
    let mention = &folded[char2byte[*s]..char2byte[*e]];
    let mention = crate::link::strip_circulaire_qualifier(mention)
        .map_or(std::borrow::Cow::Borrowed(mention), std::borrow::Cow::Owned);
    if !mention.starts_with("circulaire du ") || !RE_BARE_DATED_ALIAS.is_match(&mention) {
        return None;
    }
    let date = crate::link::first_date(&mention)?;
    let win = case_window(folded, char2byte[*e], 200);
    let tail = RE_CIRC_DOC_TAIL
        .captures(win)?
        .get(1)
        .expect("groupe queue")
        .as_str();
    snap.circulaire_by_object_tail(date, tail)
}

/// Résolution mémoïsée par document : l'analyse de clé ([`KeyAnalysis`]) se
/// calcule une fois par `text_key`, la cible une fois par (text_key,
/// article_key).
struct Resolver<'a> {
    snap: &'a LinkSnapshot,
    vocab: &'a CompiledVocab,
    /// The place of the body's legal order when it is not France's, constant over the
    /// document.
    owner_place: Option<&'a str>,
    analyses: FxHashMap<String, std::sync::Arc<KeyAnalysis>>,
    memo: FxHashMap<(String, Option<String>), LinkTarget>,
}

/// Cache statique des [`KeyAnalysis`] : fonction PURE de `text_key` (pliages +
/// cascade `key_signals`, aucun snapshot) — partageable entre runs, docs et
/// threads.
static KEY_ANALYSES: std::sync::OnceLock<
    std::sync::RwLock<FxHashMap<String, std::sync::Arc<KeyAnalysis>>>,
> = std::sync::OnceLock::new();

fn key_analysis(text_key: &str) -> std::sync::Arc<KeyAnalysis> {
    let cache = KEY_ANALYSES.get_or_init(|| std::sync::RwLock::new(FxHashMap::default()));
    if let Some(a) = cache.read().unwrap().get(text_key) {
        return a.clone();
    }
    let a = std::sync::Arc::new(KeyAnalysis::new(text_key));
    cache
        .write()
        .unwrap()
        .entry(text_key.to_string())
        .or_insert(a)
        .clone()
}

impl Resolver<'_> {
    fn analysis(&mut self, text_key: &str) -> &KeyAnalysis {
        if !self.analyses.contains_key(text_key) {
            self.analyses
                .insert(text_key.to_string(), key_analysis(text_key));
        }
        &self.analyses[text_key]
    }

    fn resolve(&mut self, instrument: &str, text_key: &str, ak: Option<&str>) -> LinkTarget {
        let k = (text_key.to_string(), ak.map(str::to_string));
        if let Some(t) = self.memo.get(&k) {
            return t.clone();
        }
        let gk = (
            instrument.to_string(),
            text_key.to_string(),
            ak.map(str::to_string),
            self.owner_place.map(str::to_string),
        );
        if let Some(t) = self.vocab.link_cache.read().unwrap().get(&gk) {
            self.memo.insert(k, t.clone());
            return t.clone();
        }
        self.analysis(text_key);
        let target = link_citation_analyzed(
            self.snap,
            instrument,
            &self.analyses[text_key],
            ak,
            self.owner_place,
        );
        self.memo.insert(k, target.clone());
        self.vocab
            .link_cache
            .write()
            .unwrap()
            .insert(gk, target.clone());
        target
    }

    fn citable(&mut self, text_key: &str) -> bool {
        self.analysis(text_key).signals().citability == Citability::Citable
    }
}

#[allow(clippy::too_many_arguments)]
fn compose(
    toks: &[Tok],
    chars: &[char],
    folded: &str,
    char2byte: &[usize],
    breaks: &[bool],
    vocab: &CompiledVocab,
    snap: &LinkSnapshot,
    forum: Option<Forum>,
    owner_text_uid: Option<&str>,
) -> Vec<CompiledCitation> {
    let (place, date) = match forum {
        Some(Forum::Foreign(place, date)) => (Some(place), date),
        _ => (None, None),
    };
    let mut resolver = Resolver {
        snap,
        vocab,
        owner_place: match &place {
            Some(place) => Some(place.as_str()),
            None => owner_text_uid.and_then(|u| snap.foreign_territory(u)),
        },
        analyses: FxHashMap::default(),
        memo: FxHashMap::default(),
    };

    // Registre document : uids des instruments résolus, pour l'unicité des
    // articles nus.
    let mut doc_uids: FxHashSet<String> = FxHashSet::default();
    let mut instr_resolved: FxHashMap<usize, (String, String, Option<String>)> =
        FxHashMap::default();
    for (i, t) in toks.iter().enumerate() {
        if let Some((surface, tk, _)) = instr_keys(t, chars) {
            let mut target = resolver.resolve(&surface, tk, None);
            if target.ref_text_uid.is_none() {
                if let Some(uid) = circ_tail_target(t, folded, char2byte, snap) {
                    target = LinkTarget {
                        ref_text_uid: Some(uid),
                        ref_num_key: None,
                    };
                }
            }
            if let Some(uid) = &target.ref_text_uid {
                doc_uids.insert(uid.clone());
            }
            instr_resolved.insert(i, (surface, tk.to_string(), target.ref_text_uid));
        }
    }
    let mut dated_tk_uid: FxHashMap<String, Option<String>> = FxHashMap::default();
    for (_, tk, uid) in instr_resolved.values() {
        let Some(uid) = uid else { continue };
        let ftk = dated_adoption_key(tk);
        if !RE_BARE_DATED_ALIAS.is_match(&ftk) {
            continue;
        }
        match dated_tk_uid.entry(ftk) {
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(Some(uid.clone()));
            }
            std::collections::hash_map::Entry::Occupied(mut e) => {
                if e.get().as_deref() != Some(uid) {
                    e.insert(None);
                }
            }
        }
    }
    for (i, t) in toks.iter().enumerate() {
        if !matches!(t, Tok::Instr { .. }) {
            continue;
        }
        let Some((_, tk, None)) = instr_resolved.get(&i).cloned() else {
            continue;
        };
        if let Some(Some(uid)) = dated_tk_uid.get(&dated_adoption_key(&tk)) {
            let uid = uid.clone();
            doc_uids.insert(uid.clone());
            if let Some(entry) = instr_resolved.get_mut(&i) {
                entry.2 = Some(uid);
            }
        }
    }
    let full_forms: Vec<(String, String, String)> = instr_resolved
        .values()
        .filter_map(|(surface, tk, uid)| Some((fold_chars(surface), tk.clone(), uid.clone()?)))
        .collect();
    for (i, t) in toks.iter().enumerate() {
        let Tok::Instr {
            s, e, treaty_short, ..
        } = t
        else {
            continue;
        };
        if instr_resolved
            .get(&i)
            .is_some_and(|(_, _, uid)| uid.is_some())
        {
            continue;
        }
        let surface: String = chars[*s..*e].iter().collect();
        let folded = fold_chars(&surface);
        if !*treaty_short && !RE_BARE_DATED_ALIAS.is_match(&folded) {
            continue;
        }
        let prefix = format!("{folded} ");
        let mut hits = full_forms.iter().filter(|(f, _, _)| f.starts_with(&prefix));
        if let Some((_, tk, uid)) = hits.next() {
            if hits.all(|(_, _, u)| u == uid) {
                instr_resolved.insert(i, (surface, tk.clone(), Some(uid.clone())));
            }
        }
    }

    let mut out: Vec<CompiledCitation> = Vec::new();
    let mut last_instr: Option<usize> = None;
    let quotes = quote_spans(chars);
    // A document under a territory's own civil procedure: its bare « article 700 »
    // is that code's.
    let local_procedure = instr_resolved.values().any(|(_, tk, _)| {
        let folded = canon_fold(tk);
        folded.contains("procedure civile") && names_territory(&folded)
    });

    // Adjacence génitive précalculée pour TOUS les Art, puis héritage
    // d'énumération EN ARRIÈRE : seul le dernier item touche le génitif, les
    // précédents en héritent quand seuls des Art les séparent et qu'ils sont
    // contigus (≤ 30 chars).
    let mut adj: FxHashMap<usize, usize> = FxHashMap::default();
    for (i, t) in toks.iter().enumerate() {
        let Tok::Art { e, .. } = t else { continue };
        let following = toks[i..]
            .iter()
            .enumerate()
            .take_while(|(_, t2)| tok_start(t2) <= e + 40)
            .find_map(|(j, t2)| match t2 {
                Tok::Instr { s: is, .. } => {
                    genitive_gap(&folded[char2byte[*e]..char2byte[*is]]).then_some(i + j)
                }
                _ => None,
            });
        if let Some(j) = following {
            adj.insert(i, j);
        }
    }
    let art_idx: Vec<usize> = toks
        .iter()
        .enumerate()
        .filter(|(_, t)| matches!(t, Tok::Art { .. }))
        .map(|(i, _)| i)
        .collect();
    for w in art_idx.windows(2).rev() {
        let (i, k) = (w[0], w[1]);
        if adj.contains_key(&i) {
            continue;
        }
        let Some(&target) = adj.get(&k) else { continue };
        let (Tok::Art { e, num_key: hk, .. }, Tok::Art { s, .. }) = (&toks[i], &toks[k]) else {
            unreachable!()
        };
        if owner_text_uid.is_some_and(|u| snap.has_article(u, hk))
            && RE_COORD_ART.is_match(&folded[char2byte[*e]..char2byte[*s]])
        {
            continue;
        }
        let mut gap_from = *e;
        let mut chain = true;
        for t in &toks[i + 1..k] {
            match t {
                Tok::Art { .. } => {}
                Tok::Instr { s: is, e: ie, .. }
                    if is.saturating_sub(gap_from) <= 40 && {
                        let lead: String = chars[gap_from..*is].iter().collect();
                        RE_REDACTION.is_match(fold_chars(&lead).trim_start_matches([',', ' ']))
                    } =>
                {
                    gap_from = *ie;
                }
                _ => {
                    chain = false;
                    break;
                }
            }
        }
        if chain && s.saturating_sub(gap_from) <= 40 {
            adj.insert(i, target);
        }
    }

    // Génitif orphelin : test textuel (partagé avec l'émission) + héritage
    // d'énumération EN ARRIÈRE, symétrique de `adj`. Une anaphore qui suit ne
    // propage pas : chaque item la résout à son compte.
    let orphan_after = |e: usize| {
        let rest = &folded[char2byte[e]..];
        RE_ORPHAN.is_match(rest)
            || (place.is_some() && RE_POSSESSIVE_GEN.is_match(rest))
            || RE_ZONE_STEP
                .captures(rest)
                .is_some_and(|c| !matches!(&c[1], "ou" | "ni" | "et"))
    };
    let mut orphan_heads: FxHashSet<usize> = FxHashSet::default();
    for w in art_idx.windows(2).rev() {
        let (i, k) = (w[0], w[1]);
        if adj.contains_key(&i) || adj.contains_key(&k) {
            continue;
        }
        let (Tok::Art { e, .. }, Tok::Art { s, e: ke, .. }) = (&toks[i], &toks[k]) else {
            unreachable!()
        };
        let k_orphan = orphan_heads.contains(&k)
            || (orphan_after(*ke)
                && !toks[k..]
                    .iter()
                    .take_while(|t2| tok_start(t2) <= ke + 60)
                    .any(|t2| matches!(t2, Tok::Same { .. })));
        if k_orphan
            && s.saturating_sub(*e) <= 40
            && toks[i + 1..k].iter().all(|t| matches!(t, Tok::Art { .. }))
        {
            orphan_heads.insert(i);
        }
    }

    // Consommation des mentions par les articles adjacents — dérivée d'`adj`
    // AVANT toute émission : une mention consommée par ≥ 1 article n'émet pas de
    // span propre. Une mention TÊTE reste émise.
    let mut consumed: FxHashMap<usize, u32> = FxHashMap::default();
    for &j in adj.values() {
        if instr_resolved.contains_key(&j) {
            *consumed.entry(j).or_insert(0) += 1;
        }
    }
    // Spans d'instrument qui ÉMETTRONT : les tokens d'article intérieurs à l'un
    // d'eux appartiennent au titre et ne sont jamais émis en span propre.
    let instr_emits: Vec<(usize, usize)> = toks
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let Tok::Instr {
                s, e, weak, nested, ..
            } = t
            else {
                return None;
            };
            if *nested || consumed.get(&i).copied().unwrap_or(0) >= 1 {
                return None;
            }
            let (_, tk, uid) = instr_resolved.get(&i)?;
            if uid.is_none() && (*weak || !resolver.citable(tk)) {
                return None;
            }
            if dated_individual_act(folded, char2byte[*s], char2byte[*e], uid.is_some())
                && !snap.has_text_alias(tk)
            {
                return None;
            }
            Some((*s, *e))
        })
        .collect();

    for (i, t) in toks.iter().enumerate() {
        let Tok::Art {
            s,
            e,
            surface,
            num_key,
        } = t
        else {
            if matches!(t, Tok::Instr { .. }) && instr_resolved.contains_key(&i) {
                last_instr = Some(i);
            }
            continue;
        };
        if instr_emits.iter().any(|&(is, ie)| *s >= is && *s < ie) {
            continue;
        }
        if place.is_some() && bracketed_paragraph(chars, *s, *e) {
            continue;
        }
        let ak = Some(num_key.clone());

        // 2. Anaphore collée (« R. 57-1 du même livre ») — porte la nature.
        //    Le possessif (« en vertu de son article 7 ») PRÉCÈDE le numéral :
        //    cherché aussi en arrière immédiat.
        let same_nat = toks[i..]
            .iter()
            .take_while(|t2| tok_start(t2) <= e + 60)
            .find_map(|t2| match t2 {
                Tok::Same { nature, .. } => Some(nature.as_str()),
                _ => None,
            })
            .or_else(|| {
                toks[..i].iter().rev().take(2).find_map(|t2| match t2 {
                    Tok::Same { s: ss, nature } if s.saturating_sub(*ss) <= 30 => {
                        Some(nature.as_str())
                    }
                    _ => None,
                })
            });

        let mut resolved: Option<(String, String)> = None; // (surface, text_key)
        let mut adjacent = false;
        // 1. Instrument SUIVANT génitivement adjacent (« L. 761-1 du CJA »),
        //    héritage d'énumération inclus. L'adjacence est AUTORITAIRE : l'article
        //    appartient syntaxiquement à cet instrument.
        if let Some(&j) = adj.get(&i) {
            if let Some((surface, tk, _)) = instr_resolved.get(&j) {
                resolved = Some((surface.clone(), tk.clone()));
            }
            adjacent = true;
        }
        // 3. Anaphore → mention PRÉCÉDENTE la plus proche dont la clé porte la
        //    nature citée (« du même livre » → « livre des procédures fiscales ») ET
        //    qui possède l'article au catalogue.
        if resolved.is_none() && !adjacent {
            if let Some(nat) = same_nat {
                for j in (0..i).rev() {
                    let Some((surface, tk, uid)) = instr_resolved.get(&j) else {
                        continue;
                    };
                    if nat.is_empty() {
                        let Tok::Instr { e: ie, .. } = &toks[j] else {
                            continue;
                        };
                        if s.saturating_sub(*ie) > 150 {
                            break;
                        }
                    }
                    if nat != "texte" && !starts_with_folded(tk, nat) {
                        continue;
                    }
                    if uid
                        .as_deref()
                        .is_some_and(|u| snap.has_article(u, num_key) || !snap.has_article_info(u))
                    {
                        resolved = Some((surface.clone(), tk.clone()));
                        break;
                    }
                    // A territory's own code is the one designated, linked or not.
                    if names_territory(&canon_fold(tk)) {
                        break;
                    }
                }
            }
        }
        // A foreign court takes up a nature (« du Traité susvisé »): the last instrument of that
        // nature carrying the article, else none.
        let mut named_nature = false;
        if resolved.is_none() && !adjacent && same_nat.is_none() && place.is_some() {
            if let Some(c) = RE_NATURE_ANAPHORA.captures(&folded[char2byte[*e]..]) {
                named_nature = true;
                let nat = c[1].split_whitespace().collect::<Vec<_>>().join(" ");
                resolved = (0..i).rev().find_map(|j| {
                    let (surface, tk, uid) = instr_resolved.get(&j)?;
                    (starts_with_folded(tk, &nat)
                        && uid.as_deref().is_some_and(|u| {
                            snap.has_article(u, num_key) || !snap.has_article_info(u)
                        }))
                    .then(|| (surface.clone(), tk.clone()))
                });
            }
        }
        // 3bis. Article DANS une quote : il appartient au texte quoté — le cadre
        //       prime sur l'antécédent et sur l'unicité. Cadre = dernière citation
        //       d'article émise ≤ 120 chars avant l'ouvrante, instrument résolu.
        let mut direct_target: Option<LinkTarget> = None;
        if resolved.is_none() && !adjacent {
            if let Some(&(qs, _)) = quotes.iter().find(|(qs, qe)| *s > *qs && *e < *qe) {
                let frame = out
                    .iter()
                    .rev()
                    .find(|c| c.char_end <= qs && c.target.ref_text_uid.is_some())
                    .filter(|c| qs - c.char_end <= 120);
                if let Some(frame) = frame {
                    let uid = frame.target.ref_text_uid.as_deref().unwrap();
                    if snap.has_article(uid, num_key) || !snap.has_article_info(uid) {
                        if frame.text_key.is_empty() {
                            direct_target = Some(LinkTarget {
                                ref_text_uid: Some(uid.to_string()),
                                ref_num_key: snap.num_key_for(uid, Some(num_key)),
                            });
                        } else {
                            resolved = Some((frame.instrument.clone(), frame.text_key.clone()));
                        }
                    }
                }
            }
        }
        // 3ter. Génitif de FORUM (parent implicite par juridiction) : la nature nue
        //       n'est pas une mention reconnue, mais la juridiction qui parle
        //       identifie son texte propre. Nature NUE exigée (ponctuation ou
        //       coordination derrière) ; article validé au catalogue quand la
        //       structure de la cible est connue.
        if resolved.is_none() && !adjacent && direct_target.is_none() {
            if let Some(f) = forum {
                let rest = &folded[char2byte[*e]..];
                if let Some(g) = RE_FORUM_GEN
                    .captures(rest)
                    .and_then(|c| c.get(1))
                    .filter(|g| genitive_gap(&rest[..g.start()]))
                {
                    if let Some(uid) = snap.forum_default(f, g.as_str()) {
                        if snap.has_article(uid, num_key) || !snap.has_article_info(uid) {
                            direct_target = Some(LinkTarget {
                                ref_text_uid: Some(uid.to_string()),
                                ref_num_key: snap.num_key_for(uid, Some(num_key)),
                            });
                        }
                    }
                }
            }
        }
        // Génitif ORPHELIN : un connecteur suit mais aucun instrument n'a été
        // reconnu. L'article appartient syntaxiquement à cet inconnu : le span sort
        // sans cible, les têtes d'énumération héritent (`orphan_heads`).
        let orphan = resolved.is_none()
            && !adjacent
            && (named_nature || orphan_heads.contains(&i) || orphan_after(*e));

        // 3ter. Désignation DÉICTIQUE du porteur (« … 33, 50, 51, 53, 69 et 91 du
        //       présent Code ») : le génitif ne pointe pas un instrument inconnu, il
        //       nomme le document qui parle. Réservée aux corps du référentiel.
        if orphan && direct_target.is_none() {
            if let Some(uid) = owner_text_uid
                .filter(|u| snap.has_article(u, num_key))
                .filter(|_| RE_SELF_GEN.is_match(&folded[char2byte[*e]..]))
            {
                direct_target = Some(LinkTarget {
                    ref_text_uid: Some(uid.to_string()),
                    ref_num_key: snap.num_key_for(uid, Some(num_key)),
                });
            }
        }

        // 4. Antécédent proche (article nu en prose) — validé catalogue.
        if resolved.is_none() && !adjacent && !orphan {
            if let Some(j) = last_instr {
                if let Some((surface, tk, Some(uid))) = instr_resolved.get(&j) {
                    let dist = s.saturating_sub(tok_start(&toks[j]));
                    if dist <= 1500 && snap.has_article(uid, num_key) {
                        resolved = Some((surface.clone(), tk.clone()));
                    }
                }
            }
        }
        // 5. Unicité : porteurs catalogue de l'article ∩ instruments du doc.
        //    À égalité (≥ 2 porteurs cités), départage par la citation RÉSOLUE la
        //    plus proche en amont dont la cible est candidate. Clés PRÉFIXÉES
        //    seulement (L./R./D./A.).
        //    Corps du référentiel : le texte PORTEUR revendique ses propres numéraux
        //    nus (« régie par les dispositions des articles 1049 à 1055 » dans le
        //    CPC = le CPC) ; l'unicité s'abstient alors, l'auto-résolution tranche en
        //    aval sur le porteur.
        let owner_claims = owner_text_uid.is_some_and(|u| snap.has_article(u, num_key));
        if resolved.is_none() && !adjacent && !orphan && direct_target.is_none() && !owner_claims {
            let public_key = lj_norm::article_key::article_key(num_key);
            if let Some(cands) = vocab.by_num_key.get(&public_key) {
                let prefixed = num_key.len() > 3
                    && matches!(num_key.as_bytes()[0], b'L' | b'R' | b'D' | b'A')
                    && num_key[1..].starts_with(". ");
                let mut inter = doc_uids.iter().filter(|u| cands.binary_search(*u).is_ok());
                let uid = match (inter.next(), inter.next()) {
                    (Some(uid), None) => Some(uid.as_str()),
                    (Some(_), Some(_)) if prefixed => {
                        let mut in_window = out
                            .iter()
                            .rev()
                            .take_while(|c| s.saturating_sub(c.char_end) <= 1500)
                            .filter_map(|c| c.target.ref_text_uid.as_deref())
                            .filter(|u| cands.binary_search_by(|c| c.as_str().cmp(u)).is_ok());
                        match in_window.next() {
                            Some(u) if in_window.all(|v| v == u) => Some(u),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                if let Some(uid) = uid {
                    direct_target = Some(LinkTarget {
                        ref_text_uid: Some(uid.to_string()),
                        ref_num_key: Some(public_key),
                    });
                }
            }
        }
        // 5bis. Consistance de document : le même numéral déjà RÉSOLU en amont —
        //       adopté si TOUTES les résolutions antérieures de ce num_key visent la
        //       même cible.
        if resolved.is_none() && !adjacent && !orphan && direct_target.is_none() {
            let mut prior = out
                .iter()
                .filter(|c| ak.is_some() && c.article_key == ak && c.target.ref_text_uid.is_some());
            if let Some(first) = prior.next() {
                if prior.all(|c| c.target.ref_text_uid == first.target.ref_text_uid) {
                    direct_target = Some(first.target.clone());
                }
            }
        }
        // 6. Antécédent NON VALIDABLE : le catalogue ne connaît aucun article de la
        //    cible (traités JORF sans structure) ; l'existence n'étant pas réfutable,
        //    l'antécédent serré (≤ 600) est accepté une fois l'unicité épuisée.
        if resolved.is_none() && !adjacent && !orphan && direct_target.is_none() {
            if let Some(j) = last_instr {
                if let Some((surface, tk, Some(uid))) = instr_resolved.get(&j) {
                    let dist = s.saturating_sub(tok_start(&toks[j]));
                    let treaty_like = [
                        "convention",
                        "traite",
                        "accord",
                        "protocole",
                        "charte",
                        "reglement",
                        "directive",
                    ]
                    .iter()
                    .any(|p| starts_with_folded(tk, p));
                    if dist <= 1200 && treaty_like && !snap.has_article_info(uid) {
                        resolved = Some((surface.clone(), tk.clone()));
                    }
                }
            }
        }
        // 6ter. Article nu de STYLE Convention dans une décision CEDH (« Art. 13 -
        //       lack of any effective remedy », « l'article 5 § 3 ») : la Cour cite
        //       son propre texte. Style exigé (marqueur abrégé « art. » ou « § »
        //       derrière) ; article validé au catalogue.
        if resolved.is_none() && !adjacent && !orphan && direct_target.is_none() {
            if let Some(uid) =
                (forum == Some(Forum::Cedh)).then(|| snap.forum_default(Forum::Cedh, "convention"))
            {
                let head = &folded[char2byte[s.saturating_sub(6)]..char2byte[*s]];
                let styled =
                    head.contains("art.") || folded[char2byte[*e]..].trim_start().starts_with('§');
                if styled {
                    if let Some(uid) = uid.filter(|u| snap.has_article(u, num_key)) {
                        direct_target = Some(LinkTarget {
                            ref_text_uid: Some(uid.to_string()),
                            ref_num_key: Some(lj_norm::article_key::article_key(num_key)),
                        });
                    }
                }
            }
        }
        // 7. Canon jurisprudentiel : « l'article 700 » nu = frais irrépétibles du
        //    CPC, « 699 » = distraction des dépens — cités sans instrument dans
        //    TOUTES les juridictions judiciaires.
        if resolved.is_none() && !adjacent && !orphan && direct_target.is_none() && !local_procedure
        {
            if let Some(uid) = CANON_BARE_ARTICLES
                .iter()
                .find_map(|(nk, uid)| (nk == num_key).then_some(*uid))
            {
                direct_target = Some(LinkTarget {
                    ref_text_uid: Some(uid.to_string()),
                    ref_num_key: Some(num_key.clone()),
                });
            }
        }

        // Article NU : aucun instrument rattaché NI TENTÉ — pas de génitif, pas
        // d'anaphore/antécédent/unicité/canon.
        let bare = resolved.is_none() && !adjacent && !orphan && direct_target.is_none();
        let (instrument, text_key, target) = match (resolved, direct_target) {
            (Some((surface, tk)), _) => {
                let target = resolver.resolve(&surface, &tk, ak.as_deref());
                (surface, tk, target)
            }
            (None, Some(t)) => (String::new(), String::new(), t),
            (None, None) => (String::new(), String::new(), LinkTarget::default()),
        };
        // La locution « et suivants » qui suit le numéro entre dans le span et pose
        // le signal `suivants` — la cible reste l'ancre.
        let ext = suivants_extension(chars, *e);
        out.push(CompiledCitation {
            char_start: *s,
            char_end: *e + ext,
            instrument,
            article: Some(surface.clone()),
            text_key,
            article_key: ak,
            target,
            suivants: ext > 0,
            bare,
        });
    }

    // Mentions d'instrument : span propre sauf si consommée par ≥ 1 article
    // adjacent (génitif simple ou énumération). Les mentions faibles et les actes
    // non citables n'émettent que résolues.
    for (i, t) in toks.iter().enumerate() {
        if let Tok::Instr {
            s, e, weak, nested, ..
        } = t
        {
            if *nested || consumed.get(&i).copied().unwrap_or(0) >= 1 {
                continue;
            }
            let Some((surface, tk, uid)) = instr_resolved.get(&i) else {
                continue;
            };
            if uid.is_none() {
                if *weak {
                    continue;
                }
                if !resolver.citable(tk) {
                    continue;
                }
            }
            if dated_individual_act(folded, char2byte[*s], char2byte[*e], uid.is_some())
                && !snap.has_text_alias(tk)
            {
                continue;
            }
            let mut target = resolver.resolve(surface, tk, None);
            if target.ref_text_uid.is_none() {
                if let Some(uid) = uid {
                    target = LinkTarget {
                        ref_text_uid: Some(uid.clone()),
                        ref_num_key: None,
                    };
                }
            }
            out.push(CompiledCitation {
                char_start: *s,
                char_end: *e,
                instrument: surface.clone(),
                article: None,
                text_key: tk.clone(),
                article_key: None,
                target,
                suivants: false,
                bare: false,
            });
        }
    }
    if let Some(place) = &place {
        crate::foreign::apply(&mut out, chars, breaks, snap, place.as_str(), date);
    }
    out.sort_by_key(|c| c.char_start);
    out
}

/// Locution « et suivants » immédiatement après un numéro d'article : accord
/// féminin (« dispositions 1373 et suivantes »), abréviation « et s. »,
/// virgule OCR (« , et suivants »). Le char qui borne le mot (groupe non
/// capturé) n'entre jamais dans le span.
static RE_ART_SUIVANTS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^([,\s]{1,3}et\s+(?:suivant(?:e?s)?|s\.))(?:[^\p{L}\p{N}]|$)").unwrap()
});

/// Longueur (en chars) de l'extension « et suivants » à partir de `e` — 0 si
/// le numéro n'est pas suivi de la locution.
fn suivants_extension(chars: &[char], e: usize) -> usize {
    if !matches!(chars.get(e), Some(&(' ' | ','))) {
        return 0;
    }
    let win: String = chars[e..chars.len().min(e + 24)].iter().collect();
    RE_ART_SUIVANTS
        .captures(&win)
        .map_or(0, |c| c[1].chars().count())
}

// ── regexes des lexers (compilées une fois, appliquées à des positions) ─────

/// Qualificatifs d'identité d'un acte FR entre la nature et le n°/la date
/// (« loi organique », « arrêté préfectoral », « loi du pays n° … »).
static RE_LEX_DATED_QUAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:\s+(?:organique|constitutionnelle|du\s+pays|prefectorale?|ministerielle?|interministerielle?|municipale?|communale?|royal|federale?|susvisee?|susmentionnee?|precitee?))*",
    )
    .unwrap()
});
/// The author a circulaire names before its date, « du ministre de
/// l'intérieur », « de la Première ministre » : group 1, up to the « du » the
/// date opens with.
static RE_LEX_CIRC_AUTHOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(\s+(?:du|des|de\s+la)\s+(?:premiere?\s+)?minist(?:re|res|ere)\b(?:\s*,?\s+[\p{L}'\u{2019}-]+){0,12}?)\s+(?:en\s+date\s+)?du\s+\d",
    )
    .unwrap()
});
/// Numéro d'acte FR : préfixé « n°/no » (numéro libre) ou nu (forme NN-NNN
/// exigée).
static RE_LEX_NUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s+(?:n\s*[°oº]\s*\.?\s*[\w./-]*\d[\w./-]*|\d+-\d[\w./-]*)").unwrap()
});
/// Date d'acte : « du 1er janvier 2020 », « des 16 et 24 août 1790 »,
/// « en date du 5 juin 2015 », « du 16/01/2018 », « du 09-07-1991 ».
static RE_LEX_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^\s+(?:en\s+date\s+)?(?:
            (?:du|des)\s+\d{1,2}(?:er)?(?:\s*(?:,|et|-|\u{2013})\s*\d{1,2}(?:er)?)*\s+(?:janvier|fevrier|mars|avril|mai|juin|juillet|aout|septembre|octobre|novembre|decembre)\s+\d{4}
          | du\s+\d{1,2}\s*[/.-]\s*\d{1,2}\s*[/.-]\s*\d{4}
        )",
    )
    .unwrap()
});
/// « Constitution de 1958 » (millésime nu).
static RE_LEX_CONST_YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+de\s+(?:1958|1946)\b").unwrap());
/// Identité UE : sigle parenthésé ou nu, « n° » optionnel, numéro à barre
/// oblique — « (UE) n° 604/2013 », « 2008/115/CE », « d'exécution (UE)
/// 2015/1523 », « n° 1/80 ».
static RE_LEX_EU: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^
        (?:\s+(?:d'execution|deleguee?|d'application))?
        (?:\s*\(\s*(?:ue|ce|cee|euratom)(?:\s*,\s*(?:ue|ce|cee|euratom))?\s*\))?
        (?:\s+(?:ue|ce|cee|c\.\s*e\.|communautaire))?
        (?:\s*,?\s*n\s*[°oº]\s*)?
        \s*\d{1,4}\s*/\s*\d{1,4}
        (?:\s*/\s*(?:ce|cee|ue|euratom))?
        # Identité institutionnelle écrite en toutes lettres (l'oracle capture
        # l'identité complète) : « … du Parlement européen et du
        # Conseil du 12 décembre 2012 ».
        (?:
          \s+(?:du\s+parlement\s+europeen\s+et\s+du\s+conseil|du\s+conseil|de\s+la\s+commission)
          (?:\s+(?:en\s+date\s+)?du\s+\d{1,2}(?:er)?\s+[a-z]+\s+\d{4})?
        )?",
    )
    .unwrap()
});
/// Identité UE DATÉE sans numéro — « règlement (UE) du 17 décembre 2013 »,
/// « règlement du Conseil, du 21 décembre 1992 » : sigle et institutions
/// optionnels, la date obligatoire.
static RE_LEX_EU_DATED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^
        (?:\s*\(\s*(?:ue|ce|cee|euratom|ceca)(?:\s*,\s*(?:ue|ce|cee|euratom|ceca))*\s*\))?
        (?:\s+(?:du\s+parlement\s+europeen\s+et\s+du\s+conseil|du\s+conseil|de\s+la\s+commission))?
        \s*,?\s+du\s+\d{1,2}(?:er)?\s+(?:janvier|fevrier|mars|avril|mai|juin|juillet|aout|septembre|octobre|novembre|decembre)\s+\d{4}\b",
    )
    .unwrap()
});
/// Marqueur de numéro : « n° », « n°s », « no », « n » nu (la forme stricte
/// du pourvoi discrimine), « RG : » (bandeaux d'en-tête), parenthèse ouvrante
/// (« par arrêt en date du 29 juin 2017 (16-13.988) »), date de l'arrêt cité
/// (« Cass, com, 18 décembre 2019, 18-14.827 » — n° omis).
const CC_MARK: &str = r"(?x)(?:\bn\s*[°oº]?\s*s?\s*[:.]?\s*|\brg\s*:?\s*|\(\s*
    |\d{1,2}(?:er)?\s+[a-z]+\.?\s+\d{4}\s*[-,']\s*)";
/// Date intercalée dans une énumération (« ; 25 juin 1996, n 94-15130 »).
const CC_DATE: &str = r"(?:\d{1,2}(?:er)?\s+[a-z]+\.?\s+\d{4}\s*[-,']?\s*)?";
/// Pourvoi ancré derrière « pourvoi(s) » : n° optionnel, lettre de série
/// collée tolérée (« n° J01-01.208 »).
static RE_CASE_POURVOI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?x)^\s*\(?\s*(?:en\s+cassation\s+)?(?:incidents?\s+|principaux\s+|principal\s+)?
          (?:,?\s*\(?\s*n\s*[°oº]?\s*s?\s*[:.]?\s*)?(?:[a-z]\s*)?({POURVOI})\b"
    ))
    .unwrap()
});
/// Pourvoi en fenêtre (« Cass. 3e civ., 16 mars 2022, n° 18-23.954 ») : un
/// marqueur est exigé.
static RE_CASE_POURVOI_WIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"{CC_MARK}\s*(?:[a-z]\s*)?({POURVOI})\b")).unwrap());
/// Maillon d'énumération : « et 18-23.955 », « au n° W 00-44.846 » (plage),
/// « à W 00-44.846 » (« à » plié en « a »), date intercalée (« ; 25 juin
/// 1996, n 94-15130 »).
static RE_CASE_POURVOI_NEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?x)^\s*(?:,\s*(?:et\s+)?|et\s+|au?\s+|;\s*){CC_DATE}
          (?:n\s*[°oº]?\s*s?\s*[:.]?\s*)?(?:[a-z]\s*)?({POURVOI})\b"
    ))
    .unwrap()
});
/// N° de requête CE : 5-6 chiffres nus derrière « n° » / « nos » / « sous le
/// numéro » — jamais sans le contexte CE (l'ancre).
static RE_CASE_CE_WIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:sous\s+le\s+)?(?:n\s*[°oº]\s*s?|numeros?)\s*[:.]?\s*(\d{5,6})\b").unwrap()
});
/// Maillon d'énumération CE : nom de partie toléré (« n° 476000 de la
/// société Accorinvest et n° 476009 »).
static RE_CASE_CE_NEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:[a-z'’.-]+\s+){0,4}(?:,|et)\s*(?:n\s*[°oº]\s*s?\s*)?(\d{5,6})\b").unwrap()
});
/// « il s'est pourvu en cassation sous le n° 500109 » : pourvoi administratif
/// (requête CE 6 chiffres), collé à l'ancre « cassation ».
static RE_CASE_CASSATION_CE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*sous\s+le\s+n\s*[°oº]\s*s?\s*(\d{5,6})\b").unwrap());
/// « dans son arrêt n° 350095 du 28 mai 2014 », « dans un arrêt du 10 juillet
/// 2019, n°417919 » : requête CE derrière « arrêt » — le contexte « Conseil
/// d'État » (amont ou aval) est vérifié en code.
static RE_CASE_ARRET_CE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^\s*(?:du\s+\d{1,2}(?:er)?\s+[a-z]+\.?\s+\d{4}\s*,?\s*)?
          n\s*[°oº]\s*s?\s*(\d{5,6})\b",
    )
    .unwrap()
});
/// « Par une ordonnance n° 468345 du 15 novembre 2022 » : ordonnance CE
/// (5-6 chiffres nus).
static RE_CASE_ORD_CE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*n\s*[°oº]\s*s?\s*(\d{5,6})\b").unwrap());
/// « La requête en référé n° 502527 de Mme D » : référé CE derrière
/// « requête ».
static RE_CASE_REF_CE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*en\s+refere\s+n\s*[°oº]\s*s?\s*(\d{5,6})\b").unwrap());
/// Sonde ARRIÈRE CE : « Par une décision n° 432537 du 8 janvier 2020, le
/// Conseil d'Etat… » — le n° précède l'ancre, gaté sur la nature
/// décision/ordonnance/arrêt.
static RE_CASE_CE_BACK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)(?:decision|ordonnance|arret)s?\s+
          (?:du\s+\d{1,2}(?:er)?\s+[a-z]+\s+\d{4}\s*,?\s*)?
          (?:sous\s+le\s+)?(?:n\s*[°oº]\s*s?|numeros?)\s*[:.]?\s*(\d{5,6})\b",
    )
    .unwrap()
});
/// Une autre juridiction entre le n° et l'ancre CE invalide la sonde arrière
/// (« la décision n° X du tribunal administratif… le Conseil d'Etat »).
static RE_CASE_OTHER_JUR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)tribunal|cour\s+d'appel|cour\s+administrative|conseil\s+constitutionnel
          |cour\s+de\s+cassation|cour\s+europeenne|cour\s+de\s+justice",
    )
    .unwrap()
});
/// Décision du Conseil constitutionnel derrière « décision(s) » : le suffixe
/// DC/QPC identifie la famille, sauf gate « décision » explicite (« dans sa
/// décision n°2015-479 »).
static RE_CASE_CONSTIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*(?:du\s+conseil\s+constitutionnel\s+)?n\s*[°oº]\s*(\d{2,4}-\d{1,4})\s*(?:dc|qpc)\b",
    )
    .unwrap()
});
static RE_CASE_CONSTIT_WIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)n\s*[°oº]\s*s?\s*(\d{2,4}-\d{1,4})\s*(?:dc|qpc)\b
          |decisions?\s+(?:qpc\s+)?n\s*[°oº]\s*s?\s*(\d{2,4}-\d{1,4})\b",
    )
    .unwrap()
});
/// Énumération constit : « n° 2016-554 QPC du 22 juillet 2016, n° 2016-610
/// QPC du 10 février 2017 et n° 2016-614 QPC » — suffixe et date intercalés.
static RE_CASE_CONSTIT_NEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^\s*(?:dc|qpc)?\s*(?:du\s+\d{1,2}(?:er)?\s+[a-z]+\s+\d{4})?\s*
          (?:,|et)\s*n\s*[°oº]\s*s?\s*(\d{2,4}-\d{1,4})\b",
    )
    .unwrap()
});
/// Sonde ARRIÈRE constit : « la décision n°2021-823 du Conseil
/// constitutionnel » — n° avant l'ancre, gaté sur « décision », collé à
/// l'ancre (queue `du` seule tolérée).
static RE_CASE_CONSTIT_BACK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)decisions?\s+(?:qpc\s+)?n\s*[°oº]\s*s?\s*(\d{2,4}-\d{1,4})
          (?:\s*(?:dc|qpc))?\s*(?:,|du)?\s*$",
    )
    .unwrap()
});
/// Date derrière l'ancre TC, discriminant de la juridiction. Le nom de l'acte
/// s'intercale (« Tribunal des conflits, ordonnance du 9 octobre 2017 ») : le
/// vocabulaire en est fermé, et ce qui le précède ne porte ni chiffre ni fin de
/// phrase, sinon « TC de Caen en date du 11 mai 2023 » redeviendrait une
/// citation.
static RE_CASE_TC_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^\s*[,(]?\s*
          (?:[^.;:0-9]{0,25}?\b(?:ordonnance|decision|arret|jugement|avis)\s+)?
          (?:du\s+|le\s+|en\s+date\s+du\s+)?\d{1,2}(?:er)?\s+[a-z]+\.?\s+\d{4}\b",
    )
    .unwrap()
});
/// « la décision n° 3643 en date du 17 décembre 2007 » : le numéro AVANT la
/// date, l'inverse de [`RE_CASE_TC_DATE`]. Nom de l'acte, degré et date sont
/// exigés ensemble, et la classe sans ponctuation forte reste dans la phrase.
static RE_CASE_TC_NUM_DATED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^[^.;:0-9]{0,60}?\b(?:ordonnance|decision|arret|jugement|avis)s?\s+
          n\s*[°oº]?\s*s?\s*:?\s*
          (c\s?\d{3,4}|\d{2}-\d{2}\.\d{3}|\d{2}-\d{4,5}|\d{3,5})\b
          \s*,?\s*(?:du\s+|le\s+|en\s+date\s+du\s+)\d{1,2}(?:er)?\s+[a-z]+\.?\s+\d{4}\b",
    )
    .unwrap()
});
/// Numéro TC derrière la date, nom d'usage toléré. Le degré est optionnel
/// (« n C3952 ») : c'est la forme du numéro qui discrimine, et trois graphies
/// se succèdent au fonds selon l'époque, `C4174` (2005 et après), `3220`
/// (2000-2004), `00012` (avant 2000) ; les citations ajoutent la forme
/// année-numéro (« 14-03.959 », « 99-03174 »).
static RE_CASE_TC_NUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^[^.;:]{0,80}?\bn\s*[°oº]?\s*s?\s*:?\s*
          (c\s?\d{3,4}|\d{2}-\d{2}\.\d{3}|\d{2}-\d{4,5}|\d{3,5})\b",
    )
    .unwrap()
});
/// Maillon daté de l'énumération TC : « n° 03086, du 12 décembre 2005, Commune
/// de Cestas, n° 3485 ». Le point de l'initiale (« M. C. et autres ») traverse,
/// là où le point de fin de phrase arrête : une lettre seule entre frontière de
/// mot et point n'est pas une fin de phrase.
static RE_CASE_TC_NEXT_DATED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^\s*(?:,|;|\bet\b)\s*
          (?:du\s+|le\s+|en\s+date\s+du\s+)?\d{1,2}(?:er)?\s+[a-z]+\.?\s+\d{4}\b
          (?:[^.;:]|\b[a-z]\.){0,60}?\bn\s*[°oº]?\s*s?\s*:?\s*
          (c\s?\d{3,4}|\d{2}-\d{2}\.\d{3}|\d{2}-\d{4,5}|\d{3,5})\b",
    )
    .unwrap()
});
/// Maillon nu de l'énumération TC : « n° 3789, 3790, 3791 ».
static RE_CASE_TC_NEXT_BARE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^\s*(?:,|\bet\b)\s*
          (c\s?\d{3,4}|\d{2}-\d{2}\.\d{3}|\d{2}-\d{4,5}|\d{3,5})\b",
    )
    .unwrap()
});
/// Sonde ARRIÈRE TC : « la décision n° 3124 du Tribunal des Conflits ». La
/// queue qui rejoint l'ancre ne porte aucun chiffre, ce qui écarte le décret
/// visé (« n° 2015-233 du 27 février 2015 »), et le groupe 2 dit si la date se
/// tient de ce côté.
static RE_CASE_TC_BACK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)\bn\s*[°oº]?\s*s?\s*:?\s*
          (c\s?\d{3,4}|\d{2}-\d{2}\.\d{3}|\d{2}-\d{4,5}|\d{3,5})\b
          (\s*,?\s*(?:du\s+|le\s+|en\s+date\s+du\s+)?\d{1,2}(?:er)?\s+[a-z]+\.?\s+\d{4})?
          [^.;:0-9]{0,30}$",
    )
    .unwrap()
});
/// Requête CEDH : format NNNNN/YY à barre oblique (« req. n° 30010/10 »).
static RE_CASE_CEDH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*n\s*[°oº]\s*s?\s*(\d{1,5}\s*/\s*\d{2})\b").unwrap());
static RE_CASE_CEDH_WIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:req(?:uetes?)?\s*\.?\s*)?n\s*[°oº]\s*s?\s*(\d{1,5}\s*/\s*\d{2})\b").unwrap()
});
static RE_CASE_CEDH_NEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:,|et)\s*(?:n\s*[°oº]\s*s?\s*)?(\d{1,5}\s*/\s*\d{2})\b").unwrap()
});
/// N° de requête CEDH derrière « arrêt » : « son arrêt n° 29217/12,
/// Tarakhel » — 4-5 chiffres avant la barre, disjoints des RG à préfixe court
/// (1-3 chiffres) ; le « n° » reste exigé.
static RE_CASE_CEDH_ARRET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"n\s*[°oº]\s*s?\s*(\d{4,5}\s*/\s*\d{2})\b").unwrap());
/// Un « règlement/directive n° 2988/95 » cité dans la fenêtre d'un « arrêt »
/// est un acte UE, pas une requête CEDH.
static RE_CASE_EU_ACT_BEFORE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:reglements?|directives?)\s*(?:\((?:ce|cee|ue)\)\s*)?$").unwrap()
});
/// Affaire CJUE ancrée derrière « affaire(s) / aff. » : préfixe de rôle
/// C-/T-/F- optionnel (« aff. 6/64 » CJCE historique). Un slashnum nu à 4-5
/// chiffres avant la barre est une requête CEDH — routé par
/// [`case_cjue_aff`].
static RE_CASE_AFF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^s?\s*\(?\s*(?:jointes\s+)?(?:n\s*[°oº]\s*)?
          (?:([ctf])(?:\s*-\s*|\s+)?)?(\d{1,5}\s*/\s*\d{2,7})\b",
    )
    .unwrap()
});
/// Affaire CJUE en fenêtre : le préfixe de rôle est exigé. Graphies tolérées :
/// tiret absent collé (« C631/13 »), espace seul (« C 434/15 »), séparateur
/// tiret (« C 287-16 »), espace dans le numéro (« C-29/ 10 ») — normalisées
/// par [`cjue_key`]. Itéré sur toute la fenêtre.
static RE_CASE_CJUE_WIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([ctf])(?:\s*-\s*|\s+)?(\d{1,4}\s*[/-]\s*\d{2,4})\b").unwrap());
/// Plage / énumération sans re-préfixe : « C-338/11 à 347/11 »,
/// « C-131/13, 163/13 et 164/13 » — le membre nu hérite du préfixe du membre
/// précédent (« à » plié en « a »).
static RE_CASE_CJUE_RANGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:,|a\s|et\s)\s*(\d{1,4}\s*/\s*\d{2,4})\b").unwrap());
/// N° de jugement TA (7 chiffres nus) ou d'arrêt CAA (AAXX99999) derrière
/// « jugement/arrêt n° » — chaîne du fond administratif.
static RE_CASE_ADMIN_TA: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*n\s*[°oº]\s*s?\s*(\d{7})\b").unwrap());
static RE_CASE_ADMIN_TA_NEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:,|et)\s*(?:n\s*[°oº]\s*s?\s*)?(\d{7})\b").unwrap());
static RE_CASE_ADMIN_CAA: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*n\s*[°oº]\s*s?\s*(\d{2}[a-z]{2}\d{5})\b").unwrap());
static RE_CASE_ADMIN_CAA_NEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:,|et)\s*(?:n\s*[°oº]\s*s?\s*)?(\d{2}[a-z]{2}\d{5})\b").unwrap()
});
/// RG derrière l'ancre « RG / Rôle / répertoire général » : « RG n°
/// 21/04532 », « RG : 18/00064 », « sous le n° 11/00094 », « RG n° F
/// 12/06470 » (lettre de section).
static RE_CASE_RG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?x)^\s*[:.]?\s*(?:sous\s+le\s+)?(?:n(?:umero)?\s*[°oº]?\s*s?)?\s*:?\s*(?:[a-z]\s*)?
          ({ROLE})\b"
    ))
    .unwrap()
});
static RE_CASE_RG_NEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^\s*(?:,|et)\s*(?:(?:n\s*[°oº]\s*s?\s*)?(?:rg\s*:?\s*)?)\s*({ROLE_SLASHED})\b"
    ))
    .unwrap()
});
/// « enregistrée sous le no 20/00325 », « enrôlées sous les numéros 18/02640
/// et 18/02641 », « enregistré au répertoire général sous le n° 2008F175 ».
static RE_CASE_ENROL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?x)^(?:\(e\))?s?\s*(?:au\s+repertoire\s+general\s+)?
          sous\s+l(?:e|es)\s+n(?:umeros?|os?)?\s*[°oº]?\s*s?\s*[:.]?\s*(?:[a-z]\s+)?({ROLE})\b"
    ))
    .unwrap()
});
/// « CA [Localité 8], 14 nov. 2019, n°18/04366 » : sigle CA + ville/date puis
/// marqueur n° STRICT et slashnum.
static RE_CASE_CA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?x)^\s*(?:\[[^\]]{{1,30}}\]|(?:de\s+)?[a-z][a-z'\- ]{{1,25}})?\s*,?\s*
          (?:\d{{1,2}}(?:er)?\s+[a-z]+\.?\s+\d{{4}}\s*,?\s*)?n\s*[°oº]\s*s?\s*({ROLE_SLASHED})\b"
    ))
    .unwrap()
});
/// « DÉCISION DÉFÉRÉE : 21/00287 » — bandeau CA sans « RG », deux-points
/// exigés, numéro immédiat.
static RE_CASE_DEFEREE_NUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"^\s*(?:a\s+la\s+cour\s*)?:\s*({ROLE_SLASHED})\b")).unwrap()
});
/// « ARRÊT AU FOND DU 25 MARS 2011 N° 2011/183 » — ordinal d'arrêt CA
/// (année/n°), clé nue jamais résolue.
static RE_CASE_ARRET_ORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?x)^[^.;:()]{0,40}?n\s*[°oº]\s*s?\s*((?:19|20)\d{2}\s*/\s*\d{1,4})\b").unwrap()
});
/// « Vu la requête … enregistrée sous le N°RG » collé à l'ancre : le n° de la
/// procédure EN COURS (JLD rétention), pas une citation.
static RE_CASE_RG_OWN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"requetes?\b[^.;]{0,120}enregistr[a-z]*[^.;]{0,40}$").unwrap());
/// Forme RG judiciaire derrière « jugement/arrêt/ordonnance » : marqueur n°
/// STRICT — « Jugement (N° 14/00048) rendu … », « jugement rendu le 10/05/2016
/// (instance n° 14-08937) », « ordonnance d'injonction de payer
/// n° 2023IP00245 ».
static RE_CASE_ADMIN_RG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?x)^\s*(?:rendue?\s+le\s+[\d/.-]{{8,10}}\s*)?(?:d'injonction\s+de\s+payer\s+)?
          \(?\s*(?:instance\s+)?n\s*[°oº]\s*s?\s*(?:rg\s*:?\s*)?(?:[a-z]\s+)?
          ({ROLE_SLASHED}|\d{{4}}(?:\s?[a-z]\s?|[a-z]{{2}})\d{{3,6}}|\d{{2}}-\d{{5,6}})\b"
    ))
    .unwrap()
});
/// Forme juridictionnelle au voisinage d'un RG (amont : dernière occurrence ;
/// aval : première) : groupe 1 = région de ville, mappée au référentiel par
/// préfixe de mots. Les segments de formation CPH s'intercalent (« Conseil de
/// Prud'hommes - Formation paritaire d'EVRY »).
static RE_CASE_RG_JUR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)(?:cour\s+d'appel|tribunal\s+judiciaire|tribunal\s+de\s+grande\s+instance
          |tribunal\s+d'instance|juge\s+de\s+l'execution|tribunal\s+(?:mixte\s+)?de\s+commerce
          |conseil\s+de\s+prud'hommes|tribunal\s+administratif|cour\s+administrative\s+d'appel)
          (?:\s*-?\s*formation\s+(?:paritaire|de\s+departage)|\s*-?\s*section\s+[a-z'\- ]{1,30})*
          \s+(?:de\s+la\s+|de\s+|d'|du\s+)?([a-z][a-z'\- ]{1,40})",
    )
    .unwrap()
});
/// Nature après un connecteur d'anaphore (« du même CODE », « de cette LOI »).
static RE_SAME_NAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s+(?:meme\s+)?(code|livre|texte|loi|convention|reglement|decret|accord|charte|protocole|directive|ordonnance|arrete|traite|statut)\b",
    )
    .unwrap()
});
/// Nature AVANT un « précité(e) » (« du code précité », « de la loi
/// susmentionnée précitée ») — appliqué à la fenêtre qui PRÉCÈDE l'ancre.
static RE_SAME_PRECITE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:du|de\s+la)\s+(code|reglement|accord|texte|decret|loi|convention|directive)(?:\s+[a-z'\-]+)?\s+$",
    )
    .unwrap()
});
static RE_NUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^(?:[lrd]\s*\.?\s*'?\s*|a\s*\.\s*)?(\d+(?:er)?(?:\.\d+)*(?:\s*-\s*\d+)*(?:\s+(?:{ART_NUM_SUFFIX})\b)?(?:\s*-\s*\d+)*)",
    ))
    .unwrap()
});
/// Ordinal latin seul (suite d'un suffixe-lettre : « 1394 B bis »).
static RE_NUM_ORDINAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^ (?:{ART_NUM_SUFFIX})\b")).unwrap());
/// Alias TSV en forme datée NUE (nature + date, rien d'autre) : jamais un
/// pattern de l'automate.
static RE_BARE_DATED_ALIAS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:decret|loi|arrete|ordonnance|decision|deliberation|circulaire) du \d{1,2}(?:er)? (?:janvier|fevrier|mars|avril|mai|juin|juillet|aout|septembre|octobre|novembre|decembre) \d{4}$",
    )
    .unwrap()
});
/// Queue-objet d'une circulaire datée dans le TEXTE, après la mention :
/// auteur (« du ministre de l'intérieur ») enjambé, connecteur explicite
/// exigé, bornée à la ponctuation dure.
static RE_CIRC_DOC_TAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^\s*(?:du ministre [a-z' -]{0,60}? )?(?:relatives? |relatifs? |portant |concernant )(?:a |au |aux )?([^,;:.()!?«»"]{4,140})"#,
    )
    .unwrap()
});
/// Nature nue d'un instrument de FORUM derrière le connecteur génitif
/// (« , paragraphe 2, du règlement de procédure, toute partie… », « § 2 de
/// la Convention. ») — bornée à la phrase, nue seulement (ponctuation,
/// coordination ou guillemet derrière) ; le préfixe est re-validé par
/// [`genitive_gap`].
static RE_FORUM_GEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^[^.;:]{0,30}?(?:du|de\s+la)\s+(reglement de procedure|convention)\s*(?:[.,;:)»"]|et\s|ou\s|$)"#,
    )
    .unwrap()
});
/// Connecteur génitif directement après un numéral — l'article appartient à
/// l'instrument qui suit, reconnu ou pas (gate du génitif orphelin).
static RE_ORPHAN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^[\s,]*(?:et\s+)?(?:(?:alineas?|alienas?|al\.?|§|paragraphes?|points?)\s*\d*\s*,?\s*|\([^)]{1,60}\)\s*)*(?:(?:du|de\s+la|des|dudit|de\s+ladite)\s|de\s+l')",
    )
    .unwrap()
});
/// A possessive after the numeral, « l'article 12 de leur convention » : the parties' own
/// instrument, which a foreign court's antecedent never names.
static RE_POSSESSIVE_GEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^[\s,]*(?:(?:alineas?|al\.?|§|paragraphes?|points?)\s*\d*\s*,?\s*|\([^)]{1,60}\)\s*)*de\s+(?:leurs?|son|sa|ses)\s",
    )
    .unwrap()
});
/// A nature taken up after the numeral, « l'article 14, alinéas 3 et 4 du Traité susvisé » :
/// the last instrument of that nature the decision named.
static RE_NATURE_ANAPHORA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^[\s,]*(?:(?:alineas?|al\.?|§§?|paragraphes?|points?)\s*(?:\d+(?:er)?(?:\s*,\s*\d+|\s+et\s+\d+)*)?\s*,?\s*|\([^)]{1,60}\)\s*)*(?:du|de\s+la|de\s+l'\s*)(traite|acte\s+uniforme|reglement|code|loi|decret|ordonnance|convention|accord|protocole)(?:\s+ohada)?\s+(?:sus[\s-]?vise|precite|sus[\s-]?enonce|sus[\s-]?evoque|sus[\s-]?mentionne|sus[\s-]?indique|suscite)e?s?\b",
    )
    .unwrap()
});
/// Génitif DÉICTIQUE après un numéral (« 91 du présent Code », « 7 de la
/// présente loi ») : le porteur se nomme lui-même. Mêmes qualificatifs de
/// subdivision tolérés que [`RE_ORPHAN`].
static RE_SELF_GEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^[\s,]*(?:et\s+)?(?:(?:alineas?|al\.?|§|paragraphes?|points?)\s*\d*\s*,?\s*)*(?:du|de\s+la|au|a\s+la)\s+present(?:e)?\s+(?:code|loi|decret|ordonnance|arrete|reglement|texte|convention|charte|statut|constitution|accord|protocole|deliberation)\b",
    )
    .unwrap()
});
/// Désignateur de zone d'urbanisme entre le numéral et le connecteur génitif
/// (« 3 UA du POS », « 4 NC1 du PLU ») : le token capturé est examiné en
/// aval.
static RE_ZONE_STEP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s+([0-9]?[a-z]{1,2}[0-9]{0,2})\s+(?:(?:du|de\s+la|des)\s|de\s+l')").unwrap()
});
/// Qualificatif d'état du texte ouvrant sur un acte daté embarqué —
/// l'énumération d'articles l'enjambe (« dans leur rédaction antérieure à la
/// loi n° 2004-439 du 26 mai 2004 et 893, 894 du Code civil »). Seuls les
/// participes qui EXIGENT un acte objet (antérieure à, issue de, modifiée
/// par…) ouvrent l'enjambement.
static RE_REDACTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^dans\s+(?:sa|leur|ses|leurs)\s+redactions?\s+(?:anterieures?|posterieures?|issues?|resultante?s?|modifiees?)\s+(?:a|de|du|par)\s+(?:la\s+|l'\s*)?",
    )
    .unwrap()
});
/// Paragraphe(s) romain(s) de subdivision suivant un numéral d'article
/// (« L. 1142-1, I, du code… », « II et III, »). Enjambé par la marche
/// d'énumération avant le probe génitif.
static RE_ROMAN_PARA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:i{1,3}|iv|v|vi{1,3}|ix|x)(?:\s*(?:,|et)\s*(?:i{1,3}|iv|v|vi{1,3}|ix|x))*\s*,\s*",
    )
    .unwrap()
});
/// Ordinal(aux) arabe(s) de subdivision suivant un numéral d'article
/// (« L. 1142-1-1, 1°, du code… », « 78-2, 3° et 4°, du CPP ») : un numéro
/// suivi du signe degré n'est jamais un article — enjambé par la marche
/// d'énumération comme le paragraphe romain.
static RE_ARABIC_PARA: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{1,2}\s*[°º](?:\s*(?:,|et)\s*\d{1,2}\s*[°º])*\s*,?").unwrap());
/// Article préfixé nu (L./R./D. + point exigé) directement devant le génitif
/// d'une mention d'instrument, SANS mot « article » amont (bandeaux JLD).
/// Ancré à droite sur la fin de la tranche sondée (qui s'arrête au début de
/// la mention).
static RE_BARE_ART_GEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:^|[\s(;:,])([lrd]\s*\.\s*\d+(?:\s*-\s*\d+)*)\s*,?\s*(?:et\s+suivants?\s+)?(?:du|de\s+la|de\s+l')\s*$",
    )
    .unwrap()
});
/// Article NOMMÉ, sans numéral (« l'article préliminaire du CPP ») — token
/// d'article à part entière de la marche d'énumération.
static RE_NAMED_ART: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^preliminaire\b").unwrap());
/// Queue de titre d'acte daté NON capturée par la mention : « loi du
/// 10 juillet 1991 [relative à l'aide juridique] et L. 761-1 … » — la marche
/// d'énumération la traverse, borne courte.
static RE_TITLE_TAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s+(?:relati(?:f|ve)s?|portant|concernant|modifiant|fixant|instituant)\b")
        .unwrap()
});
/// « et » de reprise d'énumération dans une queue de titre.
static RE_ET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bet\s+").unwrap());
static RE_SEP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:\s*[a-z]\))?(?:\s*[-–](?:\s*\d{1,2}(?:er|eme|e)\s+alineas?\b)?)?(?:\s*(?:paragraphes?|paragr\.?|alineas?|§)\s*\d+(?:er)?)?(?:\s+(?:i{1,3}|iv|v|vi{1,3}|ix|x)\b)?(?:\s*(?:et|&)\s*suivant(?:e?s)?)?(?:\s*(?:anciens?|anciennes?|nouveaux?|nouvelles?|modifiee?s?|abrogee?s?))?(?:\s*,?\s*(?:et\s+|ou\s+)?\(\s*(?:anciennement\s+|anciens?\s+|ex[\s-]|devenus?\s+)?|\s*\)\s*,?\s*(?:et\s+|ou\s+)?|\s*,?\s*(?:et\s+|ou\s+)?(?:du\s+|de\s+l'\s*|des\s+|de\s+la\s+)(?:premier\s+|deuxieme\s+|dernier\s+)?(?:alinea\s+de\s+l'\s*)?articles?\s+|\s*,?\s*devenu\s+|\s*,?\s*(?:ou\s+)?anciennement\s+|\s*,?\s*\*\s*|\s*,?\s*(?:et\s+|ou\s+)?[-–]\s*|\s*,?\s*(?:et|ou|a|à|ainsi\s+que)\s+|\s*,?\s*)",
    )
    .unwrap()
});
/// Reprise du mot « article » derrière un connecteur de coordination, ancrée
/// sur le numéral qui suit. Dans un corps du référentiel dont le porteur
/// revendique la tête, elle borne l'héritage d'énumération : « l'article X ou
/// de l'article Y du code Z » ne partage pas son génitif. La reprise en
/// virgule (« , de l'article Y ») le partage toujours.
static RE_COORD_ART: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:et|ou|ni|ainsi\s+que)\s+(?:du\s+|des\s+|de\s+la\s+|de\s+l'\s*|l'\s*)(?:(?:premier|deuxieme|dernier)\s+alinea\s+de\s+l'\s*)?articles?\s*$",
    )
    .unwrap()
});

#[cfg(test)]
mod tests {
    use super::*;

    fn text(uid: &str, title: &str, title_key: &str, nature: &str, n_vigueur: i64) -> CatalogText {
        CatalogText {
            text_uid: uid.to_string(),
            title: title.to_string(),
            title_key: title_key.to_string(),
            nature: nature.to_string(),
            jurisdiction: Some("FR".to_string()),
            num_prefix_agnostic: false,
            n_vigueur,
            date_texte: None,
            nor: None,
        }
    }

    fn fixture() -> (Vec<CatalogText>, Vec<(String, String)>) {
        let texts = vec![
            text(
                "BOI-IR-BASE-10-10",
                "IR - Base d'imposition - Revenus imposables",
                "IR - Base d'imposition - Revenus imposables",
                "BOFIP",
                5,
            ),
            text(
                "cir_45678",
                "Circulaire du 23 juin 2021 relative aux modalités d'attribution des \
                 bourses d'enseignement supérieur",
                "Circulaire du 23 juin 2021",
                "CIRCULAIRE",
                0,
            ),
            text(
                "LEGI_CJA",
                "Code de justice administrative",
                "Code de justice administrative",
                "CODE",
                900,
            ),
            text("LEGI_CC", "Code civil", "Code civil", "CODE", 2800),
            text(
                "LEGI_CESEDA",
                "Code de l'entrée et du séjour des étrangers et du droit d'asile",
                "Code de l'entrée et du séjour des étrangers et du droit d'asile",
                "CODE",
                1200,
            ),
            text(
                "LEGI_CGI",
                "Code général des impôts",
                "Code général des impôts",
                "CODE",
                2000,
            ),
            text(
                "JORF_CEDH",
                "Convention européenne de sauvegarde des droits de l'homme et des libertés fondamentales",
                "Convention européenne de sauvegarde des droits de l'homme et des libertés fondamentales",
                "TRAITE",
                60,
            ),
            text(
                "LEGI_LPF",
                "Livre des procédures fiscales",
                "Livre des procédures fiscales",
                "CODE",
                800,
            ),
            text(
                "JORF_AJ",
                "Loi n° 91-647 du 10 juillet 1991 relative à l'aide juridique",
                "Loi du 10 juillet 1991 relative à l'aide juridique",
                "LOI",
                60,
            ),
            text("LEGI_TRAV", "Code du travail", "Code du travail", "CODE", 2500),
            text("LEGI_COM", "Code de commerce", "Code de commerce", "CODE", 2000),
            text(
                "JORF_CERT",
                "Arrêté du 27 décembre 2016 relatif aux conditions d'établissement et de \
                 transmission des certificats médicaux, rapports médicaux et avis mentionnés \
                 aux articles R. 313-22, R. 313-23 et R. 511-1 du code de l'entrée et du \
                 séjour des étrangers et du droit d'asile",
                "Arrêté du 27 décembre 2016 relatif aux conditions d'établissement et de \
                 transmission des certificats médicaux",
                "ARRETE",
                30,
            ),
            text("LEGI_RUR", "Code rural", "Code rural", "CODE", 1500),
            text(
                "LEGITEXT000006069569",
                "Code général des impôts, annexe II",
                "Code général des impôts, annexe II",
                "CODE",
                1600,
            ),
            text(
                "JORF_2010",
                "Arrêté du 12 février 2010 pris en application du deuxième alinéa du 1 de \
                 l'article 238-0 A du code général des impôts",
                "Arrêté du 12 février 2010 pris en application du deuxième alinéa du 1 de \
                 l'article 238-0 A du code général des impôts",
                "ARRETE",
                30,
            ),
            text(
                "INTL/CONVENTION-NATIONS-UNIES-CONTRATS-VENTE-1980",
                "Convention des Nations unies sur les contrats de vente internationale de marchandises",
                "Convention des Nations unies sur les contrats de vente internationale de marchandises",
                "CONVENTION",
                40,
            ),
        ];
        let mut texts = texts;
        let mut cir_libre = text(
            "cir_50001",
            "Modalités de gestion des aides personnelles au logement",
            "Modalités de gestion des aides personnelles au logement",
            "CIRCULAIRE",
            0,
        );
        cir_libre.date_texte = "2019-03-12".parse().ok();
        texts.push(cir_libre);
        let mut cir_nor = text(
            "cir_60001",
            "Orientations relatives à l'examen des demandes d'admission au séjour",
            "Orientations relatives à l'examen des demandes d'admission au séjour",
            "CIRCULAIRE",
            0,
        );
        cir_nor.nor = Some("INTK1229185C".to_string());
        texts.push(cir_nor);
        for (uid, title) in [
            (
                "cir_70001",
                "Conditions d'examen des demandes d'admission au séjour déposées par \
                 des ressortissants étrangers en situation irrégulière",
            ),
            (
                "cir_70002",
                "Examen professionnel d'accès au corps des secrétaires administratifs",
            ),
        ] {
            let mut c = text(uid, title, title, "CIRCULAIRE", 0);
            c.date_texte = "2012-11-28".parse().ok();
            texts.push(c);
        }
        let articles = vec![
            ("LEGI_CJA".to_string(), "l761-1".to_string()),
            ("LEGI_CJA".to_string(), "r771-5".to_string()),
            ("LEGI_CJA".to_string(), "r771-7".to_string()),
            ("LEGI_CC".to_string(), "1240".to_string()),
            ("LEGI_CESEDA".to_string(), "l742-3".to_string()),
            ("LEGI_CESEDA".to_string(), "l313-14".to_string()),
            ("LEGI_CESEDA".to_string(), "l425-9".to_string()),
            ("LEGI_CESEDA".to_string(), "l611-3".to_string()),
            ("LEGI_CESEDA".to_string(), "r313-22".to_string()),
            ("JORF_CEDH".to_string(), "2".to_string()),
            ("JORF_CEDH".to_string(), "5".to_string()),
            ("JORF_CEDH".to_string(), "7".to_string()),
            ("JORF_CEDH".to_string(), "14".to_string()),
            ("LEGI_CGI".to_string(), "97".to_string()),
            ("LEGI_CGI".to_string(), "98".to_string()),
            ("LEGI_CGI".to_string(), "100".to_string()),
            ("LEGI_LPF".to_string(), "l73".to_string()),
            ("LEGI_LPF".to_string(), "l16-a".to_string()),
            ("LEGI_CGI".to_string(), "164-b".to_string()),
            ("LEGI_CGI".to_string(), "1518-a-quinquies".to_string()),
            ("LEGI_COM".to_string(), "a444-32".to_string()),
            ("JORF_AJ".to_string(), "37".to_string()),
            ("LEGI_TRAV".to_string(), "l212-8".to_string()),
            ("LEGI_TRAV".to_string(), "l212-8-5".to_string()),
            ("LEGI_RUR".to_string(), "l212-8".to_string()),
            ("LEGI_RUR".to_string(), "1144".to_string()),
            ("LEGITEXT000006069569".to_string(), "202-a".to_string()),
            ("LEGITEXT000006069569".to_string(), "202-b".to_string()),
        ];
        (texts, articles)
    }

    fn engine() -> (CompiledVocab, LinkSnapshot) {
        let (texts, articles) = fixture();
        let snap = LinkSnapshot::build(texts.clone(), articles);
        let vocab = CompiledVocab::build(&texts, &snap);
        (vocab, snap)
    }

    fn cites(text: &str) -> Vec<CompiledCitation> {
        let (vocab, snap) = engine();
        extract_citations(text, &vocab, &snap, None)
    }

    fn eu_engine() -> (CompiledVocab, LinkSnapshot) {
        let texts = vec![
            text(
                "EU/STATUT-CJUE",
                "Protocole (n° 3) sur le statut de la Cour de justice de l'Union européenne",
                "Protocole (n° 3) sur le statut de la Cour de justice de l'Union européenne",
                "PROTOCOLE",
                64,
            ),
            text(
                "EU/RPROC/TRIBUNAL",
                "Règlement de procédure du Tribunal",
                "Règlement de procédure du Tribunal",
                "REGLEMENT",
                0,
            ),
            text(
                "EU/RPROC/CJUE",
                "Règlement de procédure de la Cour de justice",
                "Règlement de procédure de la Cour de justice",
                "REGLEMENT",
                0,
            ),
        ];
        let articles = vec![
            ("EU/STATUT-CJUE".to_string(), "46".to_string()),
            ("EU/STATUT-CJUE".to_string(), "53".to_string()),
        ];
        let snap = LinkSnapshot::build(texts.clone(), articles);
        let vocab = CompiledVocab::build(&texts, &snap);
        (vocab, snap)
    }

    #[test]
    fn statut_cjue_short_form_and_same_statut_anaphora() {
        let (vocab, snap) = eu_engine();
        let out = extract_citations(
            "en vertu de l'article 46 du statut de la Cour, applicable à la \
             procédure devant le Tribunal en vertu de l'article 53 du même statut",
            &vocab,
            &snap,
            None,
        );
        let uids: Vec<_> = out
            .iter()
            .map(|c| {
                (
                    c.target.ref_text_uid.as_deref(),
                    c.target.ref_num_key.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            uids,
            vec![
                (Some("EU/STATUT-CJUE"), Some("46")),
                (Some("EU/STATUT-CJUE"), Some("53")),
            ]
        );
    }

    #[test]
    fn rproc_short_forms_linked_bare_abstains() {
        let (vocab, snap) = eu_engine();
        let out = extract_citations(
            "prévues par l'article 64 du règlement de procédure du Tribunal",
            &vocab,
            &snap,
            None,
        );
        assert_eq!(
            out[0].target.ref_text_uid.as_deref(),
            Some("EU/RPROC/TRIBUNAL")
        );
        let out = extract_citations(
            "en application de l'article 38, paragraphe 1, du règlement de \
             procédure de la Cour",
            &vocab,
            &snap,
            None,
        );
        assert_eq!(out[0].target.ref_text_uid.as_deref(), Some("EU/RPROC/CJUE"));
        let out = extract_citations(
            "contraires aux prescriptions de l'article 38 du règlement de procédure",
            &vocab,
            &snap,
            None,
        );
        assert_eq!(out[0].target.ref_text_uid, None);
    }

    #[test]
    fn statut_cpi_never_mislinked_to_cjue() {
        let (vocab, snap) = eu_engine();
        let out = extract_citations(
            "les crimes visés à l'article 8 du statut de la Cour pénale internationale",
            &vocab,
            &snap,
            None,
        );
        assert!(out
            .iter()
            .all(|c| c.target.ref_text_uid.as_deref() != Some("EU/STATUT-CJUE")));
    }

    #[test]
    fn boi_reference_linked() {
        let out = cites(
            "l'administration s'est fondée sur les énonciations du \
             BOI-IR-BASE-10-10 publié le 12 septembre 2012",
        );
        let c = out
            .iter()
            .find(|c| c.text_key == "BOI-IR-BASE-10-10")
            .expect("mention BOI");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("BOI-IR-BASE-10-10"));
    }

    #[test]
    fn boi_lowercase_ignored() {
        assert!(cites("le boi-ir-base-10-10 du dossier").is_empty());
    }

    #[test]
    fn circulaire_dated_linked() {
        let out = cites(
            "en application de la circulaire du 23 juin 2021 relative aux \
             modalités d'attribution des bourses d'enseignement supérieur",
        );
        let c = out.first().expect("mention circulaire");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("cir_45678"));
    }

    #[test]
    fn circulaire_dated_by_column_linked() {
        let out = cites(
            "conformément à la circulaire du 12 mars 2019 relative aux aides \
             personnelles au logement",
        );
        let c = out.first().expect("mention circulaire");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("cir_50001"));
    }

    #[test]
    fn boi_dated_version_linked() {
        let out = cites("les prévisions du BOI-IR-BASE-10-10-20140311 s'appliquent");
        let c = out.first().expect("mention BOI");
        assert_eq!(c.text_key, "BOI-IR-BASE-10-10");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("BOI-IR-BASE-10-10"));
    }

    #[test]
    fn circulaire_date_homonyme_queue_objet_et_adoption() {
        let out = cites(
            "le refus méconnaît la circulaire du 28 novembre 2012 relative aux \
             conditions d'examen des demandes d'admission au séjour ; \
             en outre la circulaire du 28 novembre 2012 prévoit un récépissé.",
        );
        let linked: Vec<_> = out
            .iter()
            .filter(|c| c.target.ref_text_uid.as_deref() == Some("cir_70001"))
            .collect();
        assert_eq!(linked.len(), 2, "queue-objet + adoption de la forme nue");

        let out = cites("en vertu de la circulaire du 28 novembre 2012 portant examen des cas");
        let c = out.first().expect("mention circulaire");
        assert_eq!(c.target.ref_text_uid, None);
    }

    #[test]
    fn circulaire_alias_cure_passe_le_gate_acte_individuel() {
        let mut texts = Vec::new();
        for uid in ["cir_44486", "cir_36120"] {
            let mut c = text(
                uid,
                "Conditions d'examen des demandes d'admission au séjour déposées par \
                 des ressortissants étrangers en situation irrégulière",
                "Conditions d'examen des demandes d'admission au séjour déposées par \
                 des ressortissants étrangers en situation irrégulière",
                "CIRCULAIRE",
                0,
            );
            c.date_texte = "2012-11-28".parse().ok();
            texts.push(c);
        }
        let snap = LinkSnapshot::build(texts.clone(), vec![]);
        let vocab = CompiledVocab::build(&texts, &snap);
        let out = extract_citations(
            "il méconnaît les dispositions de la circulaire du 28 novembre 2012 ;",
            &vocab,
            &snap,
            None,
        );
        let c = out.first().expect("mention nue émise malgré la prose");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("cir_44486"));

        let out = extract_citations(
            "par référence aux lignes directrices de la circulaire ministérielle \
             du 28 novembre 2012, dont le point 2.2 prévoit un récépissé.",
            &vocab,
            &snap,
            None,
        );
        let c = out.first().expect("graphie qualifiée émise");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("cir_44486"));

        let out = cites("il méconnaît la circulaire du 3 février 2010 en cause.");
        assert!(out.is_empty(), "forme nue non curée toujours gatée");
    }

    #[test]
    fn a_circulaire_named_by_its_author_before_its_date_links_like_the_bare_form() {
        let mut texts = Vec::new();
        for uid in ["cir_44486", "cir_36120"] {
            let mut c = text(
                uid,
                "Conditions d'examen des demandes d'admission au séjour déposées par \
                 des ressortissants étrangers en situation irrégulière",
                "Conditions d'examen des demandes d'admission au séjour déposées par \
                 des ressortissants étrangers en situation irrégulière",
                "CIRCULAIRE",
                0,
            );
            c.date_texte = "2012-11-28".parse().ok();
            texts.push(c);
        }
        let snap = LinkSnapshot::build(texts.clone(), vec![]);
        let vocab = CompiledVocab::build(&texts, &snap);
        for (text, surface) in [
            (
                "Vu :\n- la circulaire du ministre de l'intérieur du 28 novembre 2012 relative \
                 aux conditions d'examen des demandes d'admission au séjour ;",
                "circulaire du ministre de l'intérieur du 28 novembre 2012",
            ),
            (
                "le préfet a méconnu la circulaire du ministre de l'économie, des finances et \
                 de l'industrie du 28 novembre 2012.",
                "circulaire du ministre de l'économie, des finances et de l'industrie du 28 \
                 novembre 2012",
            ),
        ] {
            let out = extract_citations(text, &vocab, &snap, None);
            let c = out.first().expect("author form emitted");
            assert_eq!(
                text.chars()
                    .skip(c.char_start)
                    .take(c.char_end - c.char_start)
                    .collect::<String>(),
                surface
            );
            assert_eq!(
                c.target.ref_text_uid.as_deref(),
                Some("cir_44486"),
                "{text}"
            );
        }
    }

    #[test]
    fn circulaire_qualificatif_non_distinctif_resout_comme_la_forme_nue() {
        let out = cites(
            "la circulaire ministérielle du 28 novembre 2012 relative aux \
             conditions d'examen des demandes d'admission au séjour ; \
             le préfet l'a méconnue.",
        );
        let c = out.first().expect("mention qualifiée émise");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("cir_70001"));

        let out = cites(
            "le refus méconnaît la circulaire du 28 novembre 2012 relative aux \
             conditions d'examen des demandes d'admission au séjour ; la \
             circulaire interministérielle du 28 novembre 2012 prévoit un récépissé.",
        );
        let linked = out
            .iter()
            .filter(|c| c.target.ref_text_uid.as_deref() == Some("cir_70001"))
            .count();
        assert_eq!(linked, 2, "graphie qualifiée adoptée par la forme nue");
    }

    #[test]
    fn circulaire_dated_unknown_unlinked() {
        let out = cites("la circulaire du 12 mars 1999 relative au recouvrement des créances");
        let c = out.first().expect("mention circulaire");
        assert_eq!(c.target.ref_text_uid, None);
    }

    #[test]
    fn nor_before_date_linked() {
        let out = cites("les termes de la circulaire NOR INTK1229185C du 28 novembre 2012");
        let c = out.first().expect("mention circulaire");
        assert_eq!(c.text_key, "Circulaire du 28 novembre 2012");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("cir_60001"));
    }

    #[test]
    fn nor_alone_linked() {
        let out = cites("la circulaire NOR : INTK1229185C du ministre de l'intérieur est invoquée");
        let c = out.first().expect("mention circulaire");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("cir_60001"));
    }

    #[test]
    fn nor_glued_linked() {
        let out =
            cites("elle méconnaît les termes de la circulaire NORINTK1229185C du 16 octobre 2012");
        let c = out.first().expect("mention circulaire");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("cir_60001"));
    }

    #[test]
    fn nor_after_date_linked() {
        let out = cites("l'arrêté du 4 mai 2017 (NOR : INTK1229185C) prévoit que");
        let c = out.first().expect("mention arrêté");
        assert_eq!(c.text_key, "Arrêté du 4 mai 2017");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("cir_60001"));
    }

    #[test]
    fn nor_unknown_unlinked() {
        let out = cites("la circulaire NOR ABCD1234567X du 3 février 1998 sur les gens du voyage");
        let c = out.first().expect("mention circulaire");
        assert_eq!(c.target.ref_text_uid, None);
    }

    fn cases(text: &str) -> Vec<CompiledCase> {
        let (vocab, snap) = engine();
        let chrono = crate::chrono::ChronoSnapshot::new(vec![
            (
                "ca_paris".to_string(),
                "CA".to_string(),
                "Paris".to_string(),
            ),
            ("tj75056".to_string(), "TJ".to_string(), "Paris".to_string()),
            (
                "ca_aix_provence".to_string(),
                "CA".to_string(),
                "Aix-en-Provence".to_string(),
            ),
            (
                "ta_nantes".to_string(),
                "TA".to_string(),
                "Nantes".to_string(),
            ),
            (
                "caa_nantes".to_string(),
                "CAA".to_string(),
                "Nantes".to_string(),
            ),
            (
                "tcom7501".to_string(),
                "TCOM".to_string(),
                "Paris".to_string(),
            ),
        ]);
        doc_extract(text, &vocab, &snap, &chrono, None, None).cases
    }

    /// (surface, `court|docket`) of the emitted spans. A citation has no date.
    fn case_surfaces(text: &str) -> Vec<(String, String)> {
        cases(text)
            .into_iter()
            .map(|c| {
                (
                    text.chars()
                        .skip(c.char_start)
                        .take(c.char_end - c.char_start)
                        .collect(),
                    addr_str(&c.address),
                )
            })
            .collect()
    }

    fn addr_str(a: &CaseAddress) -> String {
        let c = |o: &Option<String>| o.clone().unwrap_or_default();
        format!("{}|{}", c(&a.court), c(&a.docket))
    }

    fn by_article<'a>(out: &'a [CompiledCitation], art: &str) -> &'a CompiledCitation {
        out.iter()
            .find(|c| c.article.as_deref() == Some(art))
            .unwrap_or_else(|| panic!("article {art} absent"))
    }

    #[test]
    fn enumeration_bridges_redaction_qualifier() {
        let out = cites(
            "la Cour d'appel a violé les articles 230, 232, dans leur rédaction \
             antérieure à la loi n° 2004-439 du 26 mai 2004 et 893, 894 du Code civil ;",
        );
        for art in ["230", "232", "893", "894"] {
            assert_eq!(
                by_article(&out, art).target.ref_text_uid.as_deref(),
                Some("LEGI_CC"),
                "article {art}"
            );
        }
    }

    #[test]
    fn attaches_following_genitive_instrument() {
        let out = cites(
            "En application de l'article L. 761-1 du code de justice administrative, \
             la somme est mise à la charge de l'État.",
        );
        let c = by_article(&out, "L. 761-1");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CJA"));
        assert_eq!(c.target.ref_num_key.as_deref(), Some("l761-1"));
    }

    #[test]
    fn same_code_anaphora_resolves_to_last_code() {
        let out = cites(
            "Aux termes de l'article L. 761-1 du code de justice administrative : \
             \" (...) \". En vertu de l'article R. 771-5 du même code : \" Sauf s'il \
             apparaît de façon certaine (...) \".",
        );
        let c = by_article(&out, "R. 771-5");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CJA"));
    }

    #[test]
    fn same_code_anaphora_skips_instrument_quoted_in_frame() {
        let out = cites(
            "Aux termes de l'article R. 771-12 du code de justice administrative : \
             \" Lorsque, en application du dernier alinéa de l'article 23-2 de \
             l'ordonnance n° 58-1067 du 7 novembre 1958 portant loi organique sur \
             le Conseil constitutionnel, l'une des parties entend contester le \
             refus de transmission () \". En vertu de l'article R. 771-5 du même \
             code : \" Sauf s'il apparaît de façon certaine () \". L'article \
             R. 771-7 de ce code dispose que : \" () les présidents peuvent \
             statuer. \".",
        );
        let c = by_article(&out, "R. 771-5");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CJA"));
        assert_eq!(c.target.ref_num_key.as_deref(), Some("r771-5"));
        let c = by_article(&out, "R. 771-7");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CJA"));
    }

    #[test]
    fn same_code_after_ordinal_qualifier_resolves() {
        let out = cites(
            "Il méconnaît l'article L. 425-9 du code de l'entrée et du séjour des \
             étrangers et du droit d'asile et le 9° de l'article L. 611-3 du même \
             code dès lors qu'elle établit qu'elle ne pourra en bénéficier.",
        );
        let c = by_article(&out, "L. 611-3");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CESEDA"));
    }

    #[test]
    fn bare_convention_anaphora_resolves_enumeration() {
        let out = cites(
            "Saisir la Cour sur le fondement de l'article 1er du protocole \
             additionnel n° 16 à la convention européenne de sauvegarde des \
             droits de l'homme et des libertés fondamentales, sur \
             l'interprétation de l'article 1er du protocole additionnel n° 12 \
             et des articles 2, 5, 7 et 14 de la convention, d'une demande \
             d'avis relative aux décrets contestés.",
        );
        assert!(
            !out.iter()
                .any(|c| c.article.is_none() && c.text_key.starts_with("Convention")),
            "la mention imbriquée n'émet pas de span propre"
        );
        for art in ["2", "5", "7", "14"] {
            let c = by_article(&out, art);
            assert_eq!(
                c.target.ref_text_uid.as_deref(),
                Some("JORF_CEDH"),
                "article {art}"
            );
        }
    }

    #[test]
    fn annexe_cgi_alias_attaches_and_links() {
        let out = cites(
            "le Premier ministre a refus\u{e9} de retirer et d'abroger les articles \
             202 A et 202 B de l'annexe II au code g\u{e9}n\u{e9}ral des imp\u{f4}ts, en tant \
             qu'ils m\u{e9}connaissent la directive. L'article 202 A de l'annexe II \
             au code g\u{e9}n\u{e9}ral des imp\u{f4}ts dispose que.",
        );
        let annexe: Vec<_> = out
            .iter()
            .filter(|c| c.text_key == "annexe ii au code general des impots")
            .collect();
        assert_eq!(
            annexe.len(),
            3,
            "202 A + 202 B \u{e9}num\u{e9}r\u{e9}s, puis 202 A seul"
        );
        for c in &annexe {
            assert_eq!(
                c.target.ref_text_uid.as_deref(),
                Some("LEGITEXT000006069569"),
                "cible = l'annexe II, jamais le CGI de base"
            );
        }
    }

    #[test]
    fn genitive_instrument_inside_quote_attaches() {
        let out = cites(
            "7. Aux termes de l'article L. 73 du livre des procédures fiscales : \
             \" Peuvent être évalués d'office : () 2° Le bénéfice imposable des \
             contribuables qui perçoivent des revenus non commerciaux ou des \
             revenus assimilés lorsque la déclaration annuelle prévue à l'article \
             97 du code général des impôts n'a pas été déposée dans le délai \
             légal ; () \". Aux termes de l'article 97 du code général des \
             impôts : \" Les contribuables soumis obligatoirement ou sur option \
             au régime de la déclaration contrôlée sont tenus de souscrire \
             chaque année une déclaration. \"",
        );
        let l73 = by_article(&out, "L. 73");
        assert_eq!(l73.target.ref_text_uid.as_deref(), Some("LEGI_LPF"));
        let arts: Vec<_> = out
            .iter()
            .filter(|c| c.article.as_deref() == Some("97"))
            .collect();
        assert_eq!(arts.len(), 2, "deux citations de l'article 97");
        for c in arts {
            assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CGI"));
        }
    }

    #[test]
    fn de_ce_code_anaphora_resolves() {
        let out = cites(
            "Vu le code de justice administrative. L'article R. 771-7 de ce code \
             dispose que les présidents peuvent statuer.",
        );
        let c = by_article(&out, "R. 771-7");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CJA"));
    }

    #[test]
    fn treaty_prefix_anaphora_adopts_dated_form() {
        let out = cites(
            "Vu la convention de Vienne du 11 avril 1980 sur les contrats de vente \
             internationale de marchandises. Le délai de deux ans de l'article 39 \
             de la Convention de Vienne est un délai de dénonciation.",
        );
        let c = by_article(&out, "39");
        assert_eq!(
            c.target.ref_text_uid.as_deref(),
            Some("INTL/CONVENTION-NATIONS-UNIES-CONTRATS-VENTE-1980")
        );
    }

    #[test]
    fn dead_treaty_prefix_emits_nothing() {
        let out =
            cites("Le choix est régi par l'article 3 de la Convention de Rome selon l'appelant.");
        let c = by_article(&out, "3");
        assert_eq!(c.target.ref_text_uid, None);
        assert!(!out.iter().any(|c| c.article.is_none()));
    }

    #[test]
    fn quoted_article_belongs_to_quoted_text() {
        let out = cites(
            "Aux termes de l'article L. 742-3 du code de l'entrée et du séjour des \
             étrangers et du droit d'asile : \" Par dérogation à l'article L. 313-14, \
             l'étranger peut être transféré vers l'État responsable de l'examen. \"",
        );
        let c = by_article(&out, "L. 313-14");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CESEDA"));
        assert_eq!(c.target.ref_num_key.as_deref(), Some("l313-14"));
    }

    #[test]
    fn quoted_article_after_bare_frame_gets_folded_num_key() {
        let filler = "considérant ce qui suit. ".repeat(70);
        let text = format!(
            "Vu le code de l'entrée et du séjour des étrangers et du droit d'asile. \
             {filler}Le préfet a méconnu l'article L. 425-9, qui dispose : \" Par \
             dérogation à l'article L. 611-3, l'étranger peut se maintenir. \"",
        );
        let out = cites(&text);
        let frame = by_article(&out, "L. 425-9");
        assert_eq!(frame.target.ref_text_uid.as_deref(), Some("LEGI_CESEDA"));
        assert!(frame.text_key.is_empty());
        let c = by_article(&out, "L. 611-3");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CESEDA"));
        assert_eq!(c.target.ref_num_key.as_deref(), Some("l611-3"));
    }

    #[test]
    fn non_genitive_following_instrument_is_not_attached() {
        let out = cites(
            "Aux termes du code de l'entrée et du séjour des étrangers et du droit \
             d'asile : \" L'étranger qui fait l'objet d'une décision de transfert en \
             application de l'article L. 742-3 ou d'une requête aux fins de prise en \
             charge peut la contester. \"",
        );
        let c = by_article(&out, "L. 742-3");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CESEDA"));
    }

    #[test]
    fn enumeration_traverses_instruments() {
        let out = cites(
            "La cour a violé les articles 1240 du code civil et L. 761-1 du code de \
             justice administrative.",
        );
        assert_eq!(
            by_article(&out, "1240").target.ref_text_uid.as_deref(),
            Some("LEGI_CC")
        );
        assert_eq!(
            by_article(&out, "L. 761-1").target.ref_text_uid.as_deref(),
            Some("LEGI_CJA")
        );
    }

    #[test]
    fn dispositif_self_reference_is_skipped() {
        let out = cites(
            "D É C I D E : Article 1er : La requête est rejetée. Article 2 : \
             Le jugement sera notifié.",
        );
        assert!(out.iter().all(|c| c.article.is_none()));
        let out = cites(
            "DÉCIDE : Article 1er. - La requête est rejetée. Article 2 - Les dépens \
             sont mis à la charge de l'Etat.",
        );
        assert!(out.iter().all(|c| c.article.is_none()));
    }

    #[test]
    fn generated_surface_links_numbered_act() {
        let out = cites("Sur le fondement de l'article 37 de la loi n° 91-647 du 10 juillet 1991.");
        let c = by_article(&out, "37");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("JORF_AJ"));
        assert_eq!(c.target.ref_num_key.as_deref(), Some("37"));
    }

    #[test]
    fn arabic_ordinal_subdivision_is_not_an_article() {
        let out = cites("la cour a violé l'article L. 1142-1-1, 1°, du code civil ;");
        assert_eq!(out.iter().filter(|c| c.article.is_some()).count(), 1);
        assert_eq!(
            by_article(&out, "L. 1142-1-1")
                .target
                .ref_text_uid
                .as_deref(),
            Some("LEGI_CC")
        );
        let out = cites(
            "au regard des articles L. 1142-1-1, 1°, et L. 1142-17 du code civil, \
             et de l'article 78-2, 3° et 4° du code civil.",
        );
        let nums: Vec<&str> = out.iter().filter_map(|c| c.article.as_deref()).collect();
        assert_eq!(nums, vec!["L. 1142-1-1", "L. 1142-17", "78-2"]);
    }

    #[test]
    fn enumeration_inherits_trailing_genitive() {
        let out = cites(
            "en application des articles R. 771-5 et R. 771-7 du code de justice \
             administrative, la requête est rejetée.",
        );
        assert_eq!(
            by_article(&out, "R. 771-5").target.ref_text_uid.as_deref(),
            Some("LEGI_CJA")
        );
        assert_eq!(
            by_article(&out, "R. 771-7").target.ref_text_uid.as_deref(),
            Some("LEGI_CJA")
        );
    }

    #[test]
    fn coordinated_article_repeat_does_not_share_genitive() {
        let texts = vec![
            text(
                "GN_CPP",
                "Code de procédure pénale",
                "Code de procédure pénale",
                "CODE",
                1200,
            ),
            text("GN_CP", "Code pénal", "Code pénal", "CODE", 1000),
        ];
        let articles: Vec<(String, String)> =
            [("GN_CPP", "1149"), ("GN_CPP", "1152"), ("GN_CP", "159")]
                .iter()
                .map(|(u, a)| (u.to_string(), a.to_string()))
                .collect();
        let snap = LinkSnapshot::build(texts.clone(), articles);
        let vocab = CompiledVocab::build(&texts, &snap);
        let chrono = crate::chrono::ChronoSnapshot::new(vec![]);
        let body = "sera condamné par application des dispositions de l'article 1149 \
                    ou de l'article 159 du Code pénal.";
        let out = doc_extract(body, &vocab, &snap, &chrono, None, Some("GN_CPP")).citations;
        let c = by_article(&out, "1149");
        assert!(c.bare, "1149 : nu, le porteur le revendique en aval");
        assert_eq!(c.target.ref_text_uid, None);
        assert_eq!(
            by_article(&out, "159").target.ref_text_uid.as_deref(),
            Some("GN_CP")
        );
    }

    #[test]
    fn coordinated_article_repeat_shares_genitive_without_owner_claim() {
        let out = cites(
            "sur le fondement du 11° de l'article L. 313-11 et de l'article \
             L. 313-14 du code de l'entrée et du séjour des étrangers et du \
             droit d'asile.",
        );
        for art in ["L. 313-11", "L. 313-14"] {
            assert_eq!(
                by_article(&out, art).target.ref_text_uid.as_deref(),
                Some("LEGI_CESEDA"),
                "article {art}"
            );
        }
    }

    #[test]
    fn comma_article_repeat_still_inherits_genitive() {
        let out = cites(
            "les jugements rendus en application des articles L. 622-8, du \
             premier alinéa de l'article L. 642-20-1 du Code de commerce.",
        );
        for art in ["L. 622-8", "L. 642-20-1"] {
            assert_eq!(
                by_article(&out, art).target.ref_text_uid.as_deref(),
                Some("LEGI_COM"),
                "article {art}"
            );
        }
    }

    #[test]
    fn newline_inside_mention_still_matches() {
        let out = cites("Vu l'article 1240 du code\ncivil, le moyen est fondé.");
        let c = by_article(&out, "1240");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CC"));
    }

    #[test]
    fn alinea_in_genitive_gap_is_transparent() {
        let out = cites("au titre de l'article 37 alinéa 2 de la loi du 10 juillet 1991 précitée.");
        let c = by_article(&out, "37");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("JORF_AJ"));
    }

    #[test]
    fn nature_anaphora_picks_matching_antecedent() {
        let out = cites(
            "Vu la loi du 10 juillet 1991 relative à l'aide juridique. Vu le code \
             de justice administrative. L'article 37 de cette loi dispose que.",
        );
        let c = by_article(&out, "37");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("JORF_AJ"));
    }

    #[test]
    fn devenu_chain_attaches_both_numerals() {
        let out = cites("a violé l'article 1382, devenu 1240, du code civil ;");
        assert_eq!(
            by_article(&out, "1240").target.ref_text_uid.as_deref(),
            Some("LEGI_CC")
        );
        assert_eq!(
            by_article(&out, "1382").target.ref_text_uid.as_deref(),
            Some("LEGI_CC")
        );
    }

    #[test]
    fn et_suivants_gap_is_genitive() {
        let out = cites("sur le fondement des articles 1240 et suivants du code civil.");
        assert_eq!(
            by_article(&out, "1240").target.ref_text_uid.as_deref(),
            Some("LEGI_CC")
        );
    }

    #[test]
    fn et_suivants_extends_span() {
        let text = "sur le fondement des articles 1240 et suivants du code civil.";
        let out = cites(text);
        let c = by_article(&out, "1240");
        assert!(c.suivants);
        assert_eq!(
            text.chars()
                .skip(c.char_start)
                .take(c.char_end - c.char_start)
                .collect::<String>(),
            "1240 et suivants"
        );

        let out = cites("vu les articles 1240 et suivantes du code civil.");
        let c = by_article(&out, "1240");
        assert!(c.suivants);
        assert_eq!(
            c.char_end - c.char_start,
            "1240 et suivantes".chars().count()
        );

        let out = cites("vu les articles 1240 et s. du code civil.");
        let c = by_article(&out, "1240");
        assert!(c.suivants);
        assert_eq!(c.char_end - c.char_start, "1240 et s.".chars().count());

        let text = "les articles 1240 et 1241 et suivants du code civil.";
        let out = cites(text);
        assert!(!by_article(&out, "1240").suivants);
        assert!(by_article(&out, "1241").suivants);
    }

    #[test]
    fn bare_article_flag() {
        let out = cites("acquise en application de l'article 21-2, sans autre condition.");
        let c = by_article(&out, "21-2");
        assert!(c.bare);
        assert_eq!(c.target.ref_text_uid, None);

        let out = cites("sur le fondement de l'article 82 du code de la famille congolais.");
        assert!(!by_article(&out, "82").bare);

        let out = cites("l'article 1240 du code civil.");
        assert!(!by_article(&out, "1240").bare);
    }

    #[test]
    fn bare_numeral_stays_with_owner_text() {
        let texts = vec![
            text(
                "LEGI_CPC",
                "Code de procédure civile",
                "Code de procédure civile",
                "CODE",
                1500,
            ),
            text("LEGI_CIV", "Code civil", "Code civil", "CODE", 2500),
        ];
        let articles: Vec<(String, String)> = [
            ("LEGI_CPC", "1049"),
            ("LEGI_CPC", "1055"),
            ("LEGI_CPC", "1056-1"),
            ("LEGI_CIV", "1049"),
            ("LEGI_CIV", "1055"),
            ("LEGI_CIV", "57"),
        ]
        .iter()
        .map(|(u, a)| (u.to_string(), a.to_string()))
        .collect();
        let snap = LinkSnapshot::build(texts.clone(), articles);
        let vocab = CompiledVocab::build(&texts, &snap);
        let chrono = crate::chrono::ChronoSnapshot::new(vec![]);
        let body = "L'action aux fins de déclaration judiciaire de naissance est régie par \
                    les dispositions des articles 1049 à 1055. Le dispositif de la décision, \
                    contenant les énonciations prévues à l'article 57 du code civil, est \
                    transcrit sur les registres.";

        let out = doc_extract(body, &vocab, &snap, &chrono, None, Some("LEGI_CPC")).citations;
        for k in ["1049", "1055"] {
            let c = by_article(&out, k);
            assert!(c.bare, "{k} : nu, le porteur le revendique en aval");
            assert_eq!(
                c.target.ref_text_uid, None,
                "{k} : pas de cible de document"
            );
        }
        assert_eq!(
            by_article(&out, "57").target.ref_text_uid.as_deref(),
            Some("LEGI_CIV"),
            "le code nommé emporte son propre renvoi"
        );

        let out = doc_extract(body, &vocab, &snap, &chrono, None, None).citations;
        assert_eq!(
            by_article(&out, "1049").target.ref_text_uid.as_deref(),
            Some("LEGI_CIV")
        );
    }

    #[test]
    fn deictic_genitive_names_the_owner_text() {
        let texts = vec![
            text(
                "SN_FAM",
                "Code de la famille",
                "Code de la famille",
                "CODE",
                900,
            ),
            text("LEGI_CIV", "Code civil", "Code civil", "CODE", 2500),
        ];
        let articles: Vec<(String, String)> = [
            ("SN_FAM", "33"),
            ("SN_FAM", "91"),
            ("SN_FAM", "86"),
            ("LEGI_CIV", "33"),
            ("LEGI_CIV", "91"),
        ]
        .iter()
        .map(|(u, a)| (u.to_string(), a.to_string()))
        .collect();
        let snap = LinkSnapshot::build(texts.clone(), articles);
        let vocab = CompiledVocab::build(&texts, &snap);
        let chrono = crate::chrono::ChronoSnapshot::new(vec![]);
        let body = "applique les sanctions prévues par les articles 33 et 91 du présent Code.";

        let out = doc_extract(body, &vocab, &snap, &chrono, None, Some("SN_FAM")).citations;
        assert!(by_article(&out, "33").bare, "33 : nu, revendiqué en aval");
        assert_eq!(
            by_article(&out, "91").target.ref_text_uid.as_deref(),
            Some("SN_FAM"),
            "91 touche le déictique : il désigne le porteur"
        );

        let out = doc_extract(body, &vocab, &snap, &chrono, None, None).citations;
        assert_eq!(by_article(&out, "91").target.ref_text_uid, None);
    }

    #[test]
    fn named_unknown_code_is_not_bare_anaphora() {
        let out = cites(
            "Vu le code civil ; considérant qu'aux termes de l'article 47 du code civil tout acte ; \
             le ministre s'est fondé sur l'article 82 du code de la famille congolais sans toutefois établir.",
        );
        let c = by_article(&out, "82");
        assert_eq!(c.target.ref_text_uid, None);

        let out = cites(
            "Aux termes de l'article 47 du code civil tout acte fait foi ; \
             il résulte de l'article 34 du code qu'elle invoque que la filiation est établie.",
        );
        let c = by_article(&out, "34");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CC"));
    }

    #[test]
    fn orphan_enumeration_heads_inherit() {
        let out = cites(
            "Aux termes de l'article 47 du code civil tout acte fait foi ; l'acte est \
             incomplet au regard des articles 84, 92 et 99 du code de la famille congolais.",
        );
        for num in ["84", "92", "99"] {
            assert_eq!(by_article(&out, num).target.ref_text_uid, None, "art {num}");
        }

        let out = cites("les articles 1240, 1241 et 1242 du code civil s'appliquent.");
        for num in ["1240", "1241", "1242"] {
            assert_eq!(
                by_article(&out, num).target.ref_text_uid.as_deref(),
                Some("LEGI_CC"),
                "art {num}"
            );
        }
    }

    #[test]
    fn orphan_genitive_never_guesses() {
        let out = cites(
            "Vu la loi n° 91-647 du 10 juillet 1991. Le ministre s'est fondé sur \
             l'article 37 du code de la famille congolais pour refuser le visa.",
        );
        let c = by_article(&out, "37");
        assert_eq!(c.target.ref_text_uid, None);
    }

    #[test]
    fn zone_designator_genitive_is_orphan() {
        let out = cites(
            "Vu la loi n° 91-647 du 10 juillet 1991. Considérant que l'article 37 UA du POS \
             prévoit que, pour être constructible, un terrain doit avoir un accès.",
        );
        let c = by_article(&out, "37");
        assert_eq!(c.target.ref_text_uid, None);
    }

    #[test]
    fn missing_du_still_attaches() {
        let out = cites("condamné au titre de l'article 1240 code civil à payer.");
        assert_eq!(
            by_article(&out, "1240").target.ref_text_uid.as_deref(),
            Some("LEGI_CC")
        );
    }

    #[test]
    fn single_attached_instrument_has_no_own_span() {
        let out = cites("Vu l'article 1240 du code civil.");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].article.as_deref(), Some("1240"));
        let out = cites("Vu le code civil.");
        assert_eq!(out.len(), 1);
        assert!(out[0].article.is_none());
    }

    #[test]
    fn structural_dated_act_without_catalog_still_spans() {
        let out = cites("en application de la loi n° 2099-1 du 1er janvier 2099 précitée.");
        assert_eq!(out.len(), 1);
        assert!(out[0].article.is_none());
        assert_eq!(out[0].target.ref_text_uid, None);
    }

    #[test]
    fn local_act_emits_no_span() {
        let out = cites("par un arrêté préfectoral du 3 mai 2019, le préfet a refusé.");
        assert!(out.is_empty());
    }

    #[test]
    fn reproduced_block_ties_break_on_nearest_resolved_citation() {
        let out = cites(
            "Les articles L. 212-8 et L. 212-8-5 du code du travail de l'époque \
             admettaient la conclusion d'un tel accord d'entreprise : \
             - Article L212-8- \" Une convention ou un accord collectif étendu \
             ou un accord d'entreprise peut prévoir que la durée hebdomadaire \
             varie sur tout ou partie de l'année. \" Les salariés agricoles \
             sont mentionnés à l'article 1144 du code rural.",
        );
        let c = by_article(&out, "L212-8");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_TRAV"));
        assert_eq!(c.target.ref_num_key.as_deref(), Some("l212-8"));
    }

    #[test]
    fn fiscal_letter_suffix_articles_resolve() {
        let out = cites(
            "Il résulte de l'article 164 B du code général des impôts que le              contribuable est imposable. L'article L. 16 A du livre des              procédures fiscales encadre la demande.",
        );
        let c = by_article(&out, "164 B");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CGI"));
        assert_eq!(c.target.ref_num_key.as_deref(), Some("164-b"));
        let c = by_article(&out, "L. 16 A");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_LPF"));
        assert_eq!(c.target.ref_num_key.as_deref(), Some("l16-a"));
    }

    #[test]
    fn fiscal_letter_ordinal_and_arrete_prefix_resolve() {
        let out = cites(
            "La valeur locative est fixée selon l'article 1518 A quinquies du              code général des impôts. Les émoluments sont tarifés à l'article              A. 444-32 du code de commerce.",
        );
        let c = by_article(&out, "1518 A quinquies");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CGI"));
        assert_eq!(c.target.ref_num_key.as_deref(), Some("1518-a-quinquies"));
        let c = by_article(&out, "A. 444-32");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_COM"));
        assert_eq!(c.target.ref_num_key.as_deref(), Some("a444-32"));
    }

    #[test]
    fn title_citing_articles_reopens_enumeration() {
        let out = cites(
            "Vu : - l'arrêté du 27 décembre 2016 relatif aux conditions \
             d'établissement et de transmission des certificats médicaux, \
             rapports médicaux et avis mentionnés aux articles R. 313-22, \
             R. 313-23 et R. 511-1 du code de l'entrée et du séjour des \
             étrangers et du droit d'asile ;",
        );
        let c = by_article(&out, "R. 313-22");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CESEDA"));
        assert_eq!(c.target.ref_num_key.as_deref(), Some("r313-22"));
    }

    #[test]
    fn spans_are_sorted_and_non_overlapping() {
        let out = cites(
            "Vu : - le livre des procédures fiscales ; - l'arrêté du 12 février \
             2010 pris en application du deuxième alinéa du 1 de l'article \
             238-0 A du code général des impôts ; - le code de justice \
             administrative. Considérant ce qui suit :",
        );
        let arrete = out
            .iter()
            .find(|c| c.target.ref_text_uid.as_deref() == Some("JORF_2010"))
            .expect("mention d'arrêté absente");
        assert!(arrete.article.is_none());
        let mut prev_end = 0usize;
        for c in &out {
            assert!(
                c.char_start < c.char_end && c.char_start >= prev_end,
                "span [{}, {}) après end={prev_end} — tri/chevauchement violé",
                c.char_start,
                c.char_end
            );
            prev_end = c.char_end;
        }
    }

    #[test]
    fn possessive_articles_visa_binds_to_preceding_instrument() {
        let out = cites(
            "Vu les autres pièces produites. Vu le code de justice administrative \
             et notamment ses articles L. 761-1, R. 771-5 et R. 771-7. \
             Considérant ce qui suit :",
        );
        for art in ["L. 761-1", "R. 771-5", "R. 771-7"] {
            let c = by_article(&out, art);
            assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CJA"), "{art}");
        }
    }

    #[test]
    fn tel_que_modifie_prose_is_not_a_genitive_gap() {
        let out = cites(
            "Vu le code de justice administrative, notamment ses articles \
             L. 761-1 et R. 771-5, tel que modifié par le décret n° 87-1034 \
             du 22 décembre 1987. Considérant ce qui suit :",
        );
        for art in ["L. 761-1", "R. 771-5"] {
            let c = by_article(&out, art);
            assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CJA"), "{art}");
        }
    }

    #[test]
    fn ambiguous_bare_prefixed_article_abstains() {
        let out = cites(
            "L'article L. 212-8-5 du code du travail et l'article 1144 du code \
             rural sont invoqués. Selon l'accord d'entreprise applicable, le \
             moyen tiré de l'article L. 212-8 est fondé.",
        );
        let c = by_article(&out, "L. 212-8");
        assert_eq!(c.target.ref_text_uid, None);
    }

    #[test]
    fn degree_enumeration_bridges_genitive_adjacency() {
        let out = cites(
            "le préfet s'est fondé sur les dispositions de l'article L. 611-3 \
             1° et 5° du code de l'entrée et du séjour des étrangers et du \
             droit d'asile.",
        );
        let c = by_article(&out, "L. 611-3");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CESEDA"));
    }

    #[test]
    fn roman_paragraph_bridges_distributive_enumeration() {
        let out = cites(
            "Vu les articles L. 761-1, I, du code de justice administrative \
             et 37 de la loi n° 91-647 du 10 juillet 1991 ;",
        );
        assert_eq!(
            by_article(&out, "L. 761-1").target.ref_text_uid.as_deref(),
            Some("LEGI_CJA")
        );
        let c = by_article(&out, "37");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("JORF_AJ"));
        assert_eq!(c.target.ref_num_key.as_deref(), Some("37"));
    }

    #[test]
    fn bare_nature_words_are_generic() {
        let out = cites(
            "La loi permet au juge d'apprécier ; la décision attaquée méconnaît \
             le code et la convention des parties.",
        );
        assert!(out.is_empty());
    }

    // ── citations de jurisprudence ──────────────────────────────────────────

    #[test]
    fn case_pourvoi_anchored_and_enumeration() {
        assert_eq!(
            case_surfaces("sur le pourvoi n° B 18-23.954 formé par M. X"),
            vec![("18-23.954".to_string(), "cc|1823954".to_string())]
        );
        assert_eq!(
            case_surfaces("les pourvois n° 18-23.954 et 18-23.955 sont joints"),
            vec![
                ("18-23.954".to_string(), "cc|1823954".to_string()),
                ("18-23.955".to_string(), "cc|1823955".to_string()),
            ]
        );
    }

    #[test]
    fn case_cassation_window_bridges_chamber_and_date() {
        assert_eq!(
            case_surfaces("voir Cour de cassation, 3e chambre civile, 16 mars 2022, n° 21-12.345"),
            vec![("21-12.345".to_string(), "cc|2112345".to_string())]
        );
        assert_eq!(
            case_surfaces("(Cass. soc., 18 janvier 2011, n° 09-89.876)"),
            vec![("09-89.876".to_string(), "cc|0989876".to_string())]
        );
        assert!(case_surfaces("le pourvoi formé par M. X est rejeté").is_empty());
    }

    #[test]
    fn case_conseil_detat_needs_juridiction_context() {
        assert_eq!(
            case_surfaces("la décision du Conseil d'État du 12 avril 2019, n° 412412"),
            vec![("412412".to_string(), "ce|412412".to_string())]
        );
        assert!(case_surfaces("la demande enregistrée sous le n° 412412 est rejetée").is_empty());
    }

    #[test]
    fn case_constit_requires_dc_qpc_suffix() {
        assert_eq!(
            case_surfaces(
                "le Conseil constitutionnel, dans sa décision n° 2020-800 DC du 26 juin 2020"
            ),
            vec![("2020-800".to_string(), "constit|2020-800".to_string())]
        );
        assert_eq!(
            case_surfaces("selon la décision n° 2019-781 QPC du Conseil"),
            vec![("2019-781".to_string(), "constit|2019-781".to_string())]
        );
        assert!(case_surfaces("la décision n° 2020-1310 du 29 octobre 2020").is_empty());
    }

    /// Surfaces relevées dans le fonds judiciaire : c'est le juge judiciaire
    /// qui cite le Tribunal des conflits, au moment de se déclarer incompétent.
    #[test]
    fn case_tc_reads_the_short_docket_behind_the_date() {
        for (text, num) in [
            ("(TC, 8 février 1873, [U], n° 00012)", "00012"),
            (
                "Vu la décision du Tribunal des conflits, 8 février 1873, Blanco, n° 00012",
                "00012",
            ),
            (
                "la décision du Tribunal des conflits du 8 février 1873 (n° 00012 dit « [K] »)",
                "00012",
            ),
            ("TC, 23 octobre 2000, n° 3220)", "3220"),
            ("TC 9 décembre 2019, N° C4174, à propos des soins", "C4174"),
            (
                "TC 28 mars 2011 Commune de la Clusaz c/ Société mutuelle \
                 d'assurance du bâtiment n° 3773)",
                "3773",
            ),
            (
                "(TC, 7 juillet 2014, Mme Voguet c/ Centre de prestations \
                 sociales interministérielles, n C3952)",
                "C3952",
            ),
        ] {
            let key = CaseAddress::new("tc", &lj_core::docket_key::tc_ordinal(num), "");
            assert_eq!(
                case_surfaces(text),
                vec![(num.to_string(), addr_str(&key))],
                "surface {text:?}"
            );
        }
    }

    #[test]
    fn case_tc_pourvoi_is_not_a_cassation_reference() {
        assert_eq!(
            case_surfaces("TC, 7 juillet 2014, pourvoi n° 14-03.959"),
            vec![("14-03.959".to_string(), "tc|1403959".to_string())]
        );
    }

    /// Le visa d'un tribunal judiciaire, verbatim : trois décisions d'affilée,
    /// nom d'usage intercalé, et le `C` détaché du numéro moderne.
    #[test]
    fn case_tc_visa_enumerates_one_span_per_decision() {
        assert_eq!(
            case_surfaces(
                "Vu la décision du Tribunal des conflits, 8 février 1873, Blanco, \
                 n° 00012 Vu la décision du Tribunal des conflits, 22 janvier 1921, \
                 Société commerciale de l'Ouest africain, n° 00706 Vu la décision du \
                 Tribunal des conflits, 23 avril 2007, n° C 3451"
            ),
            vec![
                ("00012".to_string(), "tc|12".to_string()),
                ("00706".to_string(), "tc|706".to_string()),
                ("C 3451".to_string(), "tc|3451".to_string()),
            ]
        );
    }

    /// The docket ahead of the anchor, the shape of a visa.
    #[test]
    fn case_tc_reads_the_docket_ahead_of_the_anchor() {
        for (text, num) in [
            (
                "Vu la décision n° 3124 du Tribunal des Conflits du 23 novembre 1998 ;",
                "3124",
            ),
            (
                "Vu les autres pièces du dossier ; Vu la décision n° 3869 du 15 \
                 octobre 2012 du Tribunal des conflits ;",
                "3869",
            ),
            (
                "tendant à ce que la décision n° 3940 du 19 mai 2014, par laquelle \
                 le Tribunal des conflits a déclaré",
                "3940",
            ),
            (
                "Par une décision n° 4238 du 14 mars 2022, le Tribunal des conflits \
                 a déclaré la juridiction administrative compétente",
                "4238",
            ),
        ] {
            let key = CaseAddress::new("tc", &lj_core::docket_key::tc_ordinal(num), "");
            assert_eq!(
                case_surfaces(text),
                vec![(num.to_string(), addr_str(&key))],
                "surface {text:?}"
            );
        }
    }

    /// L'énumération du visa administratif, verbatim : trois dates, un nom
    /// d'usage par décision, puis une liste nue de numéros.
    #[test]
    fn case_tc_enumerates_behind_one_anchor() {
        assert_eq!(
            case_surfaces(
                "- les décisions du Tribunal des conflits du 20 octobre 1997, \
                 Préfet du Finistère, n° 03086, du 12 décembre 2005, Commune de \
                 Cestas, n° 3485 et du 22 novembre 2011, M. C. et autres c/ Lycée \
                 David d'Angers, n° 3789, 3790, 3791 ;"
            ),
            vec![
                ("03086".to_string(), "tc|3086".to_string()),
                ("3485".to_string(), "tc|3485".to_string()),
                ("3789".to_string(), "tc|3789".to_string()),
                ("3790".to_string(), "tc|3790".to_string()),
                ("3791".to_string(), "tc|3791".to_string()),
            ]
        );
        assert_eq!(
            case_surfaces(
                "arrêts rendus par le Tribunal des conflits le 8 avril 2019 sous \
                 le n° 4154 et le 14 juin 2021 sous le n° 4209 ;"
            ),
            vec![
                ("4154".to_string(), "tc|4154".to_string()),
                ("4209".to_string(), "tc|4209".to_string()),
            ]
        );
        // Le maillon nu s'arrête devant un mot : la somme n'énumère pas.
        assert_eq!(
            case_surfaces("Tribunal des conflits, 3 juillet 2023, n° 4283, 1500 euros"),
            vec![("4283".to_string(), "tc|4283".to_string())]
        );
    }

    /// Le nom de l'acte s'intercale entre l'ancre et la date.
    #[test]
    fn case_tc_reads_the_date_behind_the_kind_of_act() {
        for (text, num) in [
            (
                "il résulte de la jurisprudence (Tribunal des conflits, ordonnance \
                 du 9 octobre 2017, n°4106) que les juridictions",
                "4106",
            ),
            (
                "le Tribunal des conflits a jugé, par une ordonnance du 17 juillet \
                 2024 au n°4320, que c'est à tort",
                "4320",
            ),
        ] {
            let key = CaseAddress::new("tc", &lj_core::docket_key::tc_ordinal(num), "");
            assert_eq!(
                case_surfaces(text),
                vec![(num.to_string(), addr_str(&key))],
                "surface {text:?}"
            );
        }
    }

    #[test]
    fn case_tc_reads_the_docket_before_its_date_ahead_of_the_anchor() {
        assert_eq!(
            case_surfaces(
                "requête enregistrée au secrétariat du Tribunal des conflits, tendant \
                 à ce que la décision n° 3643 en date du 17 décembre 2007, notifiée le \
                 29 janvier 2008, soit rapportée"
            ),
            vec![("3643".to_string(), "tc|3643".to_string())]
        );
        assert!(
            case_surfaces(
                "le Tribunal des conflits ait tranché cette question. Par une décision \
                 n° 4374 du 8 juin 2026, ce tribunal a jugé"
            )
            .is_empty(),
            "l'ancre et le numéro sont dans deux phrases"
        );
    }

    #[test]
    fn case_tc_sigle_needs_the_date_and_a_docket_shaped_number() {
        // Tribunal de commerce, et terme de comparaison d'expropriation.
        assert!(case_surfaces(
            "DECISION du Président du TC de Caen en date du 11 Mai 2023 RG n° 2023001895"
        )
        .is_empty());
        assert!(case_surfaces("le TC n° 2 proposé par l'expropriant est écarté").is_empty());
        // Citation sans numéro : pas de clé, donc pas de span.
        assert!(case_surfaces("TC 8 février 1873, arrêt dit [B]").is_empty());
        assert!(case_surfaces("TC 29 février 1908").is_empty());
        // Le sigle en minuscules n'est pas la juridiction.
        assert!(case_surfaces("tc, 8 février 1873, n° 00012").is_empty());
        // Le décret visé derrière la citation porte un millésime, pas un numéro
        // du fonds.
        assert!(case_surfaces(
            "Vu l'arrêt du tribunal des conflits du 17 octobre 2011 dit SCEA du \
             CHENEAU, Vu le Décret n° 2015-233 du 27 février 2015"
        )
        .is_empty());
    }

    #[test]
    fn case_cedh_sigle_disambiguates_court_from_convention() {
        let text = "CEDH, arrêt X. c. France du 3 octobre 2014, req. n° 30010/10";
        assert_eq!(
            case_surfaces(text),
            vec![("30010/10".to_string(), "cedh|30010/10".to_string())]
        );
        assert!(
            cites(text).is_empty(),
            "le sigle ne doit pas lier la Convention"
        );
        let conv = "sur le fondement de l'article 5 de la CEDH et de la loi";
        assert!(cases(conv).is_empty());
        assert!(cites(conv).iter().any(|c| c.text_key == "cedh"));
    }

    #[test]
    fn case_cjue_prefix_required_in_window_but_not_after_affaire() {
        assert_eq!(
            case_surfaces("l'arrêt de la CJUE du 6 octobre 2021, C-561/19, point 27"),
            vec![("C-561/19".to_string(), "cjue|c-561/19".to_string())]
        );
        assert_eq!(
            case_surfaces("l'arrêt rendu dans l'affaire 6/64, Costa contre ENEL"),
            vec![("6/64".to_string(), "cjue|6/64".to_string())]
        );
        assert_eq!(
            case_surfaces("dans les affaires jointes C-402/05 et C-415/05, le juge"),
            vec![
                ("C-402/05".to_string(), "cjue|c-402/05".to_string()),
                ("C-415/05".to_string(), "cjue|c-415/05".to_string()),
            ]
        );
        assert!(case_surfaces("l'arrêt applique le règlement 44/2001 du Conseil").is_empty());
    }

    #[test]
    fn case_cjue_party_names_and_graphies() {
        assert_eq!(
            case_surfaces(
                "au visa des arrêts MIT C-431/04, GSK C-210/13, Bayer C-11/13 \
                 et Forsgren C-631/13 de la CJUE"
            ),
            vec![
                ("C-431/04".to_string(), "cjue|c-431/04".to_string()),
                ("C-210/13".to_string(), "cjue|c-210/13".to_string()),
                ("C-11/13".to_string(), "cjue|c-11/13".to_string()),
                ("C-631/13".to_string(), "cjue|c-631/13".to_string()),
            ]
        );
        assert_eq!(
            case_surfaces("l'arrêt Forsgren C-631-13 du 15 janvier 2015"),
            vec![("C-631-13".to_string(), "cjue|c-631/13".to_string())]
        );
        assert_eq!(
            case_surfaces("dans son arrêt Forsgren C631/13 du 15 janvier 2015"),
            vec![("C631/13".to_string(), "cjue|c-631/13".to_string())]
        );
        assert_eq!(
            case_surfaces("l'ordonnance Glaxosmithkline C-210/13 du 14 novembre 2013"),
            vec![("C-210/13".to_string(), "cjue|c-210/13".to_string())]
        );
        assert!(case_surfaces("l'ordonnance n° 58-1067 du 7 novembre 1958").is_empty());
        assert_eq!(
            case_surfaces("dans l'affaire déjà citée C-210/13 Glaxosmithkline"),
            vec![("C-210/13".to_string(), "cjue|c-210/13".to_string())]
        );
    }

    #[test]
    fn case_rg_requires_upstream_jurisdiction_and_uppercase() {
        assert_eq!(
            case_surfaces(
                "le jugement du tribunal judiciaire de Paris du 12 mai 2021, RG n° 21/04532"
            ),
            vec![("21/04532".to_string(), "tj75056|21/04532".to_string())]
        );
        assert_eq!(
            case_surfaces("l'arrêt de la cour d'appel d'Aix-en-Provence (RG 19/12345)"),
            vec![(
                "19/12345".to_string(),
                "ca_aix_provence|19/12345".to_string()
            )]
        );
        assert_eq!(
            case_surfaces("l'affaire enregistrée RG n° 21/04532 au greffe"),
            vec![("21/04532".to_string(), "|21/04532".to_string())]
        );
        assert_eq!(
            case_surfaces("le jugement du tribunal judiciaire de Meaux, RG n° 21/04532"),
            vec![("21/04532".to_string(), "|21/04532".to_string())]
        );
    }

    /// (surface, clé) attendues, dans l'ordre.
    fn assert_cases(text: &str, expected: &[(&str, &str)]) {
        let got = case_surfaces(text);
        let want: Vec<(String, String)> = expected
            .iter()
            .map(|(s, k)| (s.to_string(), k.to_string()))
            .collect();
        assert_eq!(got, want, "texte : {text}");
    }

    #[test]
    fn case_admin_ta_jugement_with_downstream_jurisdiction() {
        assert_cases(
            "Par un jugement n° 1901563 du 15 février 2023, le tribunal administratif \
             de Nantes a condamné l'ONIAM à verser une somme",
            &[("1901563", "ta_nantes|1901563")],
        );
        assert_cases(
            "Par deux jugements n° 2202850 et n° 2202851 du 16 mars 2023, le tribunal \
             administratif de Nantes a rejeté leurs demandes",
            &[
                ("2202850", "ta_nantes|2202850"),
                ("2202851", "ta_nantes|2202851"),
            ],
        );
    }

    #[test]
    fn case_admin_caa_arret_keeps_uppercase_docket() {
        assert_cases(
            "Par un arrêt n° 20NT01234 du 5 mai 2022, la cour administrative d'appel \
             de Nantes a rejeté l'appel",
            &[("20NT01234", "caa_nantes|20NT01234")],
        );
    }

    #[test]
    fn case_ce_backward_probe_with_enumeration_and_guard() {
        assert_cases(
            "Par une décision n° 432537 du 8 janvier 2020, le Conseil d'Etat statuant \
             au contentieux a annulé cet arrêt",
            &[("432537", "ce|432537")],
        );
        assert_cases(
            "tranchées par la décision nos 431188, 431348 du 22 mars 2021 du Conseil \
             d'Etat, statuant au contentieux",
            &[("431188", "ce|431188"), ("431348", "ce|431348")],
        );
        assert_cases(
            "la décision n° 432537 du tribunal administratif de Rennes, puis le Conseil \
             d'Etat a rejeté le surplus",
            &[],
        );
    }

    #[test]
    fn case_ce_ordonnance_refere_and_pourvoi_administratif() {
        assert_cases(
            "Par une ordonnance n° 468345 du 15 novembre 2022, notifiée le 23 novembre \
             2022, le président de la section du contentieux",
            &[("468345", "ce|468345")],
        );
        assert_cases(
            "La requête en référé n° 502527 de Mme D tendant à la suspension de la décision",
            &[("502527", "ce|502527")],
        );
        assert_cases(
            "contre laquelle il s'est pourvu en cassation sous le n° 500109 ; 2°) de mettre",
            &[("500109", "ce|500109")],
        );
        assert_cases(
            "statuant au contentieux sur les pourvois n° 476000 de la société Accorinvest \
             et n° 476009 de la Société générale",
            &[("476000", "ce|476000"), ("476009", "ce|476009")],
        );
    }

    #[test]
    fn case_constit_backward_and_enumeration() {
        assert_cases(
            "Vu la décision n°2021-823 du Conseil constitutionnel du 13 août 2021",
            &[("2021-823", "constit|2021-823")],
        );
        assert_cases(
            "des décisions du Conseil Constitutionnel n° 2016-554 QPC du 22 juillet 2016, \
             n° 2016-610 QPC du 10 février 2017 et n° 2016-614 QPC du 1er mars 2017",
            &[
                ("2016-554", "constit|2016-554"),
                ("2016-610", "constit|2016-610"),
                ("2016-614", "constit|2016-614"),
            ],
        );
        assert_cases(
            "le conseil constitutionnel a, dans sa décision n°2015-479, déclaré conforme",
            &[("2015-479", "constit|2015-479")],
        );
    }

    #[test]
    fn case_cedh_arret_and_enumeration() {
        assert_cases(
            "dans son arrêt n° 29217/12, Tarakhel c. Suisse, rendu en grande chambre",
            &[("29217/12", "cedh|29217/12")],
        );
        assert_cases(
            "l'arrêt de la Cour européenne des droits de l'homme du 18 novembre 2021 \
             n°15670/18 et 43115/18 M. A et autres c. Croatie",
            &[("15670/18", "cedh|15670/18"), ("43115/18", "cedh|43115/18")],
        );
        let pad = "considérant que la cour a déjà statué ; ".repeat(35);
        assert_cases(
            &format!("{pad}ARRÊT AU FOND DU 25 MARS 2011 N° 2011/183 Rôle N° 10/01539"),
            &[("2011/183", "|2011/183"), ("10/01539", "|10/01539")],
        );
        assert_cases(
            "ARRÊT AU FOND DU 25 MARS 2011 N° 2011/183 Rôle N° 10/01539",
            &[("10/01539", "|10/01539")],
        );
        assert_cases(
            "l'arrêt rendu au visa de l'article 3 du règlement n° 2988/95 du Conseil",
            &[],
        );
        assert_cases(
            "jugé, dans l'affaire 26604/16 Waldner c. France, que la méthode choisie",
            &[("26604/16", "cedh|26604/16")],
        );
    }

    #[test]
    fn case_cjue_enumeration_range_and_pourvoi_guard() {
        assert_cases(
            "les arrêts de la Cour de justice de l'Union européenne C-383/13 du \
             10 septembre 2013, C-166/13 du 5 novembre 2014 et C-249/13 du 11 décembre 2014",
            &[
                ("C-383/13", "cjue|c-383/13"),
                ("C-166/13", "cjue|c-166/13"),
                ("C-249/13", "cjue|c-249/13"),
            ],
        );
        assert_cases(
            "Par son arrêt C-338/11 à 347/11 Santander Asset Management du 10 mai 2012",
            &[("C-338/11", "cjue|c-338/11"), ("347/11", "cjue|c-347/11")],
        );
        assert_cases(
            "a rendu l'arrêt suivant : Pourvoi n° T 17-21.405 RÉPUBLIQUE FRANÇAISE",
            &[("17-21.405", "cc|1721405")],
        );
    }

    #[test]
    fn case_cjue_pourvoi_guard_holds_on_the_spaced_tail_and_spares_the_paragraph_number() {
        assert_cases(
            "a rendu l'arrêt suivant : Joint les pourvois n° E 12-11. 828 et n° C 12-12. 907",
            &[("12-11. 828", "cc|1211828"), ("12-12. 907", "cc|1212907")],
        );
        assert_cases(
            "Cour de justice de l'Union européenne du 21 décembre 2011, n° C-495/10. 31. Dans le cas où",
            &[("C-495/10", "cjue|c-495/10")],
        );
        assert_cases(
            "un arrêt du 14 mai 2019 (CJUE 14 mai 2019 C 55-18) qui a indiqué que",
            &[("C 55-18", "cjue|c-55/18")],
        );
    }

    #[test]
    fn case_cjue_only_the_general_court_numbers_past_three_digits() {
        assert_cases(
            "arrêt du 16 juillet 2025, Rimorchiatori Riuniti Panfido c/ CINEA (T-1193/23)",
            &[("T-1193/23", "cjue|t-1193/23")],
        );
        assert_cases(
            "arrêt du 13 septembre 2018, Birkenstock Sales c/ EUIPO, C-26/17 P, EU C2018/714",
            &[("C-26/17", "cjue|c-26/17")],
        );
        assert_cases(
            "CJUE, arrêt du 27 mars 2014, LCL c/Fesih Kalhan, C6565-12) dans lequel elle",
            &[],
        );
    }

    #[test]
    fn case_cc_paren_ranges_and_dated_enumerations() {
        assert_cases(
            "sur renvoi après cassation par arrêt en date du 29 juin 2017 (16-13.988), \
             d'un arrêt rendu le 14 janvier 2016",
            &[("16-13.988", "cc|1613988")],
        );
        assert_cases(
            "Vu leur connexité, joint les pourvois n° T 00-44.843 au n° W 00-44.846 et \
             n° E 00-45.038 au n° G 00-45.041 ;",
            &[
                ("00-44.843", "cc|0044843"),
                ("00-44.846", "cc|0044846"),
                ("00-45.038", "cc|0045038"),
                ("00-45.041", "cc|0045041"),
            ],
        );
        assert_cases(
            "(Civ. 1ère 11 mars 1989, n 87-10798 ; 25 juin 1996, n 94-15130)",
            &[("87-10798", "cc|8710798"), ("94-15130", "cc|9415130")],
        );
        assert_cases(
            "soumis au droit irlandais (Cass, com, 18 décembre 2019, 18-14.827).",
            &[("18-14.827", "cc|1814827")],
        );
    }

    #[test]
    fn case_aff_sans_prefixe_le_millesime_a_deux_chiffres_seul_est_une_affaire_de_la_cour() {
        assert_cases("aff. 6/64, Costa contre ENEL", &[("6/64", "cjue|6/64")]);
        assert_cases(
            "l'affaire n° 20/515 a fait l'objet d'une remise au rôle",
            &[("20/515", "|20/515")],
        );
        assert_cases(
            "jointe au dossier de l'affaire n° 20/1347",
            &[("20/1347", "|20/1347")],
        );
        assert_cases(
            "l'affaire C-782/19 concerne la même question",
            &[("C-782/19", "cjue|c-782/19")],
        );
    }

    #[test]
    fn case_rg_enrol_deferee_and_own_header_guards() {
        assert_cases(
            "décision attaquée en date du 05 octobre 2020, enregistrée sous le no 20/00325",
            &[("20/00325", "|20/00325")],
        );
        assert_cases(
            "DÉCISION DÉFÉRÉE :  21/00287  Jugement du TJ HORS JAF",
            &[("21/00287", "|21/00287")],
        );
        assert_cases(
            "Appel d'une décision (No RG 09/ 29) rendue par le juge de l'expropriation",
            &[("09/ 29", "|09/29")],
        );
        assert_cases(
            "Vu la requête en prolongation de rétention, enregistrée sous le N°RG \
             25/04461 présentée par M. le Préfet",
            &[],
        );
        assert_cases(
            "N° RG 22/01114 - N° Portalis DBVF-V-B7G-GAYK MINUTE N°",
            &[],
        );
        assert_cases(
            "jugement du Tribunal de Commerce de PARIS - RG n° 2023063685",
            &[("2023063685", "tcom7501|2023063685")],
        );
        assert_cases(
            "jugement du Tribunal de Commerce de PARIS du 16 Novembre 2015 enregistré(e) \
             au répertoire général sous le n° 2015F00459",
            &[("2015F00459", "tcom7501|2015F00459")],
        );
        assert_cases(
            "d'un jugement rendu le 10/05/2016 (instance n° 14-08937) par le tribunal \
             de grande instance de Paris",
            &[("14-08937", "tj75056|14/08937")],
        );
    }

    #[test]
    fn case_spans_do_not_perturb_instrument_extraction() {
        let text = "Vu l'article L. 761-1 du code de justice administrative ; \
                    vu l'arrêt de la CJUE du 6 octobre 2021, C-561/19 ; \
                    vu l'article 1240 du code civil ;";
        let out = cites(text);
        assert_eq!(
            by_article(&out, "L. 761-1").target.ref_text_uid.as_deref(),
            Some("LEGI_CJA")
        );
        assert_eq!(
            by_article(&out, "1240").target.ref_text_uid.as_deref(),
            Some("LEGI_CC")
        );
        assert_eq!(
            case_surfaces(text),
            vec![("C-561/19".to_string(), "cjue|c-561/19".to_string())]
        );
    }

    fn forum_engine() -> (CompiledVocab, LinkSnapshot) {
        let texts = vec![
            text("INTL/CEDH", "Convention de sauvegarde des droits de l'homme et des libertés fondamentales", "Convention de sauvegarde des droits de l'homme et des libertés fondamentales", "TRAITE", 0),
            text("EU/RPROC/CJUE", "Règlement de procédure de la Cour de justice", "Règlement de procédure de la Cour de justice", "REGLEMENT", 0),
            text("EU/RPROC/TRIBUNAL", "Règlement de procédure du Tribunal", "Règlement de procédure du Tribunal", "REGLEMENT", 0),
            text("JORFTEXT000000537611", "Loi n° 91-647 du 10 juillet 1991 relative à l'aide juridique", "Loi du 10 juillet 1991 relative à l'aide juridique", "LOI", 60),
            text("FR/PREAMBULE-1946", "Préambule de la Constitution du 27 octobre 1946", "Préambule de la Constitution du 27 octobre 1946", "CONSTITUTION", 0),
            text("INTL/CONV-108", "Convention pour la protection des personnes à l'égard du traitement automatisé des données à caractère personnel, faite à Strasbourg le 28 janvier 1981", "Convention pour la protection des personnes à l'égard du traitement automatisé des données à caractère personnel", "TRAITE", 27),
            text("INTL/CONVENTION-INTERNATIONALE-TRAVAIL-158-CESSATION-1990", "Convention internationale du travail n° 158 concernant la cessation de la relation de travail à l'initiative de l'employeur", "Convention internationale du travail n° 158 concernant la cessation de la relation de travail à l'initiative de l'employeur", "CONVENTION", 0),
            text("EU/REG/1307-2013", "Règlement (UE) n° 1307/2013 du Parlement européen et du Conseil du 17 décembre 2013 établissant les règles relatives aux paiements directs en faveur des agriculteurs", "Règlement (UE) n° 1307/2013", "REGLEMENT", 0),
            text("EU/REG/1306-2013", "Règlement (UE) n° 1306/2013 du Parlement européen et du Conseil du 17 décembre 2013 relatif au financement, à la gestion et au suivi de la politique agricole commune", "Règlement (UE) n° 1306/2013", "REGLEMENT", 0),
            text("JORF_L91636", "Loi n° 91-636 du 10 juillet 1991 autorisant l'approbation d'une convention sur le crédit-bail", "Loi du 10 juillet 1991 autorisant l'approbation d'une convention sur le crédit-bail", "LOI", 2),
        ];
        let articles: Vec<(String, String)> = [
            ("INTL/CEDH", "3"),
            ("INTL/CEDH", "13"),
            ("INTL/CEDH", "44"),
            ("INTL/CEDH", "45"),
            ("JORFTEXT000000537611", "37"),
            ("JORFTEXT000000537611", "75"),
            ("EU/REG/1307-2013", "52"),
        ]
        .iter()
        .map(|(u, n)| (u.to_string(), n.to_string()))
        .collect();
        let snap = LinkSnapshot::build(texts.clone(), articles);
        let vocab = CompiledVocab::build(&texts, &snap);
        (vocab, snap)
    }

    fn target_of(out: &[CompiledCitation]) -> (Option<&str>, Option<&str>) {
        (
            out[0].target.ref_text_uid.as_deref(),
            out[0].target.ref_num_key.as_deref(),
        )
    }

    #[test]
    fn forum_rproc_links_by_court() {
        let (vocab, snap) = forum_engine();
        let boilerplate = "Aux termes de l'article 69, paragraphe 2, du règlement \
                           de procédure, toute partie qui succombe est condamnée aux dépens.";
        let out = extract_citations(boilerplate, &vocab, &snap, Some(Forum::CjueCour));
        assert_eq!(target_of(&out), (Some("EU/RPROC/CJUE"), None));
        let out = extract_citations(boilerplate, &vocab, &snap, Some(Forum::CjueTribunal));
        assert_eq!(target_of(&out), (Some("EU/RPROC/TRIBUNAL"), None));
        let out = extract_citations(boilerplate, &vocab, &snap, None);
        assert_eq!(target_of(&out), (None, None));
    }

    #[test]
    fn forum_cedh_bare_convention_is_cesdh() {
        let (vocab, snap) = forum_engine();
        let out = extract_citations(
            "Cet arrêt est devenu définitif en vertu de l'article 44 § 2 de la Convention.",
            &vocab,
            &snap,
            Some(Forum::Cedh),
        );
        assert_eq!(target_of(&out), (Some("INTL/CEDH"), Some("44")));
        let out = extract_citations(
            "conformément aux articles 45 § 2 de la Convention et 74 § 2 du règlement,",
            &vocab,
            &snap,
            Some(Forum::Cedh),
        );
        assert_eq!(target_of(&out), (Some("INTL/CEDH"), Some("45")));
        assert!(out.iter().skip(1).all(|c| c.target.ref_text_uid.is_none()));
        let out = extract_citations(
            "la violation de l'article 3 de la convention d'indemnisation conclue entre les parties",
            &vocab,
            &snap,
            Some(Forum::Cedh),
        );
        assert_eq!(target_of(&out), (None, None));
    }

    #[test]
    fn a_swiss_court_never_cites_a_french_text() {
        let texts = vec![
            text(
                "LEGITEXT000006070716",
                "Code de procédure civile",
                "Code de procédure civile",
                "CODE",
                900,
            ),
            text(
                "JORFTEXT000000537611",
                "Loi n° 91-647 du 10 juillet 1991 relative à l'aide juridique",
                "Loi du 10 juillet 1991 relative à l'aide juridique",
                "LOI",
                60,
            ),
        ];
        let articles: Vec<(String, String)> = [
            ("LEGITEXT000006070716", "106"),
            ("JORFTEXT000000537611", "75"),
        ]
        .iter()
        .map(|(u, n)| (u.to_string(), n.to_string()))
        .collect();
        let snap = LinkSnapshot::build(texts.clone(), articles);
        let vocab = CompiledVocab::build(&texts, &snap);
        let federal_court = Forum::of(Some("COUR_SUPREME"), None, Some("geo:CH"), None);
        assert!(matches!(federal_court, Some(Forum::Foreign(p, None)) if p.as_str() == "CH"));
        let cpc = "les frais sont répartis selon l'article 106 du code de procédure civile";
        let out = extract_citations(cpc, &vocab, &snap, federal_court);
        assert_eq!(target_of(&out), (None, None));
        let out = extract_citations(cpc, &vocab, &snap, None);
        assert_eq!(target_of(&out).0, Some("LEGITEXT000006070716"));
        let out = extract_citations(
            "au titre de l'article 75 de la loi du 10 juillet 1991",
            &vocab,
            &snap,
            federal_court,
        );
        assert_eq!(target_of(&out), (None, None));
    }

    #[test]
    fn a_foreign_forum_keeps_its_spans_sorted_and_apart() {
        let text =
            "12 Article 16-3 du Règlement (UE) 2024/1351 elle fait suite à un rejet définitif";
        for (vocab, snap) in [engine(), eu_engine()] {
            for place in ["BE", "CH", "LU"] {
                let forum = Some(Forum::Foreign(
                    crate::link::Place::new(place).unwrap(),
                    None,
                ));
                let out = extract_citations(text, &vocab, &snap, forum);
                assert!(!out.is_empty());
                assert!(
                    out.windows(2).all(|w| w[0].char_end <= w[1].char_start),
                    "{place}: {:?}",
                    out.iter()
                        .map(|c| (c.char_start, c.char_end))
                        .collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn an_act_cited_by_its_number_takes_the_act_of_the_place_carrying_it() {
        let law = |uid: &str, title: &str, title_key: &str, signed: Option<jiff::civil::Date>| {
            let mut t = text(uid, title, title_key, "LOI", 60);
            t.jurisdiction = Some(uid[..2].to_string());
            t.date_texte = signed;
            t
        };
        let texts = vec![
            law(
                "BJ/LOI-2004-07",
                "Loi n° 2004-07 du 23 octobre 2007 portant composition de la Cour suprême",
                "Loi du 23 octobre 2007",
                None,
            ),
            law(
                "BJ/LOI-2007-27",
                "Loi N° 2007-27 du 23 octobre 2007 portant institution du service militaire",
                "Loi du 23 octobre 2007",
                None,
            ),
            law(
                "BJ/LOI-2008-07",
                "Loi N° 2008-07 du 28 février 2008 portant code de procédure civile",
                "Loi du 28 février 2008",
                Some(jiff::civil::date(2011, 2, 28)),
            ),
            law(
                "MA/CODE-FAMILLE",
                "Code de la famille",
                "Code de la famille",
                None,
            ),
        ];
        let articles: Vec<(String, String)> = ["BJ/LOI-2004-07", "BJ/LOI-2007-27"]
            .iter()
            .map(|u| (u.to_string(), "51".to_string()))
            .collect();
        let snap = LinkSnapshot::build(texts.clone(), articles).with_aliases(vec![(
            "MA/CODE-FAMILLE".to_string(),
            "number".to_string(),
            String::new(),
            "70-03".to_string(),
        )]);
        let vocab = CompiledVocab::build(&texts, &snap);
        let at = |place: &str| {
            Some(Forum::Foreign(
                crate::link::Place::new(place).unwrap(),
                None,
            ))
        };
        let linked = |cite: &str, place: &str| {
            let out = extract_citations(cite, &vocab, &snap, at(place));
            out[0].target.ref_text_uid.clone()
        };
        for (cite, place, uid) in [
            (
                "l'article 51 de la loi n° 2004-07 du 23 octobre 2007",
                "BJ",
                Some("BJ/LOI-2004-07"),
            ),
            (
                "Vu la loi n°2007-27 du 23 octobre 2007",
                "BJ",
                Some("BJ/LOI-2007-27"),
            ),
            (
                "l'article 51 de la loi n° 2004-07",
                "BJ",
                Some("BJ/LOI-2004-07"),
            ),
            (
                "l'article 51 de la loi n° 2004-07 du 5 mai 1999",
                "BJ",
                None,
            ),
            (
                "la loi n° 2008-07 du 28 février 2011",
                "BJ",
                Some("BJ/LOI-2008-07"),
            ),
            (
                "l'article 49 de la loi n° 70-03",
                "MA",
                Some("MA/CODE-FAMILLE"),
            ),
            ("l'article 49 de la loi n° 70-03", "SN", None),
        ] {
            assert_eq!(linked(cite, place).as_deref(), uid, "{place}: {cite}");
        }
    }

    #[test]
    fn a_title_the_place_carries_links_a_capture_that_reads_as_a_fragment() {
        let mut ncpc = text(
            "LU/CODE-PROCEDURE-CIVILE",
            "Nouveau Code de procédure civile luxembourgeois",
            "code de procédure civile luxembourgeois",
            "code_procedure_civile",
            1421,
        );
        ncpc.jurisdiction = Some("LU".to_string());
        let texts = vec![
            ncpc,
            text(
                "LEGITEXT000006070716",
                "Code de procédure civile",
                "Code de procédure civile",
                "CODE",
                900,
            ),
        ];
        let snap = LinkSnapshot::build(texts.clone(), vec![]).with_aliases(vec![(
            "LU/CODE-PROCEDURE-CIVILE".to_string(),
            "title".to_string(),
            "fr".to_string(),
            "Nouveau code de procédure civile".to_string(),
        )]);
        let vocab = CompiledVocab::build(&texts, &snap);
        let out = extract_citations(
            "comme visant le Nouveau code de procédure civile (NCPC)",
            &vocab,
            &snap,
            Some(Forum::Foreign(crate::link::Place::new("LU").unwrap(), None)),
        );
        assert_eq!(target_of(&out).0, Some("LU/CODE-PROCEDURE-CIVILE"));
    }

    #[test]
    fn a_uniform_act_replaced_under_its_name_reads_by_the_decision_date() {
        let (old, new) = (
            "OHADA/ACTE-UNIFORME-1997-DROIT-COMMERCIAL-GENERAL",
            "OHADA/ACTE-UNIFORME-2010-DROIT-COMMERCIAL-GENERAL",
        );
        let texts: Vec<CatalogText> = [
            (old, "Acte uniforme portant sur le droit commercial général"),
            (
                new,
                "Acte uniforme révisé portant sur le droit commercial général",
            ),
        ]
        .iter()
        .map(|(uid, title)| {
            let mut t = text(uid, title, title, "ACTE_UNIFORME", 300);
            t.jurisdiction = Some("OHADA".to_string());
            t
        })
        .collect();
        let articles = ["95", "101", "116"]
            .iter()
            .flat_map(|n| {
                [
                    (old.to_string(), n.to_string()),
                    (new.to_string(), n.to_string()),
                ]
            })
            .collect();
        let snap = LinkSnapshot::build(texts.clone(), articles).with_aliases(vec![(
            new.to_string(),
            "abbreviation".to_string(),
            "fr".to_string(),
            "AUDCG".to_string(),
        )]);
        let vocab = CompiledVocab::build(&texts, &snap);
        let linked = |cite: &str, date: Option<&str>| {
            let out = extract_citations(
                cite,
                &vocab,
                &snap,
                Forum::of(None, None, Some("OHADA"), date),
            );
            target_of(&out).0.map(str::to_string)
        };
        let generic = "l'article 95 de l'Acte uniforme relatif au droit commercial général";
        let dated = "l'article 101 de l'Acte uniforme portant sur le droit commercial général du 17 avril 1997";
        let revised =
            "l'article 116 de l'Acte uniforme révisé portant sur le droit commercial général";
        let sigle = "l’article 95 de l’AUDCG";
        let defined =
            "l’article 116 de l’Acte uniforme révisé portant sur le droit commercial général (AUXX)";
        for (cite, date, uid) in [
            (generic, Some("2009-02-26"), Some(old)),
            (generic, Some("2014-11-11"), None),
            (generic, Some("2021-10-28"), Some(new)),
            (generic, None, None),
            (dated, Some("2014-11-11"), Some(old)),
            (revised, Some("2014-11-11"), Some(new)),
            (sigle, Some("2009-02-26"), Some(old)),
            (sigle, Some("2023-04-27"), Some(new)),
            (defined, Some("2014-11-11"), Some(new)),
        ] {
            assert_eq!(linked(cite, date).as_deref(), uid, "{date:?}: {cite}");
        }
    }

    #[test]
    fn the_1998_enforcement_act_reads_by_its_date_or_the_decision_date() {
        let old = "OHADA/ACTE-UNIFORME-1998-ORGANISATION-DES-PROCEDURES-SIMPLIFIEES-DE-RECOUVREMENT-ET-DES-VOIES-D-EXECUTION";
        let new = "OHADA/ACTE-UNIFORME-2023-ORGANISATION-DES-PROCEDURES-SIMPLIFIEES-DE-RECOUVREMENT-ET-DES-VOIES-D-EXECUTION";
        let name = "Acte uniforme portant organisation des procédures simplifiées de recouvrement et des voies d'exécution";
        let texts: Vec<CatalogText> = [
            (old, format!("{name} du 10 avril 1998")),
            (new, name.to_string()),
        ]
        .iter()
        .map(|(uid, title)| {
            let mut t = text(uid, title, title, "ACTE_UNIFORME", 300);
            t.jurisdiction = Some("OHADA".to_string());
            t
        })
        .collect();
        let articles = ["15", "49", "153"]
            .iter()
            .flat_map(|n| {
                [
                    (old.to_string(), n.to_string()),
                    (new.to_string(), n.to_string()),
                ]
            })
            .collect();
        let snap = LinkSnapshot::build(texts.clone(), articles).with_aliases(vec![(
            new.to_string(),
            "abbreviation".to_string(),
            "fr".to_string(),
            "AUPSRVE".to_string(),
        )]);
        let vocab = CompiledVocab::build(&texts, &snap);
        let linked = |cite: &str, date: Option<&str>| {
            let out = extract_citations(
                cite,
                &vocab,
                &snap,
                Forum::of(None, None, Some("OHADA"), date),
            );
            target_of(&out).0.map(str::to_string)
        };
        let generic = "l'article 49 de l'Acte uniforme portant organisation des procédures simplifiées de recouvrement et des voies d'exécution";
        let dated_first = "l'article 153 de l'Acte uniforme du 10 avril 1998 portant organisation des procédures simplifiées de recouvrement et des voies d'exécution";
        let dated_last = "l'article 15 de l'Acte uniforme OHADA portant organisation des procédures simplifiées de recouvrement et des voies d'exécution du 10 avril 1998";
        let dated_bare = "l'article 49 de l'Acte uniforme 10 avril 1998 portant organisation des procédures simplifiées de recouvrement et des voies d'exécution";
        let revised = "l'article 49 de l'Acte uniforme révisé portant organisation des procédures simplifiées de recouvrement et des voies d'exécution";
        let sigle = "l'article 49 de l'AUPSRVE";
        for (cite, date, uid) in [
            (generic, Some("2011-03-15"), Some(old)),
            (generic, Some("2026-06-25"), None),
            (generic, Some("2029-02-16"), Some(new)),
            (generic, None, None),
            (dated_first, Some("2026-06-25"), Some(old)),
            (dated_last, Some("2026-06-25"), Some(old)),
            (dated_bare, Some("2026-06-25"), Some(old)),
            (revised, Some("2026-06-25"), Some(new)),
            (sigle, Some("2013-06-03"), Some(old)),
            (sigle, Some("2026-06-25"), None),
        ] {
            assert_eq!(linked(cite, date).as_deref(), uid, "{date:?}: {cite}");
        }
    }

    #[test]
    fn a_dutch_court_cites_by_title_book_and_its_own_abbreviations() {
        let (awb, bw6, iw) = ("NL/BWBR0005537", "NL/BWBR0005289", "NL/BWBR0004770");
        let cedh = "INTL/CEDH";
        let texts: Vec<CatalogText> = [
            (awb, "Algemene wet bestuursrecht", "NL"),
            (bw6, "Burgerlijk Wetboek Boek 6", "NL"),
            (iw, "Invorderingswet 1990", "NL"),
            (cedh, "Convention européenne des droits de l'homme", "INTL"),
        ]
        .iter()
        .map(|(uid, title, iso)| {
            let mut t = text(uid, title, title, "LOI", 300);
            t.jurisdiction = Some(iso.to_string());
            t
        })
        .collect();
        let articles = [(awb, "8:29"), (bw6, "119"), (iw, "7"), (cedh, "6")]
            .iter()
            .map(|(uid, n)| (uid.to_string(), n.to_string()))
            .collect();
        let alias = |uid: &str, value: &str| {
            (
                uid.to_string(),
                "abbreviation".to_string(),
                "nl".to_string(),
                value.to_string(),
            )
        };
        let snap = LinkSnapshot::build(texts.clone(), articles).with_aliases(vec![
            alias(bw6, "BW"),
            alias(bw6, "BW6"),
            alias(iw, "Iw"),
        ]);
        let vocab = CompiledVocab::build(&texts, &snap);
        let linked = |cite: &str| -> Vec<Option<String>> {
            extract_citations(cite, &vocab, &snap, Forum::of(None, None, Some("NL"), None))
                .iter()
                .map(|c| c.target.ref_text_uid.clone())
                .collect()
        };
        let some = |uid: &str| Some(uid.to_string());
        assert_eq!(
            linked("artikel 8:29, derde lid, van de Algemene wet bestuursrecht"),
            [some(awb)]
        );
        assert_eq!(linked("artikel 6:119 lid 1 BW"), [some(bw6)]);
        assert_eq!(linked("artikel 7 van de Iw"), [some(iw)]);
        assert_eq!(
            linked(
                "artikel 6, eerste lid, van het Europees Verdrag tot bescherming van de Rechten \
                 van de Mens en de fundamentele vrijheden ten aanzien van de termijn"
            ),
            [some(cedh)]
        );
        assert_eq!(
            linked(
                "Bijlage\nAlgemene wet bestuursrecht\nArtikel 8:29\nPartijen die verplicht zijn.\n\
                 Artikel 8:29\nTwee.\nParticipatiewet\nArtikel 35\nOnverminderd het bepaalde."
            ),
            [some(awb), some(awb)]
        );
        assert_eq!(
            linked(
                "de artikelen 7, eerste lid, en 12w van de Instellingswet Autoriteit Consument \
                 en Markt (Iw) en artikel 7 van de Iw. Artikel 7 van de Iw."
            ),
            [None, None, None, None]
        );
    }

    #[test]
    fn forum_cedh_annex_style_and_english() {
        let (vocab, snap) = forum_engine();
        let out = extract_citations(
            "Art. 13 - lack of any effective remedy in domestic law",
            &vocab,
            &snap,
            Some(Forum::Cedh),
        );
        assert_eq!(target_of(&out), (Some("INTL/CEDH"), Some("13")));
        let out = extract_citations(
            "Article 26 Toute personne doit pouvoir exprimer",
            &vocab,
            &snap,
            Some(Forum::Cedh),
        );
        assert_eq!(target_of(&out), (None, None));
        let out = extract_citations(
            "Article 3 of the Convention for the protection of human rights and fundamental freedoms",
            &vocab,
            &snap,
            Some(Forum::Cedh),
        );
        assert_eq!(target_of(&out), (Some("INTL/CEDH"), Some("3")));
    }

    #[test]
    fn roman_suffix_and_dated_alias_link_aide_juridique() {
        let (vocab, snap) = forum_engine();
        let out = extract_citations(
            "au titre de l'article 75-I de la loi du 10 juillet 1991",
            &vocab,
            &snap,
            None,
        );
        assert_eq!(target_of(&out), (Some("JORFTEXT000000537611"), Some("75")));
    }

    #[test]
    fn preamble_and_numbered_convention_aliases_resolve() {
        let (vocab, snap) = forum_engine();
        let out = extract_citations(
            "le onzième alinéa du Préambule de la Constitution de 1946",
            &vocab,
            &snap,
            None,
        );
        assert_eq!(
            out[0].target.ref_text_uid.as_deref(),
            Some("FR/PREAMBULE-1946")
        );
        let out = extract_citations(
            "méconnaît l'article 8 de la convention n° 108 du Conseil de l'Europe \
             pour la protection des personnes à l'égard du traitement automatisé \
             des données à caractère personnel",
            &vocab,
            &snap,
            None,
        );
        assert_eq!(target_of(&out), (Some("INTL/CONV-108"), None));
        let out = extract_citations(
            "l'article 10 de la convention n° 158 de l'organisation internationale du travail",
            &vocab,
            &snap,
            None,
        );
        assert_eq!(
            target_of(&out),
            (
                Some("INTL/CONVENTION-INTERNATIONALE-TRAVAIL-158-CESSATION-1990"),
                None
            )
        );
    }

    #[test]
    fn eu_derived_act_by_date_and_title_tokens() {
        let (vocab, snap) = forum_engine();
        let out = extract_citations(
            "l'article 52 du règlement du 17 décembre 2013 établissant les règles \
             relatives aux paiements directs en faveur des agriculteurs",
            &vocab,
            &snap,
            None,
        );
        assert_eq!(target_of(&out), (Some("EU/REG/1307-2013"), Some("52")));
        let out = extract_citations(
            "l'article 52 du règlement du 17 décembre 2013",
            &vocab,
            &snap,
            None,
        );
        assert_eq!(target_of(&out), (None, None));
        let out = extract_citations(
            "les stipulations de l'article 12 du règlement de copropriété du 3 mai 1955",
            &vocab,
            &snap,
            None,
        );
        assert_eq!(target_of(&out), (None, None));
    }

    fn territory_engine() -> (CompiledVocab, LinkSnapshot) {
        let mut texts = vec![
            text(
                "LEGITEXT000006070716",
                "Code de procédure civile",
                "Code de procédure civile",
                "CODE",
                1500,
            ),
            text("LEGI_CC", "Code civil", "Code civil", "CODE", 2800),
            text(
                "LEGI_DOU_YT",
                "Code des douanes de Mayotte",
                "Code des douanes de Mayotte",
                "CODE",
                400,
            ),
        ];
        let mut local = text(
            "FR-PF/CODE-IMPOTS",
            "Code des impôts de la Polynésie française",
            "code des impôts de la polynésie française",
            "CODE",
            200,
        );
        local.jurisdiction = Some("PF".to_string());
        texts.push(local);
        let articles = [
            ("LEGITEXT000006070716", "700"),
            ("LEGITEXT000006070716", "455"),
            ("LEGI_CC", "1240"),
            ("FR-PF/CODE-IMPOTS", "150"),
        ]
        .map(|(uid, nk)| (uid.to_string(), nk.to_string()))
        .to_vec();
        let snap = LinkSnapshot::build(texts.clone(), articles);
        let vocab = CompiledVocab::build(&texts, &snap);
        (vocab, snap)
    }

    fn targets(text: &str) -> Vec<Option<String>> {
        let (vocab, snap) = territory_engine();
        extract_citations(text, &vocab, &snap, None)
            .into_iter()
            .map(|c| c.target.ref_text_uid)
            .collect()
    }

    #[test]
    fn territorial_code_is_not_the_national_one() {
        for text in [
            "l'article 700 du code de procédure civile de la Polynésie française",
            "l'article 700 du Code de Procédure Civile de Nouvelle Calédonie",
            "l'article 700 du code de procédure civile polynésien",
            "l'article 1240 du code civil de la Polynésie française",
        ] {
            assert_eq!(targets(text), vec![None], "{text}");
        }
        let cpc = Some("LEGITEXT000006070716".to_string());
        assert_eq!(
            targets("l'article 700 du code de procédure civile applicable en Polynésie française"),
            vec![cpc]
        );
        assert_eq!(
            targets("l'article 150 du code des impôts de la Polynésie française"),
            vec![Some("FR-PF/CODE-IMPOTS".to_string())]
        );
        assert_eq!(
            targets("le code des douanes de Mayotte"),
            vec![Some("LEGI_DOU_YT".to_string())]
        );
    }

    #[test]
    fn same_code_and_bare_700_stop_at_a_territorial_code() {
        assert_eq!(
            targets(
                "vu le code civil ; vu le code de procédure civile de la Polynésie \
                 française ; l'article 1240 du même code"
            ),
            vec![Some("LEGI_CC".to_string()), None, None]
        );
        assert_eq!(
            targets(
                "l'article 700 du code de procédure civile de la Polynésie française. \
                 Condamne X au titre de l'article 700"
            ),
            vec![None, None]
        );
        assert_eq!(
            targets("Condamne X au titre de l'article 700"),
            vec![Some("LEGITEXT000006070716".to_string())]
        );
    }

    #[test]
    fn a_foreign_court_s_article_takes_the_instrument_its_genitive_names() {
        let (traite, aua, aupsrve) = (
            "OHADA/TRAITE-1993-OHADA",
            "OHADA/ACTE-UNIFORME-2017-DROIT-DE-L-ARBITRAGE",
            "OHADA/ACTE-UNIFORME-2023-RECOUVREMENT",
        );
        let texts: Vec<CatalogText> = [
            (traite, "Traité OHADA", "TRAITE"),
            (
                aua,
                "Acte uniforme relatif au droit de l'arbitrage",
                "ACTE_UNIFORME",
            ),
            (
                aupsrve,
                "Acte uniforme portant organisation des procédures simplifiées de recouvrement",
                "ACTE_UNIFORME",
            ),
        ]
        .iter()
        .map(|(uid, title, nature)| {
            let mut t = text(uid, title, title, nature, 300);
            t.jurisdiction = Some("OHADA".to_string());
            t
        })
        .collect();
        let articles = [(traite, "1"), (traite, "14"), (aua, "12"), (aupsrve, "14")]
            .iter()
            .map(|(uid, n)| (uid.to_string(), n.to_string()))
            .collect();
        let snap = LinkSnapshot::build(texts.clone(), articles);
        let vocab = CompiledVocab::build(&texts, &snap);
        let linked = |cite: &str| -> Vec<(String, Option<String>)> {
            let chars: Vec<char> = cite.chars().collect();
            extract_citations(cite, &vocab, &snap, Forum::of(None, None, Some("CM"), None))
                .iter()
                .filter(|c| c.article.is_some())
                .map(|c| {
                    (
                        chars[c.char_start..c.char_end].iter().collect(),
                        c.target.ref_text_uid.clone(),
                    )
                })
                .collect()
        };
        let none = |n: &str| (n.to_string(), None);
        assert_eq!(
            linked(
                "l'article 12 de l'Acte uniforme relatif au droit de l'arbitrage ; renvoyons \
                 les parties à l'exécution de l'article 12 de leur convention"
            )[1],
            none("12")
        );
        assert_eq!(
            linked(
                "l'article 1 du Traité OHADA ; les protagonistes ont prévu à l'article 23 (1) \
                 des statuts de la société une clause"
            )[1..],
            [none("23")]
        );
        let recovery = "l'article 14 de l'Acte uniforme portant organisation des procédures \
                        simplifiées de recouvrement ; aux termes de l'article 14, alinéas 3 et 4 \
                        du Traité susvisé";
        assert_eq!(linked(recovery)[1], none("14"));
        assert_eq!(
            linked(&format!("l'article 1 du Traité OHADA ; {recovery}"))[2],
            ("14".to_string(), Some(traite.to_string()))
        );
    }
}
