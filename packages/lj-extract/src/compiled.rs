//! Moteur d'extraction des citations (ADR 0158, remplace le volet 1 de
//! l'ADR 0156) : l'automate ne porte plus le catalogue — seulement les FORMES
//! structurelles de citation (ancres de nature « code/loi/convention… »,
//! « article(s) », connecteurs d'anaphore) et les alias embarqués. Chaque
//! ancre positionne un LEXER sur un petit span qui borne la mention ; le span
//! est ensuite SNAPPÉ sur le catalogue (titres exacts pliés, index du linker)
//! et le lien est délégué à [`link_citation`] (Voie B, alias, traités,
//! gazetteer : la même autorité que toujours).
//!
//! Deux phases strictement séparées (architecture « KV-cache ») :
//!
//! 1. **scan** — automate Aho-Corasick leftmost-longest (petit, statique) sur
//!    texte plié longueur-stable + lexers positionnés par classe d'ancre +
//!    lexer d'énumérations d'articles → [`Tok`]s triés par position.
//! 2. **compose** — rejoue le flux sans relire le texte : rattachement
//!    article→instrument (énumérations traversantes), anaphores (« du même
//!    code », « dudit règlement »), antécédent validé par existence de
//!    l'article au catalogue, résolution des articles nus par UNICITÉ
//!    (index inversé `num_key` → textes ∩ instruments du document).
//!
//! PUR (règle #1) : opère sur un [`LinkSnapshot`] et des [`CatalogText`]
//! hydratés par l'appelant. Chiffré au banc unifié (`lj-bench extract
//! --engine compiled`).

use std::collections::BTreeMap;
use std::sync::{LazyLock, OnceLock};

use regex::Regex;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::data::instrument_aliases;
use crate::data::LINK_ALIASES_TSV;
use crate::extract::common::FOREIGN_NATIONALITY_STEMS;
use crate::extract::key_signals::Citability;
use crate::extract::{normalize_article, normalize_instrument};
use crate::link::{link_citation_analyzed, KeyAnalysis};
use crate::link::{CatalogText, LinkSnapshot, LinkTarget};

// ── pliage longueur-stable ──────────────────────────────────────────────────
//
// 1 char → 1 char, offsets préservés — le pliage général (`lj_core::text::
// fold`) normalise l'espace et casse les positions. Couvre le français des
// titres et décisions ; tout char inconnu passe en minuscule simple.

fn fold_char(c: char) -> char {
    // Voie rapide ASCII (l'écrasante majorité des chars d'une décision) : la
    // casse ASCII évite la table Unicode de `to_lowercase`.
    if c.is_ascii() {
        return match c {
            '\n' | '\r' | '\t' => ' ',
            _ => c.to_ascii_lowercase(),
        };
    }
    match c {
        'À' | 'Â' | 'Ä' | 'à' | 'â' | 'ä' => 'a',
        'É' | 'È' | 'Ê' | 'Ë' | 'é' | 'è' | 'ê' | 'ë' => 'e',
        'Î' | 'Ï' | 'î' | 'ï' => 'i',
        'Ô' | 'Ö' | 'ô' | 'ö' => 'o',
        'Ù' | 'Û' | 'Ü' | 'ù' | 'û' | 'ü' => 'u',
        'Ç' | 'ç' => 'c',
        'Œ' | 'œ' => 'o', // « œ » 1:1 (≠ NFKD « oe ») : la stabilité prime
        '’' => '\'',
        // Blancs → espace simple : les motifs portent des espaces, le texte
        // des sauts de ligne (« accord franco-tunisien du\n17 mars 1988 »).
        '\n' | '\r' | '\t' | '\u{a0}' | '\u{2007}' | '\u{2009}' | '\u{202f}' => ' ',
        _ => c.to_lowercase().next().unwrap_or(c),
    }
}

pub(crate) fn fold_stable(s: &str) -> String {
    s.chars().map(fold_char).collect()
}

/// `fold_stable(s).starts_with(prefix)` sans allouer — `prefix` déjà plié.
/// Boucles chaudes du compose (anaphores : nature × antécédents).
fn starts_with_folded(s: &str, prefix: &str) -> bool {
    let mut it = s.chars().map(fold_char);
    prefix.chars().all(|pc| it.next() == Some(pc))
}

/// Pliage canonique de comparaison : `fold_stable` + espaces réduits à un.
/// C'est la forme des clés de `full_titles`/`code_titles` ET des candidats du
/// walker (mots re-joints par espace simple) — les titres multi-lignes du
/// texte s'apparient donc quel que soit leur blanc d'origine.
fn canon_fold(s: &str) -> String {
    fold_stable(s)
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
    /// Citation de jurisprudence (ADR 0165) : juridiction, « pourvoi »,
    /// « affaire », « RG »… — le lexer borne le span au token identifiant et
    /// dérive la clé pendante par famille. Flux séparé (`CompiledCase`),
    /// jamais composé avec les instruments.
    Case,
}

#[derive(Clone, Copy)]
enum Pat {
    /// Alias embarqué (link_aliases.tsv, instrument_aliases) : la surface
    /// pliée EST la clé de linking. `sigle` = surface courte (≤ 6 chars)
    /// exigée TOUT EN MAJUSCULES dans l'original (« du CPC » oui, « le cpc »
    /// prose non).
    Alias {
        is_code: bool,
        sigle: bool,
    },
    Anchor(Anchor),
}

/// Rôle(s) d'un motif du flux unifié (ADR 0160) : une surface peut être à la
/// fois ancre/alias citations ET marqueur structurel — LE MÊME token porte
/// les deux rôles, chaque composeur filtre les siens. `mk_len` > 0 quand le
/// rôle marqueur vient de la clôture par préfixe : la surface du marqueur
/// est le préfixe de cette longueur (en octets pliés), et le gate
/// (`marker_token`) s'applique à SES frontières — un alias OCR tronqué en
/// plein mot (« code de l'entree et du sejour des etr ») ne doit pas tuer
/// le signal porté par son préfixe sain.
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
    // Citations de jurisprudence (ADR 0165). « cedh » reste un ALIAS (la
    // Convention) : la cour se désambiguïse dans le bras Alias par le n° de
    // requête aval. « arret » porte aussi le rôle marqueur `Mk::Stop`
    // (fusion des rôles dans `fused()`, comme « ordonnance »).
    ("pourvoi", Anchor::Case),
    ("pourvois", Anchor::Case),
    ("cass.", Anchor::Case),
    ("cass", Anchor::Case),
    // Graphies collées « Cass.com., 14 oct. 2014 » : la frontière droite
    // alphanumérique de l'ancre « cass. » les rejetterait.
    ("cass.com.", Anchor::Case),
    ("cass.civ.", Anchor::Case),
    ("cass.soc.", Anchor::Case),
    ("cass.crim.", Anchor::Case),
    ("cassation", Anchor::Case),
    ("cour de cassation", Anchor::Case),
    // Chambres de la Cour de cassation citées seules (« Civ. 1ère, 27 février
    // 2013, n° 11-25536 », « Soc. 21 septembre 2011 ») : gate MAJUSCULE
    // initiale dans `lex_case` (la prose minuscule — « c. com. », le code —
    // ne sonde pas), la forme du pourvoi discrimine ensuite.
    ("civ.", Anchor::Case),
    ("civ", Anchor::Case),
    ("soc.", Anchor::Case),
    ("soc", Anchor::Case),
    ("com.", Anchor::Case),
    ("crim.", Anchor::Case),
    ("ass. plen.", Anchor::Case),
    ("conseil d'etat", Anchor::Case),
    // « CE Ass., 12 janv. 1968, n° 70951 » : gate MAJUSCULES (sinon pronom).
    ("ce", Anchor::Case),
    ("conseil constitutionnel", Anchor::Case),
    ("cons. const.", Anchor::Case),
    ("cour europeenne des droits de l'homme", Anchor::Case),
    ("cjue", Anchor::Case),
    ("cjce", Anchor::Case),
    // « TUE, 6 juillet 2022, T-250/21 » — gate 3 MAJUSCULES dans lex_case
    // (« tué » plié donne la même surface).
    ("tue", Anchor::Case),
    // « la Cour de justice (23 décembre 2009, …, C-45/08) » — leftmost-longest
    // préfère les formes longues UE ; la CJR ne déclenche rien (préfixe de
    // rôle C-/T-/F- exigé en fenêtre).
    ("cour de justice", Anchor::Case),
    ("cour de justice de l'union europeenne", Anchor::Case),
    // Graphie OCR fréquente : « 1' » pour « l' ».
    ("cour de justice de 1'union europeenne", Anchor::Case),
    ("cour de justice des communautes europeennes", Anchor::Case),
    ("affaire", Anchor::Case),
    ("affaires", Anchor::Case),
    ("aff.", Anchor::Case),
    ("arret", Anchor::Case),
    ("arrets", Anchor::Case),
    // Fond administratif (chaîne procédurale, ADR 0165 amendé) : « Par un
    // jugement n° 1901563 du 15 février 2023, le tribunal administratif de
    // Nantes… » — la juridiction se sonde en amont ET en aval du numéro.
    ("jugement", Anchor::Case),
    ("jugements", Anchor::Case),
    ("requete", Anchor::Case),
    ("requetes", Anchor::Case),
    ("rg", Anchor::Case),
    // Graphies du sigle RG et formes épelées (« R.G. 2013F00251 »,
    // « Rôle N° 16/03071 », « enrôlée au répertoire général sous le
    // n° 18/00064 ») : gate MAJUSCULE pour les sigles/en-têtes.
    ("r.g.", Anchor::Case),
    ("r.g", Anchor::Case),
    ("r. g", Anchor::Case),
    ("role", Anchor::Case),
    ("repertoire general", Anchor::Case),
    // « CA Paris, 14 nov. 2019, n° 18/04366 » — MAJUSCULES exigées (« ça »,
    // chiffre d'affaires) + marqueur n° et slashnum en sonde.
    ("ca", Anchor::Case),
    // « DÉCISION DÉFÉRÉE : 21/00287 » (bandeau CA sans « RG »).
    ("deferee", Anchor::Case),
    // « décision attaquée …, enregistrée sous le no 20/00325 », « procédures
    // enrôlées sous les numéros 18/02640 et 18/02641 ».
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
    /// Cache INTER-décisions de `link_citation_analyzed` — fonction pure de
    /// `(instrument, text_key, article_key)` sur le snapshot du run (le vocab
    /// est construit par snapshot, le cache ne peut pas fuir entre runs). Les
    /// mêmes clés se répètent massivement dans le corpus (« code civil »,
    /// L. 761-1 CJA…) : le linking devient un hit de table après chauffe.
    link_cache: std::sync::RwLock<FxHashMap<LinkCacheKey, LinkTarget>>,
}

/// Clé du cache de link inter-décisions : `(instrument, text_key, article_key)`.
type LinkCacheKey = (String, String, Option<String>);

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

/// Surfaces jamais citées comme textes : « Conseil d'Etat » = la juridiction
/// (l'entrée JORF homonyme du catalogue est du bruit), « accord collectif » =
/// l'accord d'espèce d'une entreprise, « loi du pays » nue = la catégorie
/// d'acte (ou le DIP : « la loi du pays avec lequel… »).
const BLOCKED_SURFACES: &[&str] = &["conseil d'etat", "accord collectif", "loi du pays"];

/// Articles au sens jurisprudentiel univoque, cités nus dans toutes les
/// juridictions judiciaires (« condamne X au titre de l'article 700 ») :
/// frais irrépétibles et distraction des dépens du Code de procédure civile.
const CANON_BARE_ARTICLES: &[(&str, &str)] = &[
    ("700", "LEGITEXT000006070716"),
    ("699", "LEGITEXT000006070716"),
];

/// Automate FUSIONNÉ (statique, un par process) : ancres + alias embarqués
/// (citations) + marqueurs structurels (`crate::scan`). UN flux de tokens à
/// discipline de consommation unique (ADR 0160) : une passe leftmost-longest
/// non chevauchante, un motif = un token porteur de son (ses) rôle(s) —
/// les surfaces communes aux deux sous-systèmes portent les deux rôles.
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
                // Surface commune (autre sous-système ou doublon marqueur) :
                // le même token porte les deux rôles, premier arrivé gagne.
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
        // Clôture de rôles par préfixe : un motif citations qu'un marqueur
        // préfixe à la frontière de mot (alias CESEDA ⊃ marqueur ProcImmig
        // « code de l'entrée et du séjour ») porte AUSSI ce rôle marqueur —
        // le leftmost-longest ne fait pas disparaître un signal dont la
        // surface est présente. Le gate (`marker_token`) s'applique au token
        // entier, en aval.
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
            // Vocabulaire petit et statique : le DFA (~8 ns/octet) est
            // toujours le bon choix — l'heuristique laisserait un NFA.
            .kind(Some(aho_corasick::AhoCorasickKind::DFA))
            .build(&patterns)
            .expect("automate fusionné marqueurs + citations");
        Fused { ac, roles }
    })
}

impl CompiledVocab {
    /// Bâtit les index de snap catalogue (titres, articles) — l'automate est
    /// STATIQUE ([`fused`]), plus aucune donnée catalogue dedans (ADR 0158).
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
        // Triés pour le test d'appartenance par dichotomie : les num_keys nus
        // (« 1 », « 700 ») portent des dizaines de milliers d'uids — la
        // règle 5 intersecte avec les uids du document, jamais l'inverse.
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
    /// structurelle sans identité chiffrée ni snap catalogue : n'émet un span
    /// que résolue. `treaty_short` = forme conventionnelle non datée,
    /// candidate à l'anaphore de préfixe (« la Convention de Vienne » après
    /// la forme longue datée). `nested` = mention imbriquée dans une autre
    /// (« protocole n° 16 À LA CONVENTION … ») : antécédent d'anaphore
    /// valide, jamais de span propre.
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

/// Longueur maximale d'une mention lexée (garde anti-sur-capture, même borne
/// que `MAX_INSTRUMENT_LEN` côté normalisation).
const MAX_SPAN: usize = 250;

/// Mots de prose qui n'apparaissent jamais À L'INTÉRIEUR d'un nom
/// d'instrument cité : bornes du walker (port de la borne droite legacy
/// `citation_rhs`, adaptée au texte plié — « à » ⇒ « a » n'est PAS une borne,
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

/// Mots pendants coupés en fin de span (connecteurs, participes d'apparat) :
/// un nom d'instrument ne finit jamais dessus.
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

/// Mots de nature : un « et » qui les introduit ferme la mention en cours
/// (« la convention … et l'accord franco-tunisien » = DEUX mentions).
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
    // « à » plié en « a » : connecteur légitime (« faite a la haye »), borne
    // seulement en tête d'auxiliaire (« a ete signee »).
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
/// surface — `link_citation` résout en code étranger (règle 8) au lieu de
/// mislinker le code français homonyme. Renvoie la nouvelle fin de span si
/// extension.
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

/// Surface originale d'un span plié (offsets bytes → chars).
fn span_surface(chars: &[char], byte2char: &[usize], bs: usize, be: usize) -> String {
    chars[byte2char[bs]..byte2char[be]].iter().collect()
}

/// `normalize_instrument` mémoïsé par span plié — les mêmes mentions
/// reviennent des dizaines de fois par document.
/// Cache statique de [`normalize_instrument`] : fonction PURE de la surface
/// (chaîne de replaces + regex, chère), aux surfaces massivement répétées dans
/// le corpus (« code civil », « code de procédure civile »…).
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
        if let Some(e2) = gentile_ext(folded, e) {
            // clé recalculée par l'appelant (normalize) : droit étranger.
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
    // Chaîne structurelle : code hors catalogue (« code de la famille
    // congolais ») ou graphie dégradée — la clé snape via `snap_code_name`
    // dans la normalisation, le lien via la règle 8 (codes étrangers).
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
        // le gentilé clôt l'identité (« code pénal iranien réprime… »).
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
    if let Some(m) = RE_LEX_DATED_QUAL.find(&folded[pos..]) {
        pos += m.end();
    }
    let mut has_id = false;
    if let Some(m) = RE_LEX_NUM.find(&folded[pos..]) {
        pos += m.end();
        has_id = true;
    }
    if let Some(m) = RE_LEX_DATE.find(&folded[pos..]) {
        pos += m.end();
        has_id = true;
    }
    // Extension au titre épelé verbatim : seulement derrière une identité
    // chiffrée — tout titre catalogue d'acte daté porte n°/date, une mention
    // nue (« la loi permet… ») ne peut pas matcher, on s'épargne le walk.
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

/// Droit dérivé UE : le numéro à barre oblique est l'identité. Sans lui, la
/// mention nue génitive suivie de ponctuation est une anaphore (« les
/// dispositions du règlement, »).
enum EuLex {
    Instr(Mention),
    Same(usize),
    None,
}

fn lex_eu(vocab: &CompiledVocab, folded: &str, bs: usize, be: usize) -> EuLex {
    if let Some(m) = RE_LEX_EU.find(&folded[be..]) {
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
    // Anaphore nue : « du règlement, » / « de la directive ; ».
    let word = &folded[bs..be];
    let head = folded[..bs].trim_end();
    let bare_conn = (word == "reglement" && head.ends_with(" du"))
        || (word == "directive" && head.ends_with(" de la"));
    if bare_conn
        && folded[be..]
            .trim_start()
            .starts_with([',', '.', ';', ':', ')'])
    {
        // position du connecteur (« du » / « de la »).
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
        // « … et l'accord franco-tunisien » / « … et l'article 12 » /
        // « … et 12 » : une nouvelle mention ou une énumération commence.
        let bare = w.trim_start_matches("l'").trim_start_matches("d'");
        if matches!(prev, "et" | "ou")
            && (NATURE_WORDS.contains(&bare)
                || matches!(bare, "article" | "articles")
                || bare.starts_with(|c: char| c.is_ascii_digit()))
        {
            break; // l'« et » traînant tombe au trim
        }
        // « la déclaration annuelle prévue à l'article 97 du CGI » : un mot
        // d'article en pleine marche = la mention est finie (les 7 titres
        // catalogue qui en portent un ne sont jamais cités par titre épelé) —
        // sans quoi le numéral avalé rend « citable » une mention de prose et
        // son span absorbe l'ancre de l'instrument légitime.
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
        return None; // mention nue (« la convention ») : générique.
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

// ── citations de jurisprudence (ADR 0165) ───────────────────────────────────

/// Citation de jurisprudence : span borné au token identifiant (« n°
/// [18-23.954] », « [C-561/19] », « RG n° [21/04532] », convention ADR 0143)
/// et clé pendante par famille, alignée sur la grammaire `identity.rs` mais
/// SANS date obligatoire — `cc|1823954`, `ce|412412`, `constit|2020-800`,
/// `cjue|c-561/19`, `cedh|30010/10`, `rg|{jurisdiction_code}|21/04532`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledCase {
    pub char_start: usize,
    pub char_end: usize,
    pub target_ref: String,
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
    target_ref: String,
) {
    out.push(CompiledCase {
        char_start: byte2char[abs_s],
        char_end: byte2char[abs_e],
        target_ref,
    });
}

/// Lexer positionné des citations de jurisprudence : famille par surface
/// d'ancre, 0..n spans émis (énumérations « pourvois n° X et Y »). Flux
/// séparé du flux instruments — aucune interaction avec `compose()`.
fn lex_case(
    folded: &str,
    chars: &[char],
    byte2char: &[usize],
    chrono: Option<&crate::chrono::ChronoSnapshot>,
    bs: usize,
    be: usize,
    out: &mut Vec<CompiledCase>,
) {
    // Gate MAJUSCULE initiale sur l'original : sigles et chambres seulement
    // (« Civ. 1ère » cite, « la civilisation » non ; « CE » cite, « ce » non).
    let upper = |n: usize| {
        let ci = byte2char[bs];
        chars[ci..].iter().take(n).all(|c| c.is_uppercase())
    };
    match &folded[bs..be] {
        // « pourvoi n° 18-23.954 » : le n° suit l'ancre.
        "pourvoi" | "pourvois" => case_cc(folded, byte2char, be, out, true),
        // « Cass. 3e civ., 16 mars 2022, n° 18-23.954 », « sur renvoi après
        // cassation par arrêt en date du 29 juin 2017 (16-13.988) » : chambre
        // et date s'intercalent — sonde de fenêtre, la forme du pourvoi
        // discrimine.
        // « Cass Com 22/05/2013 P no11-24812 », « cass soc 2 mars 2011
        // n°08-44977 » : « cass » sans point n'est pas un mot français, pas
        // de gate de casse.
        "cass." | "cass" | "cassation" | "cour de cassation" | "cass.com." | "cass.civ."
        | "cass.soc." | "cass.crim." => case_cc(folded, byte2char, be, out, false),
        // Chambre citée seule : « Civ. 1ère, 27 février 2013, n° 11-25536 »,
        // « Soc. 21 septembre 2011, n° 10-15.011 » — MAJUSCULE exigée
        // (« c. com. » est le code de commerce, pas la chambre commerciale).
        "civ." | "civ" | "soc." | "soc" | "com." | "crim." | "ass. plen." => {
            if upper(1) {
                case_cc(folded, byte2char, be, out, false);
            }
        }
        "conseil d'etat" => case_ce(folded, byte2char, bs, be, out),
        // « CE Ass., 12 janv. 1968, n° 70951 » : abréviation en MAJUSCULES.
        "ce" => {
            if upper(2) {
                case_ce(folded, byte2char, bs, be, out);
            }
        }
        "conseil constitutionnel" => case_constit(folded, byte2char, bs, be, out, false),
        "cons. const." => case_constit(folded, byte2char, bs, be, out, false),
        "cour europeenne des droits de l'homme" => {
            case_cedh(folded, byte2char, be, out, false);
        }
        // « requête n° 30010/10 » : le format à barre oblique est CEDH — les
        // requêtes admin (7 chiffres nus) ne matchent pas. « La requête en
        // référé n° 502527 » est un référé CE.
        "requete" | "requetes" => {
            if !case_cedh(folded, byte2char, be, out, true) {
                let win = case_window(folded, be, 40);
                if let Some(c) = RE_CASE_REF_CE.captures(win) {
                    let g = c.get(1).expect("groupe n° CE");
                    if !cut_by_window(folded, be, win, g.end()) {
                        let key = format!("ce|{}", g.as_str());
                        push_case(out, byte2char, be + g.start(), be + g.end(), key);
                    }
                }
            }
        }
        // « aff. C-561/19 », « affaire 6/64 » (slashnum nu toléré derrière
        // l'ancre affaire uniquement), « dans l'affaire 26604/16 Waldner »
        // (n° de requête CEDH, 4-5 chiffres avant la barre).
        "affaire" | "affaires" | "aff." => case_cjue_aff(folded, chars, byte2char, be, out),
        // « l'arrêt de la CJUE du 6 octobre 2021, C-561/19 » : le préfixe
        // C-/T-/F- est exigé en fenêtre (un slashnum nu serait un acte UE).
        "cjue"
        | "cjce"
        | "cour de justice"
        | "cour de justice de l'union europeenne"
        | "cour de justice de 1'union europeenne"
        | "cour de justice des communautes europeennes" => {
            case_cjue_window(folded, chars, byte2char, be, out);
        }
        // « TUE, 6 juillet 2022, T-250/21 » : MAJUSCULES exigées (« tué »
        // plié donne la même surface).
        "tue" => {
            if upper(3) {
                case_cjue_window(folded, chars, byte2char, be, out);
            }
        }
        // « arrêt » sonde toutes les cours qu'il peut introduire : CJUE
        // (préfixe exigé), CC (« arrêts du 18 février 2015 n°13-27104 »),
        // CEDH (« son arrêt n° 29217/12, Tarakhel » — 4-5 chiffres avant la
        // barre, disjoint des RG à préfixe court), CAA (« Par un arrêt
        // n° 20NT01234 …, la cour administrative d'appel de Nantes »). Les
        // formes sont mutuellement exclusives.
        "arret" | "arrets" => {
            case_cjue_window(folded, chars, byte2char, be, out);
            case_cc(folded, byte2char, be, out, false);
            case_cedh_arret(folded, chars, byte2char, be, out);
            case_admin(folded, chars, byte2char, chrono, bs, be, out);
            case_ce_arret(folded, byte2char, bs, be, out);
            case_arret_ord(folded, chars, byte2char, be, out);
        }
        // Chaîne procédurale du fond administratif : « Par un jugement
        // n° 1901563 du 15 février 2023, le tribunal administratif de
        // Nantes… » (ADR 0165 amendé — la chaîne est incluse).
        "jugement" | "jugements" => {
            case_admin(folded, chars, byte2char, chrono, bs, be, out);
        }
        // « rg » n'est pas un mot français : pas de gate de casse (les
        // bandeaux minuscules « rg no 13/00109 » citent aussi).
        "rg" | "r.g." | "r.g" | "r. g" => {
            case_rg(folded, chars, byte2char, chrono, bs, be, out);
        }
        // « Rôle N° 16/03071 » (bandeau CA) : MAJUSCULE exigée (sinon prose).
        "role" => {
            if upper(1) {
                case_rg(folded, chars, byte2char, chrono, bs, be, out);
            }
        }
        // « enrôlée au répertoire général sous le n° 18/00064 » : forme
        // épelée, minuscule légitime.
        "repertoire general" => case_rg(folded, chars, byte2char, chrono, bs, be, out),
        // « CA [Localité 8], 14 nov. 2019, n°18/04366 » : MAJUSCULES exigées
        // (« ça », chiffre d'affaires).
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

/// Clé pourvoi : chiffres seuls (« 18-23.954 » → `cc|1823954`), même
/// normalisation que la sonde de résolution (93,7 % unique au corpus).
fn cc_key(num: &str) -> String {
    let digits: String = num.chars().filter(char::is_ascii_digit).collect();
    format!("cc|{digits}")
}

fn case_cc(
    folded: &str,
    byte2char: &[usize],
    be: usize,
    out: &mut Vec<CompiledCase>,
    anchored: bool,
) {
    // Fenêtre longue : les énumérations à dates (« n 01-17122 ; 27 mars 2007,
    // n 05-14491 ; … ») dépassent 100 octets — la forme stricte du pourvoi
    // rend la distance sûre.
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
        // Énumération : « pourvois n° 18-23.954 et 18-23.955 », plages
        // « n° T 00-44.843 au n° W 00-44.846 ».
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
    // Repli CE : le pourvoi ADMINISTRATIF porte un n° de requête 5-6 chiffres
    // (« statuant sur les pourvois n° 476000 … et n° 476009 », « pourvu en
    // cassation sous le n° 500109 ») — collé à l'ancre pour ne pas ramasser
    // un n° lointain de la fenêtre.
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
        format!("ce|{}", g.as_str()),
    );
    let mut pos = g.end();
    while let Some(c) = RE_CASE_CE_NEXT.captures(&win[pos..]) {
        let g = c.get(1).expect("groupe n° CE");
        if cut_by_window(folded, off, win, pos + g.end()) {
            break;
        }
        let (s, e) = (off + pos + g.start(), off + pos + g.end());
        push_case(out, byte2char, s, e, format!("ce|{}", g.as_str()));
        pos += g.end();
    }
}

fn case_ce(folded: &str, byte2char: &[usize], bs: usize, be: usize, out: &mut Vec<CompiledCase>) {
    // Sonde ARRIÈRE : « Par une décision n° 432537 du 8 janvier 2020, le
    // Conseil d'Etat… » — dernière mention décision/ordonnance/arrêt n° en
    // amont, invalidée si une AUTRE juridiction s'intercale.
    let mut ws = bs.saturating_sub(160);
    while !folded.is_char_boundary(ws) {
        ws += 1;
    }
    let up = &folded[ws..bs];
    if let Some(c) = RE_CASE_CE_BACK.captures_iter(up).last() {
        let g = c.get(1).expect("groupe n° CE");
        // Énumération vers l'avant DANS l'amont (« nos 431188, 431348, … »).
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
                let key = format!("ce|{}", &up[s..e]);
                push_case(out, byte2char, ws + s, ws + e, key);
            }
        }
    }
    // Fenêtre AVANT : « Conseil d'État … n° 70951 », « sous le numéro 470194 ».
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
    // Sonde ARRIÈRE : « la décision n°2021-823 du Conseil constitutionnel » —
    // gatée « décision », collée à l'ancre.
    let mut ws = bs.saturating_sub(80);
    while !folded.is_char_boundary(ws) {
        ws += 1;
    }
    if let Some(c) = RE_CASE_CONSTIT_BACK.captures(&folded[ws..bs]) {
        let g = c.get(1).expect("groupe n° constit");
        // Clé sans le suffixe DC/QPC (les `docket_numbers` CONSTIT sont nus).
        let key = format!("constit|{}", g.as_str());
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
        format!("constit|{}", g.as_str()),
    );
    // Énumération : « n° 2016-554 QPC du 22 juillet 2016, n° 2016-610 QPC ».
    let mut pos = g.end();
    while let Some(c) = RE_CASE_CONSTIT_NEXT.captures(&win[pos..]) {
        let g = c.get(1).expect("groupe n° constit");
        let (s, e) = (be + pos + g.start(), be + pos + g.end());
        push_case(out, byte2char, s, e, format!("constit|{}", g.as_str()));
        pos += g.end();
    }
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
    // Énumération : « n°15670/18 et 43115/18 M. A et autres c/ Croatie ».
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
    push_case(out, byte2char, s, e, format!("cedh|{num}"));
}

/// « son arrêt n° 29217/12, Tarakhel c./ Suisse » : requête CEDH derrière
/// « arrêt » — 4-5 chiffres avant la barre, n° exigé. Un premier groupe
/// année (« ARRÊT AU FOND DU 25 MARS 2011 N° 2011/183 ») est l'ordinal d'un
/// arrêt CA : clé nue `rg||`. Un « règlement/directive n° 2988/95 » dans la
/// fenêtre n'est pas une requête.
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
            format!("rg||{num}"),
        );
    } else {
        push_cedh(out, byte2char, be + g.start(), be + g.end(), g.as_str());
    }
}

/// Clé CJUE : préfixe de rôle conservé quand présent (`cjue|c-561/19`), nu
/// sinon (« aff. 6/64 » → `cjue|6/64` — la résolution essaie le préfixe C-).
/// Le séparateur graphié tiret (« C-631-13 ») est ramené à la barre oblique.
fn cjue_key(prefix: Option<&str>, num: &str) -> String {
    let num: String = num
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| if c == '-' { '/' } else { c })
        .collect();
    match prefix {
        Some(p) => format!("cjue|{p}-{num}"),
        None => format!("cjue|{num}"),
    }
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
        // « l'affaire déjà citée C-210/13 Glaxosmithkline » : le n° n'est pas
        // collé à l'ancre — la sonde de fenêtre (préfixe exigé) prend le relais.
        cjue_scan(folded, chars, byte2char, be, win, 0, out);
        return;
    };
    let n = c.get(2).expect("groupe slashnum");
    let (first, second) = n.as_str().split_once('/').expect("slashnum");
    let (s, e, key) = match c.get(1) {
        Some(p) => (p.start(), n.end(), cjue_key(Some(p.as_str()), n.as_str())),
        // Slashnum nu : 4-5 chiffres avant la barre = requête CEDH
        // (« l'affaire 26604/16 Waldner c. France »), sinon rôle CJCE nu.
        None if first.trim().len() >= 4 => {
            push_cedh(out, byte2char, be + n.start(), be + n.end(), n.as_str());
            cjue_scan(folded, chars, byte2char, be, win, n.end(), out);
            return;
        }
        // Second membre à 5+ chiffres (« l'affaire 09/ 01106 ») : un rôle
        // judiciaire, pas une affaire UE — clé nue `rg||`.
        None if second.trim().len() >= 5 => {
            let num = rg_num_key(chars, byte2char, be + n.start(), be + n.end());
            push_case(
                out,
                byte2char,
                be + n.start(),
                be + n.end(),
                format!("rg||{num}"),
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

/// Balaye la fenêtre : numéros PRÉFIXÉS où qu'ils soient (noms d'affaires et
/// dates s'intercalent — « C-166/13 Sophie Mukarubega du 5 novembre 2014 et
/// C-249/13 »), plage à membre nu héritant du préfixe (« C-338/11 à
/// 347/11 »). Le préfixe séparé par espace seul exige la MAJUSCULE
/// (« C 434/15 » cite, une lettre de prose non).
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
        // « Pourvoi n° T 17-21.405 » : la lettre de série + le début du n° de
        // pourvoi miment un rôle CJUE à tiret — la queue « .405 » trahit.
        if pourvoi_tail(folded, off + pos + n.end()) {
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
        // « C-338/11 à 347/11 », « C-131/13, 163/13 et 164/13 » : membres nus
        // d'énumération, préfixe hérité.
        while let Some(c) = RE_CASE_CJUE_RANGE.captures(&win[pos..]) {
            let n = c.get(1).expect("slashnum");
            if pourvoi_tail(folded, off + pos + n.end()) {
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

/// Un « .NNN » collé derrière un slashnum est la fin d'un n° de pourvoi
/// Cassation (« T 17-21.405 ») — pas un rôle CJUE.
fn pourvoi_tail(folded: &str, abs_e: usize) -> bool {
    let rest = &folded.as_bytes()[abs_e..];
    rest.first() == Some(&b'.') && rest.get(1).is_some_and(u8::is_ascii_digit)
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

/// Code référentiel de la juridiction au voisinage d'un RG : DERNIÈRE forme
/// en amont, sinon PREMIÈRE en aval. `want` restreint au type attendu
/// (jugement 7 chiffres = TA, arrêt AAXX99999 = CAA). Sans `want`, la forme
/// la plus proche DÉCIDE : si sa ville ne mappe pas (CPH avant septembre,
/// ville anonymisée), pas de code — jamais la forme suivante (règle #12 :
/// pas de clé bancale).
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
        // Ville = plus long préfixe de mots qui mappe au référentiel (les
        // noms composés d'abord : « aix en provence » avant « aix »).
        let region = crate::identity::normalize_component(&folded[off + g.start()..off + g.end()]);
        let words: Vec<&str> = region.split_whitespace().collect();
        for k in (1..=words.len().min(6)).rev() {
            let city = words[..k].join(" ");
            if let Some(loc) = chrono.location(jt, &city) {
                return Some(if jt == "TCOM" {
                    format!("tcom{loc}")
                } else {
                    loc.to_string()
                });
            }
        }
        // Sans `want`, la forme la plus proche décide, mappée ou pas.
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
/// Sans juridiction, clé nue `rg||NUM` — jamais résolue (un RG nu vaut 5,87
/// cibles au corpus), mais décorée quand la GT ou une juridiction future la
/// résorbe.
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
    // « N° RG 22/01114 - N° Portalis DBVF… » : bandeau des dossiers JOINTS de
    // la décision elle-même (absents des métadonnées, donc hors filtre
    // propre-en-tête du pont).
    if case_window(folded, be + g.end(), 60).contains("portalis") {
        return;
    }
    // « Vu la requête … enregistrée sous le N°RG 25/04461 » : le n° de la
    // requête EN COURS (JLD rétention), pas une citation.
    let ws = upstream(folded, bs, 160);
    if RE_CASE_RG_OWN.is_match(&folded[ws..bs]) {
        return;
    }
    let down_end = case_window(folded, be + g.end(), 160).len() + be + g.end();
    let code = rg_jurisdiction(folded, chrono, (ws, bs), (be + g.end(), down_end), None)
        .unwrap_or_default();
    let num = rg_canon(rg_num_key(chars, byte2char, be + g.start(), be + g.end()));
    let fam = fond_family(&code);
    push_case(
        out,
        byte2char,
        be + g.start(),
        be + g.end(),
        format!("{fam}|{code}|{num}"),
    );
    // Énumération : « n° RG 15/10319, 15/10407, 15/10488 et 15/10492 ».
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
            format!("{fam}|{code}|{num}"),
        );
        pos += g.end();
    }
}

/// Famille de la clé d'un jugement du fond selon le `jurisdiction_code` : `af`
/// (fond administratif TA/CAA, ADR 0165 [af]) pour `ta_*`/`caa*`, `rg` (fond
/// judiciaire) sinon — dont code vide → `rg||NUM` nue (jamais résolue). Le
/// sigle `tass_` (judiciaire) ne matche pas `ta_`.
fn fond_family(code: &str) -> &'static str {
    if code.starts_with("ta_") || code.starts_with("caa") {
        "af"
    } else {
        "rg"
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
        format!("rg||{num}"),
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
            format!("ce|{}", g.as_str()),
        );
    }
}

/// Chaîne procédurale du fond administratif : « Par un jugement n° 1901563 du
/// 15 février 2023, le tribunal administratif de Nantes… » (clé
/// `af|ta_nantes|1901563`, ADR 0165 [af]), « Par un arrêt n° 20NT01234 …, la
/// cour administrative d'appel de Nantes » (`af|caa_nantes|20NT01234`), « Jugement
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
        let fam = fond_family(&code);
        let num = rg_num_key(chars, byte2char, be + g.start(), be + g.end());
        push_case(
            out,
            byte2char,
            be + g.start(),
            be + g.end(),
            format!("{fam}|{code}|{num}"),
        );
        // « Par deux jugements n° 2202850 et n° 2202851 du 16 mars 2023 ».
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
                format!("{fam}|{code}|{num}"),
            );
            pos += g.end();
        }
        return;
    }
    // « Jugement (N° 14/00048) rendu le … par le tribunal de grande instance
    // de X », « l'arrêt n° 98/795 rendu le 13 mars 2018 par la cour d'appel
    // de Limoges » : forme RG judiciaire, marqueur n° strict. L'ordinal
    // année/n° d'un bandeau (« ARRÊT N°2022/522 ») reste à `case_arret_ord`
    // (clé nue) — un code de juridiction ici serait un mislink.
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
/// (début, nature, fin couverte) — la fin couverte inclut le mot de nature
/// qui suit le connecteur : il appartient à l'anaphore, l'ancre qu'il porte
/// (« code », « loi »…) ne doit pas re-lexer une mention parasite.
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
/// Discipline de consommation UNIQUE (ADR 0160) : un curseur (`tok_pos`),
/// leftmost-longest — un motif = un token porteur de son (ses) rôle(s),
/// chaque composeur filtre les siens. Un match à frontière de mot GAUCHE
/// violée (« a sa » au travers de « l·a SA·S ») n'est PAS un token : il ne
/// consomme rien — c'est de la tokenisation, pas un gate. Les gates de
/// légitimité ([`crate::scan::marker_token`], frontière droite + repli
/// d'ancre) sont des filtres AVAL : ils rejettent le token, jamais la
/// consommation.
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
    // Les tokens sans rôle citations vont AUSSI au dispatcher : un marqueur
    // composite peut porter une ancre en préfixe (« code de l'entrée et du
    // séjour » → ancre « code ») ou en suffixe (« vu l'article » → tête
    // d'énumération) — le dispatcher la re-dérive, la consommation ne bouge pas.
    let mut cit_matches: Vec<(usize, usize, Option<Pat>)> = Vec::new();
    let mut tok_pos = 0usize;
    let mut pos = 0usize;
    while let Some(m) = f.ac.find(&folded[pos..]) {
        let (bs, be) = (pos + m.start(), pos + m.end());
        pos = bs + folded[bs..].chars().next().map_or(1, |c| c.len_utf8());
        if bs < tok_pos
            || (bs != 0
                && bytes[bs].is_ascii_alphanumeric()
                && bytes[bs - 1].is_ascii_alphanumeric())
        {
            continue;
        }
        tok_pos = be;
        let roles = f.roles[m.pattern().as_usize()];
        if let (Some(kind), Some(marks)) = (roles.marker, marks.as_deref_mut()) {
            // Rôle hérité par clôture de préfixe : la surface du marqueur est
            // le préfixe `mk_len` — le gate s'applique à SES frontières.
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

    // Repli d'ancre : le leftmost-longest peut préférer un alias d'OCR dégradé
    // (« code civi » pour « code civil ») qui échoue ensuite à la frontière de
    // mot — l'ancre qui préfixe la position reprend la main.
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
            // Token sans rôle citations (marqueur composite) : re-dériver
            // l'ancre qu'il porte. En suffixe (« vu l'article ») → tête
            // d'énumération ; en préfixe (« code de l'entrée et du séjour »)
            // → l'ancre reprend la main avec SA borne.
            for w in ["articles", "article"] {
                if folded[bs..be].ends_with(w)
                    && (be - w.len() == bs || !bytes[be - w.len() - 1].is_ascii_alphanumeric())
                {
                    artwords.push((be - w.len(), be));
                    break;
                }
            }
            // Ancre Case en SUFFIXE d'un marqueur du scan (« joint les
            // pourvois », « décision déférée ») : le marqueur consomme le
            // token, l'ancre garde son dispatch.
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
                // « CEDH, 3 octobre 2014, req. n° 30010/10 » : le sigle
                // désigne ici la COUR — le n° de requête aval l'atteste ;
                // sinon c'est l'alias de la Convention (token multi-rôles,
                // ADR 0165).
                if &folded[bs..be] == "cedh" && case_cedh(folded, byte2char, be, cases, false) {
                    continue;
                }
                if bs < last_instr_end {
                    // Alias conventionnel imbriqué (« protocole n° 16 à la
                    // convention européenne de sauvegarde… ») : antécédent
                    // d'anaphore sans span propre — miroir du cas ancre.
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
                // Sigle en minuscules : collision possible avec un mot de
                // prose (« tue » le verbe) — mais seulement en deçà de 4
                // chars (« du ceseda », « la cedh » sont univoques).
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
                // Un alias de code (« code de justice », graphie dégradée du
                // TSV) peut masquer le titre complet au leftmost-longest : le
                // snap catalogue reprend la borne exacte quand il va plus loin.
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
                if let Some(e2) = gentile_ext(folded, end) {
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
                    // Le possessif embarque le mot « article(s) » (« et
                    // notamment ses articles L. 821-1 ») : son match a
                    // consommé l'ancre ArtWord interne — rouvrir
                    // l'énumération sur sa fin.
                    if folded[bs..be].ends_with("article") || folded[bs..be].ends_with("articles") {
                        artwords.push((bs, be));
                    }
                }
            }
            Pat::Anchor(class) => {
                if bs < last_instr_end {
                    // Mention conventionnelle IMBRIQUÉE (« protocole n° 16 à
                    // la convention européenne de sauvegarde… ») : la
                    // convention interne est l'antécédent des anaphores « de
                    // la convention » en aval — token sans span propre.
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
                            // « il résulte de l'article R. 411-5 du code
                            // qu'elle… » : le « code » nu génitif est une
                            // anaphore du dernier code cité.
                            let head = folded[..bs].trim_end();
                            toks.push(Tok::Same {
                                s: byte2char[head.len() - 2],
                                nature: folded[bs..be].to_string(),
                            });
                        }
                        lexed
                    }
                    Anchor::Dated => {
                        let mut lexed = lex_dated(vocab, folded, bs, be);
                        if &folded[bs..be] == "ordonnance" {
                            if lexed.is_none() {
                                // « l'ordonnance Glaxosmithkline C-210/13 » :
                                // pas d'instrument daté FR — le préfixe de
                                // rôle CJUE en fenêtre désambiguïse (0165).
                                case_cjue_window(folded, chars, byte2char, be, cases);
                            }
                            // Un n° NU 5-7 chiffres n'est jamais un instrument
                            // (les ordonnances-lois portent un tiret d'année) :
                            // c'est une ordonnance CE (« n° 468345 ») ou TA
                            // (7 chiffres) / judiciaire (RG) via case_admin —
                            // la mention sans titre de lex_dated s'efface.
                            if lexed.as_ref().is_none_or(|m| m.text_key.is_empty()) {
                                let before = cases.len();
                                case_admin(folded, chars, byte2char, chrono, bs, be, cases);
                                let win = case_window(folded, be, 40);
                                if let Some(c) = RE_CASE_ORD_CE.captures(win) {
                                    let g = c.get(1).expect("groupe n° CE");
                                    if !cut_by_window(folded, be, win, g.end()) {
                                        let key = format!("ce|{}", g.as_str());
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
                            // « décision n° 2020-800 DC » : ni slashnum UE ni
                            // anaphore — le suffixe DC/QPC désambiguïse vers
                            // le Conseil constitutionnel (ADR 0165).
                            if &folded[bs..be] == "decision" {
                                case_constit(folded, byte2char, bs, be, cases, true);
                            }
                            None
                        }
                    },
                    Anchor::Treaty => {
                        let lexed = lex_treaty(vocab, folded, bs, be);
                        if lexed.is_none() {
                            // Anaphore nue « …articles 2 et 14 de la
                            // convention, » — miroir de l'anaphore UE nue.
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

    // Titre officiel citant des articles (« arrêté … et avis mentionnés aux
    // articles R. 313-22 … du CESEDA ») : le walker de titre a consommé
    // l'ancre « articles » — une mention dont la surface FINIT sur le mot
    // « article(s) » ré-ouvre une énumération.
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

    // Énumérations d'articles : à partir de chaque « article(s) », consommer
    // les numéraux — l'énumération TRAVERSE les instruments (« articles 329 du
    // CPC et 1382 du Code civil »).
    for &(aws, awe) in &artwords {
        let mut pos = awe;
        while let Some(sep) = RE_SEP.find(&folded[pos..]) {
            let mut numpos = pos + sep.end();
            // Ordinal arabe de subdivision derrière le numéral (« l'article
            // L. 1142-1-1, 1°, du code de la santé publique ») : jamais un
            // article — la marche l'enjambe et repart du séparateur (génitif
            // ou item suivant), comme le paragraphe romain.
            if let Some(m) = RE_ARABIC_PARA.find(&folded[numpos..]) {
                pos = numpos + m.end();
                continue;
            }
            if RE_NUM.captures(&folded[numpos..]).is_none() {
                // Paragraphe romain de subdivision entre le numéral et son
                // génitif (« L. 1142-1, I, du code de la santé publique ») :
                // la marche l'enjambe avant de sonder le génitif.
                if let Some(m) = RE_ROMAN_PARA.find(&folded[numpos..]) {
                    numpos += m.end();
                }
                // « du <instrument> et <numéral> » : saute la mention et
                // reprend derrière si un numéral suit. Même enjambement pour
                // le qualificatif d'état du texte qui embarque un acte daté
                // (« 232, dans leur rédaction antérieure à la loi n° 2004-439
                // du 26 mai 2004 et 893, 894 du Code civil »).
                let probe = &folded[numpos..];
                let jump = ["du ", "de la ", "de l'", "des ", "de l' "]
                    .iter()
                    .find_map(|p| probe.starts_with(p).then_some(p.len()))
                    .map(|off| (numpos + off, numpos + off + 3))
                    .or_else(|| {
                        RE_REDACTION
                            .find(probe)
                            .map(|m| (numpos + m.end(), numpos + m.end() + 3))
                    });
                if let Some((ipos, ilim)) = jump {
                    if let Some((_, &iend)) = (ipos < bytes.len())
                        .then(|| instr_spans.range(ipos..ilim.min(bytes.len())).next())
                        .flatten()
                    {
                        if let Some(sep2) = RE_SEP.find(&folded[iend..]) {
                            let cand = iend + sep2.end();
                            if sep2.end() > 1 && RE_NUM.captures(&folded[cand..]).is_some() {
                                numpos = cand;
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
            // Suffixe-lettre fiscal (« 164 B », « L. 16 A », « 163 quinquies
            // C ») : une lettre seule, MAJUSCULE dans l'original (le plié la
            // met en bas de casse), hors « I » (paragraphe romain), non suivie
            // de « . »/« ' » (sinon c'est le préfixe L./R. de l'article
            // suivant d'une énumération).
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
                // Ordinal latin APRÈS la lettre (« 1394 B bis »,
                // « 1518 A quinquies »).
                if let Some(m) = RE_NUM_ORDINAL.find(&folded[be..]) {
                    be += m.end();
                }
            }
            if be < bytes.len() && bytes[be].is_ascii_alphanumeric() {
                break;
            }
            // « Article 2 : » / « Article 1er – » = dispositif de LA décision ;
            // « (article 2) » = renvoi parenthésé au dispositif d'un jugement.
            let after = folded[be..].trim_start();
            let paren_self_ref = aws > 0 && bytes[aws - 1] == b'(' && after.starts_with(')');
            if after.starts_with(':') || after.starts_with('–') || paren_self_ref {
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
    // Une énumération pontée (« , du premier alinéa de l'article X ») et le
    // mot « article » ponté lexent le MÊME numéral : dédup exacte.
    toks.dedup_by(|a, b| match (a, b) {
        (Tok::Art { s: s1, e: e1, .. }, Tok::Art { s: s2, e: e2, .. }) => s1 == s2 && e1 == e2,
        _ => false,
    });

    // Citations de jurisprudence : deux ancres peuvent sonder le même token
    // (« la Cour de cassation (pourvoi n° 19-12.345) ») — tri par position,
    // première émission gagne, zéro chevauchement (PK (decision_id,
    // char_start) et asserts du store en aval).
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
}

/// Préparation d'un document pour les scans automate : texte original en
/// chars, texte plié (`fold_stable`, 1:1 char-stable) et tables
/// byte(plié) ↔ char(original). Proportionnelle au texte — construite UNE
/// fois par document et partagée entre les automates (vocabulaire citations
/// ici, marqueurs `crate::scan`).
///
/// `chars` mappe chaque blanc (`\n`, `\t`, nbsp…) vers l'espace simple, 1:1
/// comme `fold_stable` : les composeurs testent `' '` uniformément, les
/// tranches verbatim sortent mono-ligne, et les offsets restent ceux du
/// texte d'entrée (aucun collapse, aucun trim).
pub struct Norm {
    pub chars: Vec<char>,
    pub folded: String,
    pub byte2char: Vec<usize>,
    pub char2byte: Vec<usize>,
}

impl Norm {
    /// UNE passe sur le texte : chars mappés, pliage et les deux tables
    /// ensemble. `byte2char` n'est lu qu'aux frontières de chars pliés
    /// (positions de match automate) — les octets de continuation restent 0.
    pub fn new(text: &str) -> Self {
        let n = text.len();
        let mut chars: Vec<char> = Vec::with_capacity(n);
        let mut folded = String::with_capacity(n);
        let mut byte2char: Vec<usize> = Vec::with_capacity(n + 1);
        let mut char2byte: Vec<usize> = Vec::with_capacity(n + 1);
        for (ci, c) in text.chars().enumerate() {
            chars.push(if c.is_whitespace() { ' ' } else { c });
            char2byte.push(folded.len());
            let fc = fold_char(c);
            byte2char.push(ci);
            byte2char.extend(std::iter::repeat_n(0, fc.len_utf8() - 1));
            folded.push(fc);
        }
        byte2char.push(chars.len());
        char2byte.push(folded.len());
        Self {
            chars,
            folded,
            byte2char,
            char2byte,
        }
    }
}

/// Extraction compilée d'un document : scan (une passe) + composition sur le
/// flux de tokens. Sortie triée par position.
pub fn extract_citations(
    text: &str,
    vocab: &CompiledVocab,
    snap: &LinkSnapshot,
) -> Vec<CompiledCitation> {
    extract_citations_norm(&Norm::new(text), vocab, snap)
}

/// Variante à préparation fournie : le pipeline par-document (`DocExtract`)
/// construit un [`Norm`] et le partage entre les moteurs.
pub fn extract_citations_norm(
    norm: &Norm,
    vocab: &CompiledVocab,
    snap: &LinkSnapshot,
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
        vocab,
        snap,
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
/// fusionné → les deux flux de tokens, puis la composition citations. C'est
/// LE chemin du bridge ingest (ADR 0156 étape 11) — les entrées par-champ
/// re-scannent et ne servent qu'aux bancs.
pub struct DocExtract {
    pub scan: crate::scan::DocScan,
    pub citations: Vec<CompiledCitation>,
    /// Citations de jurisprudence (ADR 0165) — flux séparé, jamais composé.
    pub cases: Vec<CompiledCase>,
}

pub fn doc_extract(
    text: &str,
    vocab: &CompiledVocab,
    snap: &LinkSnapshot,
    chrono: &crate::chrono::ChronoSnapshot,
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
        vocab,
        snap,
    );
    DocExtract {
        scan: crate::scan::docscan_from_parts(norm, marks),
        citations,
        cases,
    }
}

/// Le texte entre un numéral d'article et l'instrument suivant est-il un
/// connecteur génitif (« du », « de la », « , du même »…) ? C'est LA condition
/// de légitimité du rattachement-suivant — sans elle, « l'article L. 742-3 ou
/// d'une requête … du règlement (UE) » rattacherait au mauvais texte.
fn genitive_gap(gap: &str) -> bool {
    // QUAL = qualificatif de subdivision entre le numéral et l'instrument :
    // « alinéa 2 », « paragraphe 1 », « point 2 », « (1) », « (ANCIEN) »,
    // chiffre nu (« 39, 2, de la Convention »). Deux formes de gap : avec
    // connecteur (« (1) du code ») ou qualificatif seul devant un sigle
    // (« 107, paragraphe 1, TFUE »).
    static RE_GAP: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?x)^(?:
              [\s,]*(?:precite[es]?\s+)?(?:et\s+suivants?\s+)?
              # [a-z]{1,2}[0-9]{0,2} : lettre d'énumération (« 5-1 a) ») ou
              # suffixe fiscal du numéral (« L. 80 CA du livre des procédures
              # fiscales », « L. 76 B ») — l'article appartient à l'instrument
              # génitif qui suit.
              (?:(?:et\s+)?(?:(?:premier|deuxieme|troisieme|second|dernier)\s+alineas?|(?:alineas?|alienas?|al\.?|§|paragraphes?|paragr\.?|points?)\s*(?:\d+(?:er)?|premiers?|seconds?)?|\(\s*[^)]{1,20}\)|[a-z]{1,2}[0-9]{0,2}[)']?|[ivx]{1,3}(?:\s+\d{1,2}\s*°?)?|\d{1,2}\s*°?|°|er)\s*,?\s+)*
              (?:et\s+)?
              (?:anciens?\s+|anciennes?\s+|nouveaux?\s+|nouvelles?\s+)?
              (?:du|de\s+la|de\s+l'\s*|des|de|dudit|de\s+ladite|du\s+meme|de\s+ce)\s*
              (?:le\s+|la\s+)?
              (?:'?(?:nouveau|nouvelle|ancien|ancienne)'?\s*(?:du\s+|de\s+la\s+|de\s+l'\s*)?)?
            |
              [\s,]*(?:(?:(?:premier|deuxieme|troisieme|second|dernier)\s+alineas?|(?:alineas?|alienas?|al\.?|§|paragraphes?|paragr\.?|points?)\s*(?:\d+(?:er)?|premiers?|seconds?)?|\(\s*[^)]{1,20}\)|[a-z][)']?|[ivx]{1,3}(?:\s+\d{1,2}\s*°?)?|\d{1,2}\s*°?)\s*,?\s+)+
            )$",
        )
        .unwrap()
    });
    // « du » avalé par l'OCR (« l'article 700 code de procédure civile ») :
    // l'adjacence pure vaut génitif.
    if gap.chars().count() <= 3 && gap.chars().all(|c| c == ' ' || c == ',') {
        return true;
    }
    gap.chars().count() <= 30 && RE_GAP.is_match(gap)
}

/// Intervalles de quote textuelle (« Aux termes de l'article X du CODE :
/// "…" ») en offsets chars. Ouvrante = guillemet précédé d'un « : » (la
/// convention des textes reproduits dans les décisions) ; les paires
/// ambiguës sans deux-points sont ignorées. Un article cité DANS la quote
/// appartient au texte quoté (cf. règle 3bis du compose).
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

/// Résolution mémoïsée par document : l'analyse de clé ([`KeyAnalysis`],
/// pliages + cascade `key_signals`) se calcule une fois par `text_key` — la
/// même clé résout des dizaines d'articles distincts — et la cible une fois
/// par (text_key, article_key).
struct Resolver<'a> {
    snap: &'a LinkSnapshot,
    vocab: &'a CompiledVocab,
    analyses: FxHashMap<String, std::sync::Arc<KeyAnalysis>>,
    memo: FxHashMap<(String, Option<String>), LinkTarget>,
}

/// Cache statique des [`KeyAnalysis`] : fonction PURE de `text_key`
/// (pliages + cascade `key_signals`, aucun snapshot) — partageable entre
/// runs, docs et threads. Les text_keys du corpus forment un vocabulaire
/// fermé de fait.
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
        // Cache inter-décisions (clé EXACTE, instrument inclus) : le memo
        // per-doc au-dessus garde la sémantique intra-document inchangée.
        let gk = (
            instrument.to_string(),
            text_key.to_string(),
            ak.map(str::to_string),
        );
        if let Some(t) = self.vocab.link_cache.read().unwrap().get(&gk) {
            self.memo.insert(k, t.clone());
            return t.clone();
        }
        self.analysis(text_key);
        let target = link_citation_analyzed(self.snap, instrument, &self.analyses[text_key], ak);
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

fn compose(
    toks: &[Tok],
    chars: &[char],
    folded: &str,
    char2byte: &[usize],
    vocab: &CompiledVocab,
    snap: &LinkSnapshot,
) -> Vec<CompiledCitation> {
    let mut resolver = Resolver {
        snap,
        vocab,
        analyses: FxHashMap::default(),
        memo: FxHashMap::default(),
    };

    // Registre document : uids des instruments résolus (unicité des articles
    // nus) — le « sur-exploiter » du KV-cache.
    let mut doc_uids: FxHashSet<String> = FxHashSet::default();
    let mut instr_resolved: FxHashMap<usize, (String, String, Option<String>)> =
        FxHashMap::default();
    for (i, t) in toks.iter().enumerate() {
        if let Some((surface, tk, _)) = instr_keys(t, chars) {
            let target = resolver.resolve(&surface, tk, None);
            if let Some(uid) = &target.ref_text_uid {
                doc_uids.insert(uid.clone());
            }
            instr_resolved.insert(i, (surface, tk.to_string(), target.ref_text_uid));
        }
    }
    // Anaphore de préfixe : « la Convention de Vienne » (forme courte non
    // datée, non résolue) adopte la forme datée résolue du même document dont
    // elle est le préfixe — cible unique exigée, sinon elle reste morte.
    let full_forms: Vec<(String, String, String)> = instr_resolved
        .values()
        .filter_map(|(surface, tk, uid)| Some((fold_stable(surface), tk.clone(), uid.clone()?)))
        .collect();
    for (i, t) in toks.iter().enumerate() {
        let Tok::Instr {
            s,
            e,
            treaty_short: true,
            ..
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
        let prefix = format!("{} ", fold_stable(&surface));
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

    // Adjacence génitive précalculée pour TOUS les Art, puis héritage
    // d'énumération EN ARRIÈRE : « articles L. 521-1 et L. 521-2 du CESEDA »
    // — seul le dernier item touche le génitif, les précédents en héritent
    // quand seuls des Art les séparent et qu'ils sont contigus (≤ 30 chars).
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
        // Entre les deux articles : rien que des articles — ou UN acte daté
        // embarqué par un qualificatif de rédaction (« 232, dans leur
        // rédaction antérieure à la loi n° 2004-439 du 26 mai 2004 et
        // 893 … du Code civil ») : l'énumération l'enjambe, l'héritage aussi.
        let (Tok::Art { e, .. }, Tok::Art { s, .. }) = (&toks[i], &toks[k]) else {
            unreachable!()
        };
        let mut gap_from = *e;
        let mut chain = true;
        for t in &toks[i + 1..k] {
            match t {
                Tok::Art { .. } => {}
                Tok::Instr { s: is, e: ie, .. }
                    if is.saturating_sub(gap_from) <= 40 && {
                        let lead: String = chars[gap_from..*is].iter().collect();
                        RE_REDACTION.is_match(fold_stable(&lead).trim_start_matches([',', ' ']))
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
        // ≤ 40 : la reprise génitive la plus longue pontée par RE_SEP
        // (« , du premier alinéa de l'article ») fait 33 chars.
        if chain && s.saturating_sub(gap_from) <= 40 {
            adj.insert(i, target);
        }
    }

    // Consommation des mentions par les articles adjacents — dérivée d'`adj`
    // AVANT toute émission : une mention consommée par ≥ 1 article (complément
    // génitif d'une locution, simple ou énumération) n'émet pas de span propre
    // (ADR 0166). Une mention TÊTE reste émise (« Vu le CJA et notamment ses
    // articles L. 761-1 » : le possessif résout en arrière hors `adj`).
    let mut consumed: FxHashMap<usize, u32> = FxHashMap::default();
    for &j in adj.values() {
        if instr_resolved.contains_key(&j) {
            *consumed.entry(j).or_insert(0) += 1;
        }
    }
    // Spans d'instrument qui ÉMETTRONT : les tokens d'article intérieurs à
    // l'un d'eux appartiennent au titre (« arrêté … pris en application de
    // l'article 238-0 A du CGI ») — consommés par la mention, jamais émis en
    // span propre. Les spans sortent disjoints PAR CONSTRUCTION (ADR 0160
    // §3) ; l'assert d'écriture du store reste l'unique frontière.
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
        // Token d'article intérieur à une mention qui émettra son propre span
        // (titre officiel citant un article : « arrêté pris en application de
        // l'article 238-0 A ») : consommé par le titre — ADR 0160 §3.
        if instr_emits.iter().any(|&(is, ie)| *s >= is && *s < ie) {
            continue;
        }
        // `num_key` = `normalize_article(surface)` posé au lex (seul
        // producteur de `Tok::Art`, jamais vide) — pas de recalcul.
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
        //    héritage d'énumération inclus. L'adjacence est AUTORITAIRE :
        //    l'article appartient syntaxiquement à cet instrument — si sa
        //    résolution échoue, on n'invente pas une autre cible.
        if let Some(&j) = adj.get(&i) {
            if let Some((surface, tk, _)) = instr_resolved.get(&j) {
                resolved = Some((surface.clone(), tk.clone()));
            }
            adjacent = true;
        }
        // 3. Anaphore → mention PRÉCÉDENTE la plus proche dont la clé porte
        //    la nature citée (« du même livre » → « livre des procédures
        //    fiscales ») ET qui possède l'article au catalogue.
        if resolved.is_none() && !adjacent {
            if let Some(nat) = same_nat {
                for j in (0..i).rev() {
                    let Some((surface, tk, uid)) = instr_resolved.get(&j) else {
                        continue;
                    };
                    // Possessif sans nature (« ses articles L. 821-1 ») : le
                    // référent est l'instrument IMMÉDIATEMENT amont — sans
                    // borne, le walk saute un contrat/une mention non résolue
                    // et mislink au premier code lointain porteur du numéro.
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
                    // Article au catalogue, OU structure inconnue (traités
                    // JORF sans lignes article) : la nature explicite suffit.
                    if uid
                        .as_deref()
                        .is_some_and(|u| snap.has_article(u, num_key) || !snap.has_article_info(u))
                    {
                        resolved = Some((surface.clone(), tk.clone()));
                        break;
                    }
                }
            }
        }
        // 3bis. Article DANS une quote : il appartient au texte quoté — le
        //       cadre (« Aux termes de l'article X du CODE : "…" ») prime sur
        //       l'antécédent (souvent un règlement cité dans la quote
        //       précédente) et sur l'unicité (le doc visait aussi le CJA).
        //       Cadre = dernière citation d'article émise ≤ 120 chars avant
        //       l'ouvrante, instrument résolu.
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
                                ref_num_key: snap
                                    .has_article(uid, num_key)
                                    .then(|| num_key.clone()),
                            });
                        } else {
                            resolved = Some((frame.instrument.clone(), frame.text_key.clone()));
                        }
                    }
                }
            }
        }
        // Génitif ORPHELIN : « l'article 82 du code de la famille congolais »
        // — un connecteur suit mais aucun instrument n'a été reconnu.
        // L'article appartient syntaxiquement à cet inconnu : deviner par
        // antécédent/unicité serait un mislink garanti (congolais → code
        // civil FR). On émet le span sans cible. Le désignateur de zone
        // d'urbanisme est enjambé (« 3 UA du POS » : article du règlement
        // local, hors catalogue), coordinations exclues.
        let orphan = resolved.is_none() && !adjacent && {
            let rest = &folded[char2byte[*e]..];
            RE_ORPHAN.is_match(rest)
                || RE_ZONE_STEP
                    .captures(rest)
                    .is_some_and(|c| !matches!(&c[1], "ou" | "ni" | "et"))
        };

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
        //    À égalité (≥ 2 porteurs cités), départage par la citation
        //    RÉSOLUE la plus proche en amont dont la cible est candidate —
        //    blocs d'articles reproduits (« - Article L212-8- " … ») : le
        //    code du travail introduit le bloc, le code rural cité ailleurs
        //    porte aussi ces clés ; chaque article résolu ancre le suivant.
        //    Clés PRÉFIXÉES seulement (L./R./D./A.) : un numéral nu
        //    (« 1382 ») vit dans trop de textes (civil/CPC, codes étrangers)
        //    pour que la proximité soit un signal.
        if resolved.is_none() && !adjacent && !orphan && direct_target.is_none() {
            if let Some(cands) = vocab.by_num_key.get(num_key) {
                let prefixed = num_key.len() > 3
                    && matches!(num_key.as_bytes()[0], b'L' | b'R' | b'D' | b'A')
                    && num_key[1..].starts_with(". ");
                let mut inter = doc_uids.iter().filter(|u| cands.binary_search(*u).is_ok());
                let uid = match (inter.next(), inter.next()) {
                    (Some(uid), None) => Some(uid.as_str()),
                    (Some(_), Some(_)) if prefixed => {
                        // Un SEUL candidat présent dans la fenêtre : deux
                        // porteurs cités à portée (CESEDA + code du travail
                        // autour d'un « L. 435-1 » nu) = ambigu, on s'abstient.
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
                        ref_num_key: Some(num_key.clone()),
                    });
                }
            }
        }
        // 5bis. Consistance de document : le même numéral déjà RÉSOLU en
        //       amont (« l'article 1648 précité », re-mention d'un article de
        //       traité sans structure) — adopté si TOUTES les résolutions
        //       antérieures de ce num_key visent la même cible.
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
        // 6. Antécédent NON VALIDABLE : le catalogue ne connaît aucun article
        //    de la cible (traités JORF sans structure — Convention de Rome,
        //    CVIM) ; l'existence n'étant pas réfutable, l'antécédent serré
        //    (≤ 600) est accepté une fois l'unicité épuisée.
        if resolved.is_none() && !adjacent && !orphan && direct_target.is_none() {
            if let Some(j) = last_instr {
                if let Some((surface, tk, Some(uid))) = instr_resolved.get(&j) {
                    let dist = s.saturating_sub(tok_start(&toks[j]));
                    // Natures conventionnelles + droit dérivé UE : une LOI
                    // sans structure connue est une loi de ratification/modif
                    // — les articles voisins ne sont pas les siens ; un
                    // règlement/une directive cités en antécédent serré sont
                    // le texte quoté.
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
        // 7. Canon jurisprudentiel : « l'article 700 » nu = frais irrépétibles
        //    du CPC, « 699 » = distraction des dépens — cités sans instrument
        //    dans TOUTES les juridictions judiciaires, jamais un autre texte.
        if resolved.is_none() && !adjacent && !orphan && direct_target.is_none() {
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

        let (instrument, text_key, target) = match (resolved, direct_target) {
            (Some((surface, tk)), _) => {
                let target = resolver.resolve(&surface, &tk, ak.as_deref());
                (surface, tk, target)
            }
            (None, Some(t)) => (String::new(), String::new(), t),
            (None, None) => (String::new(), String::new(), LinkTarget::default()),
        };
        out.push(CompiledCitation {
            char_start: *s,
            char_end: *e,
            instrument,
            article: Some(surface.clone()),
            text_key,
            article_key: ak,
            target,
        });
    }

    // Mentions d'instrument : span propre sauf si consommée par ≥ 1 article
    // adjacent (ADR 0166 — génitif simple ou énumération). Les mentions faibles
    // (structurelles sans identité chiffrée ni snap catalogue) et les actes
    // non citables (arrêté préfectoral, acte local) n'émettent que résolues.
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
            let target = resolver.resolve(surface, tk, None);
            out.push(CompiledCitation {
                char_start: *s,
                char_end: *e,
                instrument: surface.clone(),
                article: None,
                text_key: tk.clone(),
                article_key: None,
                target,
            });
        }
    }
    // Les spans sortent disjoints PAR CONSTRUCTION (consommation dérivée
    // d'`adj` + tokens d'article intérieurs aux mentions émettrices écartés,
    // ADR 0160 §3) ; l'assert d'écriture du store (spans triés sans
    // chevauchement, PK `(decision_id, char_start)`) reste l'unique frontière.
    out.sort_by_key(|c| c.char_start);
    out
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
/// Numéro d'acte FR : préfixé « n°/no » (numéro libre) ou nu (forme NN-NNN
/// exigée — « loi 2004-575 » oui, « loi 1901 » prose non).
static RE_LEX_NUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s+(?:n\s*[°oº]\s*\.?\s*[\w./-]*\d[\w./-]*|\d+-\d[\w./-]*)").unwrap()
});
/// Date d'acte : « du 1er janvier 2020 », « des 16 et 24 août 1790 »,
/// « en date du 5 juin 2015 », « du 16/01/2018 ».
static RE_LEX_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^\s+(?:en\s+date\s+)?(?:
            (?:du|des)\s+\d{1,2}(?:er)?(?:\s*(?:,|et|-|\u{2013})\s*\d{1,2}(?:er)?)*\s+(?:janvier|fevrier|mars|avril|mai|juin|juillet|aout|septembre|octobre|novembre|decembre)\s+\d{4}
          | du\s+\d{1,2}\s*[/.]\s*\d{1,2}\s*[/.]\s*\d{4}
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
        (?:\s*/\s*(?:ce|cee|ue|euratom))?",
    )
    .unwrap()
});
// Citations de jurisprudence (ADR 0165). Le span émis = le groupe 1 (token
// identifiant, convention 0143) ; la lettre de série Cassation (« n° B
// 19-12.345 ») reste hors span comme hors clé.
//
// Forme du n° de pourvoi (7 chiffres) et ses graphies réelles : point absent
// (« 11-25536 »), espace après le point (« 06-41. 614 »), virgule
// (« 02-14,799 »), second tiret (« 09-72-219 »), tiret initial absent
// (« 0430.583 »), point déplacé (« 08-215.47 »).
const CC_NUM: &str = r"(?:\d{2}[-.]\s?\d{2}[-.,]?\s?\d{3}|\d{4}\.\d{3}|\d{2}-\d{3}\.\d{2})";
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
          (?:,?\s*\(?\s*n\s*[°oº]?\s*s?\s*[:.]?\s*)?(?:[a-z]\s*)?({CC_NUM})\b"
    ))
    .unwrap()
});
/// Pourvoi en fenêtre (« Cass. 3e civ., 16 mars 2022, n° 18-23.954 ») : un
/// marqueur est exigé, la forme du pourvoi discrimine du reste de la prose.
static RE_CASE_POURVOI_WIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"{CC_MARK}\s*(?:[a-z]\s*)?({CC_NUM})\b")).unwrap());
/// Maillon d'énumération : « et 18-23.955 », « au n° W 00-44.846 » (plage),
/// « à W 00-44.846 » (« à » plié en « a »), date intercalée (« ; 25 juin
/// 1996, n 94-15130 »).
static RE_CASE_POURVOI_NEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?x)^\s*(?:,\s*(?:et\s+)?|et\s+|au?\s+|;\s*){CC_DATE}
          (?:n\s*[°oº]?\s*s?\s*[:.]?\s*)?(?:[a-z]\s*)?({CC_NUM})\b"
    ))
    .unwrap()
});
/// N° de requête CE : 5-6 chiffres nus derrière « n° » / « nos » / « sous le
/// numéro » — jamais sans le contexte CE (l'ancre), un `\d{6}` nu est du bruit.
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
/// (5-6 chiffres nus — les ordonnances-instruments portent un tiret d'année,
/// les ordonnances TA 7 chiffres, les judiciaires un RG à barre).
static RE_CASE_ORD_CE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*n\s*[°oº]\s*s?\s*(\d{5,6})\b").unwrap());
/// « La requête en référé n° 502527 de Mme D » : référé CE derrière
/// « requête ».
static RE_CASE_REF_CE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*en\s+refere\s+n\s*[°oº]\s*s?\s*(\d{5,6})\b").unwrap());
/// Sonde ARRIÈRE CE : « Par une décision n° 432537 du 8 janvier 2020, le
/// Conseil d'Etat… » — le n° précède l'ancre, gaté sur la nature
/// décision/ordonnance/arrêt (un n° de requête nu serait ambigu).
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
/// DC/QPC identifie la famille, sauf gate « décision » explicite
/// (« dans sa décision n°2015-479 » — « n° 2004-575 » nu serait une loi).
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
/// Requête CEDH : format NNNNN/YY à barre oblique (« req. n° 30010/10 ») —
/// les requêtes admin (7 chiffres nus) ne matchent pas.
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
/// chiffres avant la barre est une requête CEDH (« l'affaire 26604/16
/// Waldner c. France ») — routé par [`case_cjue_aff`].
static RE_CASE_AFF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^s?\s*\(?\s*(?:jointes\s+)?(?:n\s*[°oº]\s*)?
          (?:([ctf])(?:\s*-\s*|\s+)?)?(\d{1,5}\s*/\s*\d{2,7})\b",
    )
    .unwrap()
});
/// Affaire CJUE en fenêtre : le préfixe de rôle est exigé (un slashnum nu
/// serait un acte UE — règlement 44/2001). Graphies tolérées : tiret absent
/// collé (« C631/13 »), espace seul (« C 434/15 »), séparateur tiret
/// (« C 287-16 »), espace dans le numéro (« C-29/ 10 ») — normalisées par
/// [`cjue_key`]. Itéré sur toute la fenêtre (noms d'affaires et dates
/// s'intercalent librement entre numéros préfixés).
static RE_CASE_CJUE_WIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([ctf])(?:\s*-\s*|\s+)?(\d{1,4}\s*[/-]\s*\d{2,4})\b").unwrap());
/// Plage / énumération sans re-préfixe : « C-338/11 à 347/11 »,
/// « C-131/13, 163/13 et 164/13 » — le membre nu hérite du préfixe du membre
/// précédent (« à » plié en « a »).
static RE_CASE_CJUE_RANGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:,|a\s|et\s)\s*(\d{1,4}\s*/\s*\d{2,4})\b").unwrap());
/// N° de jugement TA (7 chiffres nus) ou d'arrêt CAA (AAXX99999) derrière
/// « jugement/arrêt n° » — chaîne du fond administratif (ADR 0165).
static RE_CASE_ADMIN_TA: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*n\s*[°oº]\s*s?\s*(\d{7})\b").unwrap());
static RE_CASE_ADMIN_TA_NEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:,|et)\s*(?:n\s*[°oº]\s*s?\s*)?(\d{7})\b").unwrap());
static RE_CASE_ADMIN_CAA: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*n\s*[°oº]\s*s?\s*(\d{2}[a-z]{2}\d{5})\b").unwrap());
static RE_CASE_ADMIN_CAA_NEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:,|et)\s*(?:n\s*[°oº]\s*s?\s*)?(\d{2}[a-z]{2}\d{5})\b").unwrap()
});
/// Formes de numéro de rôle : slashnum (« 21/04532 », « 09/ 497 »), TCOM
/// lettré (« 2015F00459 », « 2018j1016 », « 2023IP00245 »), TI historique
/// (« 11-04-000227 »), tiret (« 06-06235 », « 14-08937 »), TCOM Paris
/// tout-chiffres (« 91040813 », « 2023063685 »), TCOM espacé (« 2015 2815 »).
const RG_NUM: &str = r"(?:\d{1,4}\s*/\s*\d{2,7}|\d{4}(?:\s?[a-z]\s?|[a-z]{2})\d{3,6}|\d{2}-\d{2}-\d{3,6}|\d{2}-\d{2,6}|\d{8,10}|\d{4}\s\d{3,4})";
/// RG derrière l'ancre « RG / Rôle / répertoire général » : « RG n°
/// 21/04532 », « RG : 18/00064 », « sous le n° 11/00094 », « RG n° F
/// 12/06470 » (lettre de section).
static RE_CASE_RG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?x)^\s*[:.]?\s*(?:sous\s+le\s+)?(?:n(?:umero)?\s*[°oº]?\s*s?)?\s*:?\s*(?:[a-z]\s*)?
          ({RG_NUM})\b"
    ))
    .unwrap()
});
static RE_CASE_RG_NEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*(?:,|et)\s*(?:(?:n\s*[°oº]\s*s?\s*)?(?:rg\s*:?\s*)?)\s*(\d{1,4}\s*/\s*\d{2,7})\b",
    )
    .unwrap()
});
/// « enregistrée sous le no 20/00325 », « enrôlées sous les numéros 18/02640
/// et 18/02641 », « enregistré au répertoire général sous le n° 2008F175 ».
static RE_CASE_ENROL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?x)^(?:\(e\))?s?\s*(?:au\s+repertoire\s+general\s+)?
          sous\s+l(?:e|es)\s+n(?:umeros?|os?)?\s*[°oº]?\s*s?\s*[:.]?\s*(?:[a-z]\s+)?({RG_NUM})\b"
    ))
    .unwrap()
});
/// « CA [Localité 8], 14 nov. 2019, n°18/04366 » : sigle CA + ville/date puis
/// marqueur n° STRICT et slashnum.
static RE_CASE_CA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^\s*(?:\[[^\]]{1,30}\]|(?:de\s+)?[a-z][a-z'\- ]{1,25})?\s*,?\s*
          (?:\d{1,2}(?:er)?\s+[a-z]+\.?\s+\d{4}\s*,?\s*)?n\s*[°oº]\s*s?\s*(\d{1,4}\s*/\s*\d{2,7})\b",
    )
    .unwrap()
});
/// « DÉCISION DÉFÉRÉE : 21/00287 » — bandeau CA sans « RG », deux-points
/// exigés, numéro immédiat.
static RE_CASE_DEFEREE_NUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:a\s+la\s+cour\s*)?:\s*(\d{1,4}\s*/\s*\d{2,7})\b").unwrap()
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
/// STRICT (l'ancre est trop fréquente pour tolérer un numéro nu) —
/// « Jugement (N° 14/00048) rendu … », « jugement rendu le 10/05/2016
/// (instance n° 14-08937) », « ordonnance d'injonction de payer
/// n° 2023IP00245 ».
static RE_CASE_ADMIN_RG: LazyLock<Regex> = LazyLock::new(|| {
    // Forme tiret bornée à 5-6 chiffres au second membre : « instance
    // n° 14-08937 » cite, « l'ordonnance n° 58-1067 » (instrument à tiret
    // d'année) non.
    Regex::new(
        r"(?x)^\s*(?:rendue?\s+le\s+[\d/.-]{8,10}\s*)?(?:d'injonction\s+de\s+payer\s+)?
          \(?\s*(?:instance\s+)?n\s*[°oº]\s*s?\s*(?:rg\s*:?\s*)?(?:[a-z]\s+)?
          (\d{1,4}\s*/\s*\d{2,7}|\d{4}(?:\s?[a-z]\s?|[a-z]{2})\d{3,6}|\d{2}-\d{5,6})\b",
    )
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
        r"^\s+(?:meme\s+)?(code|livre|texte|loi|convention|reglement|decret|accord|charte|protocole|directive|ordonnance|arrete|traite)\b",
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
    // Préfixe « A. » (arrêtés — « A. 444-32 » code de commerce) : le point est
    // exigé, contrairement à L/R/D — « a » nu est une préposition (« 3 à 5 »).
    // Ordinaux latins : les composés AVANT leur préfixe (terdecies avant ter),
    // l'alternation regex prend la première branche.
    Regex::new(
        r"^(?:[lrd]\s*\.?\s*'?\s*|a\s*\.\s*)?(\d+(?:er)?(?:\.\d+)*(?:\s*-\s*\d+)*(?:\s+(?:bis|terdecies|ter|quaterdecies|quater|quindecies|quinquies|sexdecies|sexies|septies|octies|nonies|decies|undecies|duodecies))?(?:\s*-\s*\d+)*)",
    )
    .unwrap()
});
/// Ordinal latin seul (suite d'un suffixe-lettre : « 1394 B bis »).
static RE_NUM_ORDINAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^ (?:bis|terdecies|ter|quaterdecies|quater|quindecies|quinquies|sexdecies|sexies|septies|octies|nonies|decies|undecies|duodecies)\b",
    )
    .unwrap()
});
/// Connecteur génitif directement après un numéral — l'article appartient à
/// l'instrument qui suit, reconnu ou pas (gate du génitif orphelin).
static RE_ORPHAN: LazyLock<Regex> = LazyLock::new(|| {
    // Qualificatifs de subdivision et parenthèses tolérés avant le
    // connecteur (« 11 alinéa 3 des statuts », « 14 (cessation) des
    // statuts ») ; « de l' » colle au mot suivant, pas d'espace exigé.
    Regex::new(
        r"^[\s,]*(?:et\s+)?(?:(?:alineas?|alienas?|al\.?|§|paragraphes?|points?)\s*\d*\s*,?\s*|\([^)]{1,60}\)\s*)*(?:(?:du|de\s+la|des|dudit|de\s+ladite)\s|de\s+l')",
    )
    .unwrap()
});
/// Désignateur de zone d'urbanisme entre le numéral et le connecteur génitif
/// (« 3 UA du POS », « 4 NC1 du PLU ») : le token capturé est examiné en
/// aval — les coordinations (« L. 57 ou de la notification ») ne font pas
/// zone, le génitif ne leur appartient pas.
static RE_ZONE_STEP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s+([0-9]?[a-z]{1,2}[0-9]{0,2})\s+(?:(?:du|de\s+la|des)\s|de\s+l')").unwrap()
});
/// Qualificatif d'état du texte ouvrant sur un acte daté embarqué —
/// l'énumération d'articles l'enjambe (« dans leur rédaction antérieure à la
/// loi n° 2004-439 du 26 mai 2004 et 893, 894 du Code civil »). Seuls les
/// participes qui EXIGENT un acte objet (antérieure à, issue de, modifiée
/// par…) ouvrent l'enjambement : « dans sa rédaction applicable, du code
/// civil » est un qualificatif clos suivi du génitif de l'énumération.
static RE_REDACTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^dans\s+(?:sa|leur|ses|leurs)\s+redactions?\s+(?:anterieures?|posterieures?|issues?|resultante?s?|modifiees?)\s+(?:a|de|du|par)\s+(?:la\s+|l'\s*)?",
    )
    .unwrap()
});
/// Paragraphe(s) romain(s) de subdivision suivant un numéral d'article
/// (« L. 1142-1, I, du code… », « II et III, »). Enjambé par la marche
/// d'énumération avant le probe génitif — inerte si le génitif ne suit pas.
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
static RE_SEP: LazyLock<Regex> = LazyLock::new(|| {
    // « devenu » = séparateur : « 1134, devenu 1103, du code civil » émet les
    // deux numéraux, l'héritage d'énumération rattache le premier. Le pont
    // article-répété (« L. 626-22, du premier alinéa de l'article
    // L. 642-20-1, de l'article L. 651-2 ») traverse la reprise génitive.
    // Le qualificatif de subdivision collé au numéral (« 6 paragraphe 1 et
    // 13 de la convention ») fait partie du séparateur : l'énumération le
    // traverse.
    Regex::new(
        r"^(?:\s*[a-z]\))?(?:\s*(?:paragraphes?|paragr\.?|alineas?|§)\s*\d+(?:er)?)?(?:\s*(?:et|&)\s*suivant(?:e?s)?)?(?:\s*(?:anciens?|anciennes?|nouveaux?|nouvelles?|modifiee?s?|abrogee?s?))?(?:\s*,?\s*(?:et\s+|ou\s+)?(?:du\s+|de\s+l'\s*|des\s+|de\s+la\s+)(?:premier\s+|deuxieme\s+|dernier\s+)?(?:alinea\s+de\s+l'\s*)?articles?\s+|\s*,?\s*devenu\s+|\s*,?\s*(?:ou\s+)?anciennement\s+|\s*,?\s*(?:et|ou|a|à|ainsi\s+que)\s+|\s*,?\s*)",
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
        }
    }

    fn fixture() -> (Vec<CatalogText>, Vec<(String, String)>) {
        let texts = vec![
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
            // Titre officiel citant des articles : l'ancre « articles » du
            // titre rouvre l'énumération (test title_artword).
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
            // Titre officiel traversant « l'article N » au singulier (pas de
            // réouverture d'énumération) : le walker avale le titre entier,
            // le token article interne s'emboîte dans la mention (cas du
            // crash reextract v13, décision 919).
            text(
                "JORF_2010",
                "Arrêté du 12 février 2010 pris en application du deuxième alinéa du 1 de \
                 l'article 238-0 A du code général des impôts",
                "Arrêté du 12 février 2010 pris en application du deuxième alinéa du 1 de \
                 l'article 238-0 A du code général des impôts",
                "ARRETE",
                30,
            ),
            // Décret JORF de publication d'un traité : cible des alias
            // embarqués (validés par existence au catalogue), sans structure.
            text(
                "JORFTEXT000000703898",
                "Décret n° 87-1034 du 22 décembre 1987 portant publication de la convention \
                 des Nations unies sur les contrats de vente internationale de marchandises",
                "Convention des Nations unies sur les contrats de vente internationale de marchandises",
                "DECRET",
                40,
            ),
        ];
        // num_key en forme d'affichage DB (« L. 761-1 »), cf. legal_article.num_key.
        // Les R. 771-* du CJA sont étoilés au catalogue (décrets en Conseil
        // d'État) — cités « R. 771-5 » dans les décisions.
        let articles = vec![
            ("LEGI_CJA".to_string(), "L. 761-1".to_string()),
            ("LEGI_CJA".to_string(), "R*771-5".to_string()),
            ("LEGI_CJA".to_string(), "R*771-7".to_string()),
            ("LEGI_CC".to_string(), "1240".to_string()),
            ("LEGI_CESEDA".to_string(), "L. 742-3".to_string()),
            ("LEGI_CESEDA".to_string(), "L. 313-14".to_string()),
            ("LEGI_CESEDA".to_string(), "L. 425-9".to_string()),
            ("LEGI_CESEDA".to_string(), "L. 611-3".to_string()),
            ("LEGI_CESEDA".to_string(), "R. 313-22".to_string()),
            ("JORF_CEDH".to_string(), "2".to_string()),
            ("JORF_CEDH".to_string(), "5".to_string()),
            ("JORF_CEDH".to_string(), "7".to_string()),
            ("JORF_CEDH".to_string(), "14".to_string()),
            ("LEGI_CGI".to_string(), "97".to_string()),
            ("LEGI_CGI".to_string(), "98".to_string()),
            ("LEGI_CGI".to_string(), "100".to_string()),
            ("LEGI_LPF".to_string(), "L. 73".to_string()),
            ("LEGI_LPF".to_string(), "L. 16 A".to_string()),
            ("LEGI_CGI".to_string(), "164 B".to_string()),
            ("LEGI_CGI".to_string(), "1518 A quinquies".to_string()),
            ("LEGI_COM".to_string(), "A. 444-32".to_string()),
            ("JORF_AJ".to_string(), "37".to_string()),
            ("LEGI_TRAV".to_string(), "L. 212-8".to_string()),
            ("LEGI_TRAV".to_string(), "L. 212-8-5".to_string()),
            ("LEGI_RUR".to_string(), "L. 212-8".to_string()),
            ("LEGI_RUR".to_string(), "1144".to_string()),
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
        extract_citations(text, &vocab, &snap)
    }

    /// Citations de jurisprudence (ADR 0165) : chemin `doc_extract` complet,
    /// référentiel juridiction minimal pour les clés RG.
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
        doc_extract(text, &vocab, &snap, &chrono).cases
    }

    /// (surface, target_ref) des spans émis — la surface vérifie la borne
    /// « token identifiant » de la convention 0143.
    fn case_surfaces(text: &str) -> Vec<(String, String)> {
        cases(text)
            .into_iter()
            .map(|c| {
                (
                    text.chars()
                        .skip(c.char_start)
                        .take(c.char_end - c.char_start)
                        .collect(),
                    c.target_ref,
                )
            })
            .collect()
    }

    fn by_article<'a>(out: &'a [CompiledCitation], art: &str) -> &'a CompiledCitation {
        out.iter()
            .find(|c| c.article.as_deref() == Some(art))
            .unwrap_or_else(|| panic!("article {art} absent"))
    }

    #[test]
    fn enumeration_bridges_redaction_qualifier() {
        // Le qualificatif d'état du texte embarque un acte daté au milieu de
        // l'énumération : elle l'enjambe (893 reprend), et l'héritage arrière
        // traverse l'acte embarqué (230/232 remontent au Code civil).
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
        assert_eq!(c.target.ref_num_key.as_deref(), Some("L. 761-1"));
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
        // ORCA_21NC03180 : la quote du cadre cite une ordonnance — l'anaphore
        // « du même code » doit remonter PAR-DESSUS jusqu'au dernier code.
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
        // Le num_key émis est la forme OFFICIELLE étoilée du catalogue.
        assert_eq!(c.target.ref_num_key.as_deref(), Some("R*771-5"));
        let c = by_article(&out, "R. 771-7");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CJA"));
    }

    #[test]
    fn same_code_after_ordinal_qualifier_resolves() {
        // ORCA_23BX01652 : « le 9° de l'article L. 611-3 du même code ».
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
        // DCE_442036 : « …du protocole additionnel n° 12 et des articles 2,
        // 5, 7 et 14 de la convention, d'une demande… » — la mention nue
        // génitive est une anaphore de la CEDH épelée en amont.
        // La CEDH n'apparaît qu'IMBRIQUÉE dans la mention protocole : c'est
        // la mention interne (nested) qui sert d'antécédent.
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
    fn genitive_instrument_inside_quote_attaches() {
        // DCA_22DA01151 : « l'article 97 du code général des impôts » cité
        // DANS la quote de l'article L. 73 du LPF — le génitif explicite
        // prime, dans la quote comme dehors.
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
            Some("JORFTEXT000000703898")
        );
    }

    #[test]
    fn dead_treaty_prefix_emits_nothing() {
        // Aucune forme longue RÉSOLUE dans le document : la forme courte est
        // morte — l'article reste sans cible (adjacence autoritaire), la
        // mention faible n'émet aucun span propre.
        let out =
            cites("Le choix est régi par l'article 3 de la Convention de Rome selon l'appelant.");
        let c = by_article(&out, "3");
        assert_eq!(c.target.ref_text_uid, None);
        assert!(!out.iter().any(|c| c.article.is_none()));
    }

    #[test]
    fn quoted_article_belongs_to_quoted_text() {
        // L'article cité DANS la quote appartient au texte quoté (cadre),
        // pas à un antécédent ni à l'unicité.
        let out = cites(
            "Aux termes de l'article L. 742-3 du code de l'entrée et du séjour des \
             étrangers et du droit d'asile : \" Par dérogation à l'article L. 313-14, \
             l'étranger peut être transféré vers l'État responsable de l'examen. \"",
        );
        let c = by_article(&out, "L. 313-14");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CESEDA"));
        assert_eq!(c.target.ref_num_key.as_deref(), Some("L. 313-14"));
    }

    #[test]
    fn non_genitive_following_instrument_is_not_attached() {
        // « L. 742-3 ou d'une requête … du règlement » : le règlement n'est
        // PAS le texte de l'article — l'antécédent (quote CESEDA) le porte.
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
        // « 1° » = subdivision de l'article, jamais un numéro d'article : pas
        // de span « 1 », et le génitif rattache toujours l'article porteur.
        let out = cites("la cour a violé l'article L. 1142-1-1, 1°, du code civil ;");
        assert_eq!(out.iter().filter(|c| c.article.is_some()).count(), 1);
        assert_eq!(
            by_article(&out, "L. 1142-1-1")
                .target
                .ref_text_uid
                .as_deref(),
            Some("LEGI_CC")
        );
        // L'énumération continue derrière l'ordinal enjambé (avec ou sans
        // virgule de clôture).
        let out = cites(
            "au regard des articles L. 1142-1-1, 1°, et L. 1142-17 du code civil, \
             et de l'article 78-2, 3° et 4° du code civil.",
        );
        let nums: Vec<&str> = out.iter().filter_map(|c| c.article.as_deref()).collect();
        assert_eq!(nums, vec!["L. 1142-1-1", "L. 1142-17", "78-2"]);
    }

    #[test]
    fn enumeration_inherits_trailing_genitive() {
        // Seul le dernier item touche le génitif — les précédents héritent.
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
        // « de cette loi » saute le code (nature ≠ loi) pour retrouver la loi.
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
    fn orphan_genitive_never_guesses() {
        // « du code de la famille congolais » : instrument hors catalogue —
        // ni antécédent ni unicité, même si « 37 » n'existe qu'à un endroit.
        let out = cites(
            "Vu la loi n° 91-647 du 10 juillet 1991. Le ministre s'est fondé sur \
             l'article 37 du code de la famille congolais pour refuser le visa.",
        );
        let c = by_article(&out, "37");
        assert_eq!(c.target.ref_text_uid, None);
    }

    #[test]
    fn zone_designator_genitive_is_orphan() {
        // « l'article 37 UA du POS » : article du règlement de zone d'un plan
        // local d'urbanisme, hors catalogue. Le désignateur de zone est
        // enjambé par la gate du génitif orphelin — sans elle, l'unicité
        // lierait « 37 » à la loi du 10 juillet 1991 (seul porteur cité).
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
        // UNE occurrence : le numéral ; l'instrument vit dans le verbose.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].article.as_deref(), Some("1240"));
        // Mention nue : span propre.
        let out = cites("Vu le code civil.");
        assert_eq!(out.len(), 1);
        assert!(out[0].article.is_none());
    }

    #[test]
    fn structural_dated_act_without_catalog_still_spans() {
        // Acte numéroté ABSENT du catalogue : span structurel émis (identité
        // chiffrée), lien vide — le catalogue s'enrichit sans changer le scan.
        let out = cites("en application de la loi n° 2099-1 du 1er janvier 2099 précitée.");
        assert_eq!(out.len(), 1);
        assert!(out[0].article.is_none());
        assert_eq!(out[0].target.ref_text_uid, None);
    }

    #[test]
    fn local_act_emits_no_span() {
        // Acte local non citable : ni span ni lien (gate de citabilité).
        let out = cites("par un arrêté préfectoral du 3 mai 2019, le préfet a refusé.");
        assert!(out.is_empty());
    }

    #[test]
    fn reproduced_block_ties_break_on_nearest_resolved_citation() {
        // judilibre/6253cba2bd3db21cbdd8de7a : bloc d'articles REPRODUITS
        // (« - Article L212-8- " … ») — « L. 212-8 » vit dans le code du
        // travail ET le code rural, tous deux cités au doc ; le code du
        // travail introduit le bloc, chaque article résolu ancre le suivant.
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
        assert_eq!(c.target.ref_num_key.as_deref(), Some("L. 212-8"));
    }

    #[test]
    fn fiscal_letter_suffix_articles_resolve() {
        // Suffixe-lettre fiscal : la lettre seule MAJUSCULE fait partie du
        // numéral (« 164 B » ≠ « 164 » au catalogue).
        let out = cites(
            "Il résulte de l'article 164 B du code général des impôts que le              contribuable est imposable. L'article L. 16 A du livre des              procédures fiscales encadre la demande.",
        );
        let c = by_article(&out, "164 B");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CGI"));
        assert_eq!(c.target.ref_num_key.as_deref(), Some("164 B"));
        let c = by_article(&out, "L. 16 A");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_LPF"));
        assert_eq!(c.target.ref_num_key.as_deref(), Some("L. 16 A"));
    }

    #[test]
    fn fiscal_letter_ordinal_and_arrete_prefix_resolve() {
        // Combo lettre + ordinal (« 1518 A quinquies ») et préfixe d'arrêté
        // (« A. 444-32 ») — le « a » nu reste une préposition (« 3 à 5 »).
        let out = cites(
            "La valeur locative est fixée selon l'article 1518 A quinquies du              code général des impôts. Les émoluments sont tarifés à l'article              A. 444-32 du code de commerce.",
        );
        let c = by_article(&out, "1518 A quinquies");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CGI"));
        assert_eq!(c.target.ref_num_key.as_deref(), Some("1518 A quinquies"));
        let c = by_article(&out, "A. 444-32");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_COM"));
        assert_eq!(c.target.ref_num_key.as_deref(), Some("A. 444-32"));
    }

    #[test]
    fn title_citing_articles_reopens_enumeration() {
        // Le walker de titre daté consomme l'ancre « articles » interne au
        // titre officiel — l'énumération doit rouvrir derrière et rattacher
        // les numéraux au génitif qui suit (CESEDA), pas à l'arrêté.
        let out = cites(
            "Vu : - l'arrêté du 27 décembre 2016 relatif aux conditions \
             d'établissement et de transmission des certificats médicaux, \
             rapports médicaux et avis mentionnés aux articles R. 313-22, \
             R. 313-23 et R. 511-1 du code de l'entrée et du séjour des \
             étrangers et du droit d'asile ;",
        );
        let c = by_article(&out, "R. 313-22");
        assert_eq!(c.target.ref_text_uid.as_deref(), Some("LEGI_CESEDA"));
        assert_eq!(c.target.ref_num_key.as_deref(), Some("R. 313-22"));
    }

    #[test]
    fn spans_are_sorted_and_non_overlapping() {
        // Invariant d'écriture store (PK `(decision_id, char_start)`) : spans
        // triés, sans chevauchement — disjoints par construction (ADR 0160
        // §3). Le titre officiel de l'arrêté traverse « l'article 238-0 A »
        // au singulier — le token article interne appartient au titre et ne
        // doit pas émettre (crash reextract v13, décision 919).
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
        // « Vu le CJA et notamment ses articles L. 761-1, R. 771-5 et
        // R. 771-7 » (formule d'ordonnance) : « ses articles » est une ancre
        // SameConn qui consomme l'ancre ArtWord — l'énumération rouvre sur sa
        // fin et les numéraux se rattachent à l'instrument amont.
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
        // « ses articles L. 761-1 et R. 771-5, tel que modifié par le décret
        // n° 87-1034 … » : la prose « tel que modifié par le » n'est PAS un
        // gap génitif (la branche qualificatif-seul de RE_GAP exigeait un
        // blanc entre qualificatifs — sans lui, elle avalait toute prose
        // bas-de-casse lettre à lettre) ; les articles restent au CJA amont.
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
        // Les DEUX porteurs sont cités à portée : la proximité n'est plus un
        // signal, on s'abstient (l'accord d'entreprise intercalé décroche
        // aussi l'antécédent).
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
        // TA78 DTA_2305985 : « L. 611-3 1° et 5° du CESEDA » — l'énumération
        // de degrés fait partie du gap génitif, l'adjacence reste autoritaire.
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
        // Uy4YBZQ25Fr2 : « les articles L. 1142-1, I, du code de la santé
        // publique et 36 de la loi n° 66-879 » — le paragraphe romain suit
        // le premier numéral, l'énumération distributive le traverse.
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

    // ── citations de jurisprudence (ADR 0165) ───────────────────────────────

    #[test]
    fn case_pourvoi_anchored_and_enumeration() {
        // Span = le token identifiant seul ; lettre de série hors span ;
        // l'énumération émet un span par pourvoi.
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
        // « Cass. soc., … » : l'ancre abrégée porte le point.
        assert_eq!(
            case_surfaces("(Cass. soc., 18 janvier 2011, n° 09-89.876)"),
            vec![("09-89.876".to_string(), "cc|0989876".to_string())]
        );
        // Prose sans n° de pourvoi : rien.
        assert!(case_surfaces("le pourvoi formé par M. X est rejeté").is_empty());
    }

    #[test]
    fn case_conseil_detat_needs_juridiction_context() {
        assert_eq!(
            case_surfaces("la décision du Conseil d'État du 12 avril 2019, n° 412412"),
            vec![("412412".to_string(), "ce|412412".to_string())]
        );
        // Un n° à 6 chiffres SANS le contexte Conseil d'État n'émet rien.
        assert!(case_surfaces("la demande enregistrée sous le n° 412412 est rejetée").is_empty());
    }

    #[test]
    fn case_constit_requires_dc_qpc_suffix() {
        // Par l'ancre juridiction ET par le mot « décision » (token
        // multi-rôles Eu → Case).
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
        // Sans suffixe DC/QPC, « décision n° 2020-1310 » est un acte : rien.
        assert!(case_surfaces("la décision n° 2020-1310 du 29 octobre 2020").is_empty());
    }

    #[test]
    fn case_cedh_sigle_disambiguates_court_from_convention() {
        // Sigle + n° de requête aval = la COUR (span jurisprudence, pas
        // d'instrument).
        let text = "CEDH, arrêt X. c. France du 3 octobre 2014, req. n° 30010/10";
        assert_eq!(
            case_surfaces(text),
            vec![("30010/10".to_string(), "cedh|30010/10".to_string())]
        );
        assert!(
            cites(text).is_empty(),
            "le sigle ne doit pas lier la Convention"
        );
        // Sigle sans requête : l'alias Convention garde la main.
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
        // Slashnum nu en fenêtre d'« arrêt » : un acte UE, pas une affaire.
        assert!(case_surfaces("l'arrêt applique le règlement 44/2001 du Conseil").is_empty());
    }

    #[test]
    fn case_cjue_party_names_and_graphies() {
        // Nom d'affaire entre l'ancre et le numéro, y compris dans
        // l'énumération ; graphies tiret-séparateur et tiret absent
        // normalisées dans la clé.
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
        // « ordonnance » sans instrument daté FR : sonde CJUE (préfixe exigé).
        assert_eq!(
            case_surfaces("l'ordonnance Glaxosmithkline C-210/13 du 14 novembre 2013"),
            vec![("C-210/13".to_string(), "cjue|c-210/13".to_string())]
        );
        // Une ordonnance FR datée reste un instrument, pas une affaire.
        assert!(case_surfaces("l'ordonnance n° 58-1067 du 7 novembre 1958").is_empty());
        // Ancre « affaire » avec mots interstitiels : la fenêtre préfixée relaie.
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
            vec![("21/04532".to_string(), "rg|tj75056|21/04532".to_string())]
        );
        assert_eq!(
            case_surfaces("l'arrêt de la cour d'appel d'Aix-en-Provence (RG 19/12345)"),
            vec![(
                "19/12345".to_string(),
                "rg|ca_aix_provence|19/12345".to_string()
            )]
        );
        // Sans juridiction mappable : clé NUE `rg||NUM` — jamais résolue
        // (code vide ≠ tout code), mais décorée quand la GT la résorbe.
        assert_eq!(
            case_surfaces("l'affaire enregistrée RG n° 21/04532 au greffe"),
            vec![("21/04532".to_string(), "rg||21/04532".to_string())]
        );
        // Tribunal hors référentiel de test : la forme la plus proche décide,
        // sa ville ne mappe pas → clé nue aussi.
        assert_eq!(
            case_surfaces("le jugement du tribunal judiciaire de Meaux, RG n° 21/04532"),
            vec![("21/04532".to_string(), "rg||21/04532".to_string())]
        );
    }

    /// Raccourci d'assertion : (surface, clé) attendues, dans l'ordre.
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
        // Le signalement utilisateur : la chaîne du fond administratif doit
        // être citable inline (ADR 0165 amendé).
        assert_cases(
            "Par un jugement n° 1901563 du 15 février 2023, le tribunal administratif \
             de Nantes a condamné l'ONIAM à verser une somme",
            &[("1901563", "af|ta_nantes|1901563")],
        );
        // Énumération « Par deux jugements n° X et n° Y ».
        assert_cases(
            "Par deux jugements n° 2202850 et n° 2202851 du 16 mars 2023, le tribunal \
             administratif de Nantes a rejeté leurs demandes",
            &[
                ("2202850", "af|ta_nantes|2202850"),
                ("2202851", "af|ta_nantes|2202851"),
            ],
        );
    }

    #[test]
    fn case_admin_caa_arret_keeps_uppercase_docket() {
        assert_cases(
            "Par un arrêt n° 20NT01234 du 5 mai 2022, la cour administrative d'appel \
             de Nantes a rejeté l'appel",
            &[("20NT01234", "af|caa_nantes|20NT01234")],
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
        // Une autre juridiction entre le n° et l'ancre invalide la sonde.
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
        // Forme gatée « décision » sans suffixe DC/QPC.
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
        // Ordinal d'arrêt CA (année/n°) : clé nue rg||, pas une requête CEDH.
        assert_cases(
            "ARRÊT AU FOND DU 25 MARS 2011 N° 2011/183 Rôle N° 10/01539",
            &[("2011/183", "rg||2011/183"), ("10/01539", "rg||10/01539")],
        );
        // Un acte UE derrière « arrêt » n'est pas une requête.
        assert_cases(
            "l'arrêt rendu au visa de l'article 3 du règlement n° 2988/95 du Conseil",
            &[],
        );
        // Requête CEDH via l'ancre « affaire » (4-5 chiffres avant la barre).
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
        // Plage à membre nu : préfixe hérité.
        assert_cases(
            "Par son arrêt C-338/11 à 347/11 Santander Asset Management du 10 mai 2012",
            &[("C-338/11", "cjue|c-338/11"), ("347/11", "cjue|c-347/11")],
        );
        // Lettre de série d'un pourvoi Cassation : pas un rôle CJUE.
        assert_cases(
            "a rendu l'arrêt suivant : Pourvoi n° T 17-21.405 RÉPUBLIQUE FRANÇAISE",
            &[("17-21.405", "cc|1721405")],
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
        // Énumération datée derrière une chambre.
        assert_cases(
            "(Civ. 1ère 11 mars 1989, n 87-10798 ; 25 juin 1996, n 94-15130)",
            &[("87-10798", "cc|8710798"), ("94-15130", "cc|9415130")],
        );
        // Date de l'arrêt comme seul marqueur (n° omis).
        assert_cases(
            "soumis au droit irlandais (Cass, com, 18 décembre 2019, 18-14.827).",
            &[("18-14.827", "cc|1814827")],
        );
    }

    #[test]
    fn case_rg_enrol_deferee_and_own_header_guards() {
        // Forme épelée des bandeaux CA.
        assert_cases(
            "décision attaquée en date du 05 octobre 2020, enregistrée sous le no 20/00325",
            &[("20/00325", "rg||20/00325")],
        );
        assert_cases(
            "DÉCISION DÉFÉRÉE :  21/00287  Jugement du TJ HORS JAF",
            &[("21/00287", "rg||21/00287")],
        );
        assert_cases(
            "Appel d'une décision (No RG 09/ 29) rendue par le juge de l'expropriation",
            &[("09/ 29", "rg||09/29")],
        );
        // La requête EN COURS (JLD rétention) n'est pas une citation.
        assert_cases(
            "Vu la requête en prolongation de rétention, enregistrée sous le N°RG \
             25/04461 présentée par M. le Préfet",
            &[],
        );
        // Bandeau des dossiers joints de la décision elle-même (Portalis).
        assert_cases(
            "N° RG 22/01114 - N° Portalis DBVF-V-B7G-GAYK MINUTE N°",
            &[],
        );
        // Formes TCOM : lettrée et tout-chiffres, casse du docket conservée.
        assert_cases(
            "jugement du Tribunal de Commerce de PARIS - RG n° 2023063685",
            &[("2023063685", "rg|tcom7501|2023063685")],
        );
        assert_cases(
            "jugement du Tribunal de Commerce de PARIS du 16 Novembre 2015 enregistré(e) \
             au répertoire général sous le n° 2015F00459",
            &[("2015F00459", "rg|tcom7501|2015F00459")],
        );
        // Graphie tiret d'un RG à barre : canonisée dans la clé seulement.
        assert_cases(
            "d'un jugement rendu le 10/05/2016 (instance n° 14-08937) par le tribunal \
             de grande instance de Paris",
            &[("14-08937", "rg|tj75056|14/08937")],
        );
    }

    #[test]
    fn case_spans_do_not_perturb_instrument_extraction() {
        // Une citation d'arrêt au milieu d'une citation d'articles : les deux
        // flux coexistent sans se voler de tokens.
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
}
