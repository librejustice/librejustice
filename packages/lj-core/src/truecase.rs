//! Recasse déterministe des décisions « caps-lock » (vieux fonds, surtout
//! Cassation : texte intégralement en MAJUSCULES *et sans accents*). On restaure
//! une casse de phrase lisible, les accents (lexique français embarqué, filtre de
//! dominance), les noms propres de lieux/juridictions (gazetteer) et quelques
//! acronymes/civilités. Purement cosmétique, côté affichage : n'altère ni le
//! texte source stocké ni l'index BM25 (lui-même lowercasé/ascii-foldé).
//!
//! Désambiguïsation contextuelle des homographes que le filtre de dominance
//! laisse sans accent, tranchée sur les mots voisins repliés :
//! - `a` (auxiliaire) / `à` (préposition) — cf. `disambiguate_a` ;
//! - `ou`/`où`, `des`/`dès`, `du`/`dû` — cf. `disambiguate_closed` ;
//! - participe `-é` homographe d'un présent/nom (« condamne »/« condamné »,
//!   « arrête »/« arrêté »), y compris adjectival sans auxiliaire (« l'arrêt
//!   attaqué ») : tranché par un modèle contextuel appris hors-ligne sur le corpus
//!   (cf. `PARTICIPLE_CTX`, ADR 0072), avec repli sur la règle auxiliaire
//!   (`participle_fires`) pour les formes hors-modèle.
//!
//! Pas de noms de personnes.

use std::collections::HashMap;
use std::sync::LazyLock;

/// Lexique des formes accentuées (une par ligne) ; la clé de recherche est la
/// forme repliée (`fold`). Construit hors-ligne depuis Lexique383 avec un filtre
/// de dominance : seules les formes dont l'orthographe *sans accent* est rare
/// comme mot réel sont incluses (protège « a/à », « ou/où », « sur/sûr »…).
const ACCENTS_TXT: &str = include_str!("../data/accents_fr.txt");

/// Complément du lexique : formes accentuées sans ambiguïté (participes accordés
/// `-ée/-és/-ées`, vocabulaire juridique/genré absent de Lexique383 —
/// « greffière », « préfète », « requérante »…) dont l'orthographe sans accent
/// est rare comme mot réel (même filtre de dominance). Fichier séparé pour rester
/// sous le plafond de taille du hook (200 Ko/fichier).
const ACCENTS_SUPP_TXT: &str = include_str!("../data/accents_supplement_fr.txt");

/// Noms propres de lieux/juridictions français (une forme cassée par ligne),
/// filtrés des collisions avec les mots communs (« sens », « vienne », « tour »).
const PROPER_TXT: &str = include_str!("../data/proper_nouns_fr.txt");

/// Mots repliés qui, juste **après** « a/à », trahissent l'auxiliaire/le verbe « a »
/// (« a été », « a pas », « a lieu », « a condamné », « a donc statué »). Appris
/// hors-ligne sur les décisions accentuées disjointes de la GT (pureté « a » ≥ 97 %,
/// unis aux participes irréguliers connus). Un mot ici ⇒ on n'accentue pas.
const A_AUX_NEXT_TXT: &str = include_str!("../data/a_aux_next_fr.txt");

/// Mots repliés qui, juste **avant** « a/à », trahissent l'auxiliaire (sujet élidé/
/// pronom/titre : « il a », « n'a », « qui a », « les a », « Mme a »). Un mot ici ⇒
/// on n'accentue pas.
const A_AUX_PREV_TXT: &str = include_str!("../data/a_aux_prev_fr.txt");

/// Participes passés en `-é` homographes d'un présent/nom non accentué
/// (« condamne »/« condamné », « attaque »/« attaqué ») — exclus du lexique
/// principal par le filtre de dominance. Format `folded<TAB>accentué<TAB>tier`
/// (`s` = lecture participe dominante, `w` = nom/présent dominant). Restaurés
/// seulement quand un auxiliaire précède (cf. `participle_fires`), signal de
/// précision ≥ 97 % mesuré sur la GT recasse.
const PARTICIPLES_TXT: &str = include_str!("../data/participles_fr.txt");

/// Désambiguïsation contextuelle du participe masculin singulier `-é` (présent/nom
/// « condamne » vs participe « condamné » ; double homographe « arrête »/« arrêté »),
/// apprise hors-ligne sur ~28 000 décisions accentuées **disjointes de la GT
/// recasse** (cf. ADR 0072). Format `folded<TAB>forme_é<TAB>forme_alt<TAB>défaut(e|é)
/// <TAB>flips` : `forme_é` = participe accentué, `forme_alt` = lecture alternative
/// (vide ⇒ non accentuée), `défaut` = lecture majoritaire en jurisprudence, `flips`
/// = voisins repliés qui l'inversent (`p<token>`/`n<token>` ; `^`/`$` = début/fin).
/// Contextes ≥ 95 % décisifs uniquement. Consulté avant le lexique.
const PARTICIPLE_CTX_TXT: &str = include_str!("../data/participle_context_fr.txt");

static ACCENTS: LazyLock<HashMap<String, &'static str>> = LazyLock::new(|| {
    ACCENTS_TXT
        .lines()
        .chain(ACCENTS_SUPP_TXT.lines())
        .filter(|l| !l.is_empty())
        .map(|w| (fold(w), w))
        .collect()
});

static PROPER: LazyLock<HashMap<String, &'static str>> = LazyLock::new(|| {
    PROPER_TXT
        .lines()
        .filter(|l| !l.is_empty())
        .map(|w| (fold(w), w))
        .collect()
});

static A_AUX_NEXT: LazyLock<std::collections::HashSet<&'static str>> =
    LazyLock::new(|| A_AUX_NEXT_TXT.lines().filter(|l| !l.is_empty()).collect());

static A_AUX_PREV: LazyLock<std::collections::HashSet<&'static str>> =
    LazyLock::new(|| A_AUX_PREV_TXT.lines().filter(|l| !l.is_empty()).collect());

/// `folded → (forme accentuée, participe-dominant)`. Cf. `PARTICIPLES_TXT`.
static PARTICIPLES: LazyLock<HashMap<&'static str, (&'static str, bool)>> = LazyLock::new(|| {
    PARTICIPLES_TXT
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let mut it = l.split('\t');
            let folded = it.next().expect("participle: champ folded");
            let form = it.next().expect("participle: champ accentué");
            let strong = it.next() == Some("s");
            (folded, (form, strong))
        })
        .collect()
});

/// Entrée du modèle contextuel de participe (cf. `PARTICIPLE_CTX_TXT`). Tranche
/// entre deux lectures : la forme participe `-é` (`e_form`, ex. « arrêté ») et la
/// forme alternative (`alt_form`, ex. « arrête » ; vide ⇒ forme non accentuée).
struct PartCtx {
    e_form: &'static str,
    alt_form: &'static str,
    /// Décision par défaut : `true` = participe accentué (`e_form`).
    default_accent: bool,
    /// Voisins repliés (`p…`/`n…`) qui inversent la décision par défaut.
    flips: std::collections::HashSet<&'static str>,
}

static PARTICIPLE_CTX: LazyLock<HashMap<&'static str, PartCtx>> = LazyLock::new(|| {
    PARTICIPLE_CTX_TXT
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let mut it = l.splitn(5, '\t');
            let folded = it.next().expect("ctx: champ folded");
            let e_form = it.next().expect("ctx: champ participe");
            let alt_form = it.next().unwrap_or("");
            let default_accent = it.next() == Some("é");
            let flips = it.next().unwrap_or("").split_whitespace().collect();
            (
                folded,
                PartCtx {
                    e_form,
                    alt_form,
                    default_accent,
                    flips,
                },
            )
        })
        .collect()
});

/// Auxiliaires / copules (formes repliées) dont la présence juste avant un
/// participe homographe tranche en faveur du participe accentué (« a été
/// condamné », « est fondé », « ont formé »). `a` est traité à part (homographe
/// `a`/`à`) : on ne l'autorise que pour les formes participe-dominantes.
const AUX: &[&str] = &[
    "ont", "est", "sont", "ete", "etre", "etant", "ayant", "avait", "avaient", "avoir", "sera",
    "seront", "serait", "soit", "fut", "furent", "etait", "etaient", "etes", "soient", "seraient",
    "aient", "aura", "auront", "aurait", "avons", "avez", "suis", "sommes",
];

/// Acronymes préservés en capitales. Sélection volontairement étroite : uniquement
/// des formes qui ne sont jamais un mot français courant en minuscule (« SA »,
/// « CE », « PLU » collisionnent et sont exclus).
const ACRONYMS: &[&str] = &[
    "SARL", "SAS", "SASU", "SCI", "SNC", "SCP", "SELARL", "SELAS", "EURL", "SCEA", "GAEC", "GIE",
    "HLM", "SMIC", "TVA", "CSG", "CRDS", "RSA", "RMI", "CESEDA", "RGPD", "URSSAF", "CPAM", "CNAF",
    "ANAH", "CADA", "TGI", "TASS", "CPH", "JAF", "JLD", "JEX", "SNCF", "RATP", "EDF", "GDF",
    "INSEE", "OFPRA", "CNDA",
];

// Désambiguïsation de l'homographe « a » (auxiliaire avoir) / « à » (préposition),
// tranchée sur les mots voisins repliés (cf. `disambiguate_a`, `A_AUX_PREV`,
// `A_AUX_NEXT`) : défaut préposition, « a » seulement sur signal auxiliaire franc
// (sujet/initiale avant, prédicteur appris après). Mesuré sur la GT : ~97 % des « à ».

/// Têtes nominales temporelles/locatives qui précèdent l'adverbe relatif « où »
/// (« le jour où », « dans la mesure où », « au cas où ») ⇒ « où ». La
/// conjonction « ou » ne suit jamais ces noms.
const OU_PREV: &[&str] = &[
    "d",
    "etat",
    "jour",
    "moment",
    "cas",
    "mesure",
    "hypothese",
    "date",
    "lieu",
    "point",
    "instant",
    "epoque",
    "situation",
    "endroit",
    "fois",
    "periode",
    "heure",
    "stade",
    "lendemain",
];

/// Pronoms/verbes qui suivent l'adverbe relatif « où » introduisant une relative
/// (« d'où il résulte », « le jour où elle statuait », « où elles siégeaient ») ⇒
/// « où ». La conjonction « ou » + sujet est marginale en langage juridique.
const OU_NEXT: &[&str] = &[
    "il",
    "elle",
    "ils",
    "elles",
    "etaient",
    "siegeaient",
    "resident",
];

/// Déterminants que « dès » peut précéder (« dès le », « dès son ») mais que
/// l'article « des » ne précède jamais. Avec « lors »/« que » (« dès lors »,
/// « dès que ») ⇒ « dès ».
const DES_DET: &[&str] = &[
    "le", "la", "l", "les", "son", "sa", "ses", "leur", "leurs", "ce", "cet", "cette", "mon", "ma",
    "mes", "notre", "votre", "nos", "vos", "ton", "ta", "tes",
];

/// Replie une chaîne en clé de recherche : minuscule + suppression des
/// diacritiques français + ligatures (`œ→oe`, `æ→ae`). Utilisé identiquement pour
/// bâtir les tables et pour interroger un token — la cohérence est interne.
fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        for lc in c.to_lowercase() {
            match lc {
                'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => out.push('a'),
                'é' | 'è' | 'ê' | 'ë' => out.push('e'),
                'î' | 'ï' | 'í' | 'ì' => out.push('i'),
                'ô' | 'ö' | 'ó' | 'ò' | 'õ' => out.push('o'),
                'û' | 'ü' | 'ù' | 'ú' => out.push('u'),
                'ÿ' | 'ý' => out.push('y'),
                'ç' => out.push('c'),
                'œ' => out.push_str("oe"),
                'æ' => out.push_str("ae"),
                other => out.push(other),
            }
        }
    }
    out
}

/// Vrai si le texte est majoritairement en capitales (≥ 85 % des lettres) sur un
/// volume significatif — signature des vieilles décisions caps-lock. C'est le
/// portillon : on ne recasse QUE ces documents, jamais un texte déjà en casse
/// mixte (qu'on abîmerait).
pub fn is_caps_lock(text: &str) -> bool {
    let mut upper = 0usize;
    let mut letters = 0usize;
    for c in text.chars() {
        if c.is_alphabetic() {
            letters += 1;
            if c.is_uppercase() {
                upper += 1;
            }
        }
    }
    letters >= 200 && (upper as f64) / (letters as f64) >= 0.85
}

/// Recasse un texte caps-lock (multi-lignes). À n'appeler qu'après `is_caps_lock`.
pub fn truecase(text: &str) -> String {
    text.split('\n')
        .map(truecase_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_word_char(c: char) -> bool {
    c.is_alphabetic()
}

fn truecase_line(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    // Pré-passe : suite des mots repliés, pour donner à chaque mot le contexte de
    // ses voisins (désambiguïsation a/à).
    let folded = folded_word_seq(&chars);
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    let mut wi = 0usize; // ordinal du mot courant dans `folded`
                         // Début de phrase : la 1re lettre d'une ligne ou ce qui suit un « . ! ? ».
    let mut sentence_start = true;
    while i < chars.len() {
        if is_word_char(chars[i]) {
            let start = i;
            while i < chars.len() && is_word_char(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            // Initiale d'anonymisation : lettre seule suivie de « … » (« X... »).
            let followed_by_ellipsis = chars[i..].iter().take_while(|c| **c == '.').count() >= 3;
            let next_dot = chars.get(i) == Some(&'.');
            let prev = wi.checked_sub(1).map(|p| folded[p].as_str());
            let next = folded.get(wi + 1).map(String::as_str);
            out.push_str(&case_word(
                &word,
                sentence_start,
                followed_by_ellipsis,
                next_dot,
                prev,
                next,
            ));
            sentence_start = false;
            wi += 1;
        } else {
            let start = i;
            while i < chars.len() && !is_word_char(chars[i]) {
                i += 1;
            }
            let run = &chars[start..i];
            out.extend(run.iter());
            // Frontière de phrase sur « . ! ? » mais pas sur une ellipse (« ... »)
            // ni un « ; » (les attendus/moyens s'enchaînent en minuscule). Sinon on
            // conserve l'état (les ponctuations de tête gardent le début de ligne).
            if is_sentence_boundary(run) {
                sentence_start = true;
            }
        }
    }
    out
}

/// Suite des mots repliés (minuscule + ASCII) d'une ligne, dans l'ordre — sert de
/// contexte voisin (prev/next) à la désambiguïsation.
fn folded_word_seq(chars: &[char]) -> Vec<String> {
    let mut words = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if is_word_char(chars[i]) {
            let start = i;
            while i < chars.len() && is_word_char(chars[i]) {
                i += 1;
            }
            words.push(fold(&chars[start..i].iter().collect::<String>()));
        } else {
            i += 1;
        }
    }
    words
}

fn is_sentence_boundary(run: &[char]) -> bool {
    if run.contains(&'!') || run.contains(&'?') {
        return true;
    }
    // Un point, sauf s'il fait partie d'une ellipse (≥ 2 points consécutifs).
    let mut has_dot = false;
    let mut prev_dot = false;
    let mut in_ellipsis = false;
    for &c in run {
        if c == '.' {
            if prev_dot {
                in_ellipsis = true;
            }
            has_dot = true;
            prev_dot = true;
        } else {
            prev_dot = false;
        }
    }
    has_dot && !in_ellipsis
}

fn case_word(
    word: &str,
    sentence_start: bool,
    followed_by_ellipsis: bool,
    next_dot: bool,
    prev: Option<&str>,
    next: Option<&str>,
) -> String {
    // Initiale d'anonymisation « X... » : capitale conservée.
    if word.chars().count() == 1 && followed_by_ellipsis {
        return word.to_uppercase();
    }
    let upper = word.to_uppercase();
    if ACRONYMS.contains(&upper.as_str()) {
        return upper;
    }
    let lower = word.to_lowercase();
    // Civilités : « Mme/Mlle » telles quelles ; « M./MM. » seulement devant un
    // point (sinon « m » est ambigu). « me » écarté (pronom « me » trop fréquent).
    match lower.as_str() {
        "mme" => return "Mme".to_string(),
        "mmes" => return "Mmes".to_string(),
        "mlle" => return "Mlle".to_string(),
        "mlles" => return "Mlles".to_string(),
        "m" if next_dot => return "M".to_string(),
        "mm" if next_dot => return "MM".to_string(),
        _ => {}
    }
    let key = fold(&lower);
    // Nom propre (lieu/juridiction) : déjà capitalisé + accentué dans la table.
    // Homographe « a »/« à » : tranché par voisins (le filtre de dominance le
    // laisse hors lexique). Sinon, restauration d'accent par lexique.
    let base = if let Some(proper) = PROPER.get(&key) {
        (*proper).to_string()
    } else if key == "a" {
        disambiguate_a(prev, next).to_string()
    } else if let Some(form) = disambiguate_closed(&key, prev, next) {
        form.to_string()
    } else if let Some(ctx) = PARTICIPLE_CTX.get(key.as_str()) {
        // Tranche participe `-é` vs forme alternative (consulté avant le lexique :
        // gère les doubles homographes « arrête »/« arrêté » que le lexique fixait
        // à tort sur la lecture non-juridique).
        let flip = {
            let pk = format!("p{}", prev.unwrap_or("^"));
            let nk = format!("n{}", next.unwrap_or("$"));
            ctx.flips.contains(pk.as_str()) || ctx.flips.contains(nk.as_str())
        };
        if ctx.default_accent ^ flip {
            ctx.e_form.to_string()
        } else if ctx.alt_form.is_empty() {
            lower
        } else {
            ctx.alt_form.to_string()
        }
    } else if let Some(accented) = ACCENTS.get(&key) {
        (*accented).to_string()
    } else if let Some(&(form, strong)) = PARTICIPLES.get(key.as_str()) {
        if participle_fires(prev, strong) {
            form.to_string()
        } else {
            lower
        }
    } else {
        lower
    };
    let cased = if sentence_start {
        capitalize_first(&base)
    } else {
        base
    };
    // Invariant de longueur (offsets de spans de citation, lj-api decisions.rs) :
    // la recasse ne change JAMAIS le nombre de codepoints. Les seules formes des
    // lexiques qui le violent sont les ligatures œ/æ (clé foldée `oe`/`ae` → forme
    // affichée d'1 codepoint plus courte) : on retombe alors sur la minuscule
    // (recasée en début de phrase), de même longueur que l'entrée. La restauration
    // d'accents (é, à, ô…) est 1:1 et passe sans repli.
    if cased.chars().count() == word.chars().count() {
        cased
    } else {
        let lower = word.to_lowercase();
        if sentence_start {
            capitalize_first(&lower)
        } else {
            lower
        }
    }
}

/// Tranche « a » (auxiliaire) vs « à » (préposition) sur les mots voisins repliés.
/// Par défaut **préposition** (« à la », « à payer », « à M. », « à 5 000 euros ») :
/// l'auxiliaire « avoir » ne précède qu'un participe, jamais un déterminant, un
/// nom ou une quantité. On ne retient « a » que sur un signal auxiliaire franc :
/// sujet/initiale juste avant (« il a », « n'a », « M. B a »), ou participe/adverbe
/// juste après (« a été », « a condamné », « a légalement statué »).
fn disambiguate_a(prev: Option<&str>, next: Option<&str>) -> &'static str {
    if prev.is_some_and(|p| A_AUX_PREV.contains(p) || is_name_initial(p)) {
        return "a";
    }
    if next.is_some_and(a_followed_by_aux) {
        return "a";
    }
    "à"
}

/// Initiale d'anonymisation isolée (« M. B a formé ») : une seule lettre.
fn is_name_initial(w: &str) -> bool {
    let mut cs = w.chars();
    matches!((cs.next(), cs.next()), (Some(c), None) if c.is_alphabetic())
}

/// Vrai si le mot replié qui suit « a » trahit l'auxiliaire/le verbe « a » : prédicteur
/// appris (`A_AUX_NEXT` : « été », « pas », « lieu », « donc », participes fréquents…),
/// participe `-é` homographe sous le seuil (table contextuelle), ou adverbe intercalé
/// en `-ment` (« a légalement statué »).
fn a_followed_by_aux(n: &str) -> bool {
    A_AUX_NEXT.contains(n)
        || PARTICIPLE_CTX.contains_key(n)
        || (n.ends_with("ment") && n.chars().count() >= 6)
}

/// Tranche un participe homographe (« condamne »/« condamné ») non couvert par le
/// modèle contextuel : on n'accentue que si un auxiliaire le précède (« a été
/// condamné », « est fondé »). Le « a » nu (homographe `a`/`à`) n'est admis que
/// pour les formes participe-dominantes (`strong`), pour éviter « à titre »→« titré ».
fn participle_fires(prev: Option<&str>, strong: bool) -> bool {
    match prev {
        Some("a") => strong,
        Some(p) => AUX.contains(&p),
        None => false,
    }
}

/// Homographes fermés `ou`/`où`, `des`/`dès`, `du`/`dû` tranchés sur les voisins
/// repliés (signaux mesurés ≥ 95 % de pureté sur la GT recasse). Renvoie la forme
/// accentuée quand un signal franc se déclenche, sinon `None` (on garde la forme
/// non accentuée, par précision).
fn disambiguate_closed(key: &str, prev: Option<&str>, next: Option<&str>) -> Option<&'static str> {
    match key {
        // « le jour où », « dans la mesure où » : nom temporel/locatif juste avant.
        "ou" => {
            let is_ou = prev.is_some_and(|p| OU_PREV.contains(&p))
                || next.is_some_and(|n| OU_NEXT.contains(&n));
            is_ou.then_some("où")
        }
        // « dès lors », « dès que », « dès le/son… » : l'article « des » ne précède
        // jamais un déterminant ni « lors »/« que ».
        "des" => match next {
            Some("lors") | Some("que") => Some("dès"),
            Some(n) if DES_DET.contains(&n) => Some("dès"),
            _ => None,
        },
        // « aurait dû », « restant dû », « a dû être » : participe « dû ». On reste
        // étroit (l'article « du » précède aussi des noms en -er/-ir, « du premier
        // ressort », « du pouvoir » : pas de test d'infinitif).
        "du" => {
            let aux_before = matches!(
                prev,
                Some("restant" | "aurait" | "ont" | "avait" | "eut" | "eussent")
            );
            if aux_before || next == Some("etre") {
                Some("dû")
            } else {
                None
            }
        }
        _ => None,
    }
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_caps_lock_detects_full_caps_only() {
        let caps = "STATUANT SUR LE POURVOI, ".repeat(20);
        assert!(is_caps_lock(&caps));
        let mixed = "Statuant sur le pourvoi de la société. ".repeat(20);
        assert!(!is_caps_lock(&mixed));
        assert!(!is_caps_lock("TROP COURT"));
    }

    #[test]
    fn truecase_preserves_codepoint_length() {
        // Invariant critique : les offsets de spans de citation (codepoints sur le
        // texte original) sont appliqués sur le texte recasé (lj-api decisions.rs).
        // La recasse ne doit JAMAIS changer le nombre de codepoints. Les ligatures
        // œ/æ des lexiques (OEUVRE→œuvre, SCHOELCHER→Schœlcher) raccourciraient le
        // texte de 1 codepoint par occurrence et décaleraient les liens en aval —
        // repli sur la forme oe/ae pour tenir l'invariant. Restaurer les accents
        // (é, à…) reste 1:1 et autorisé.
        for s in [
            "VU L'OEUVRE ET L'ARTICLE 700 DU CODE DE PROCEDURE CIVILE",
            "TRIBUNAL DE SCHOELCHER, ARTICLE L. 761-1 DU CODE",
            "COMMUNE D'ANNOEULLIN CONTRE LE PREFET",
            "ARRET RENDU A HOENHEIM SUR LE FONDEMENT DE L'ARTICLE 9",
        ] {
            let out = truecase(s);
            assert_eq!(
                out.chars().count(),
                s.chars().count(),
                "longueur codepoints changée : {s:?} -> {out:?}"
            );
        }
    }

    #[test]
    fn restores_sentence_case_and_accents() {
        let input =
            "VU LE MEMOIRE PRODUIT ; SUR LE MOYEN PRIS DE LA VIOLATION DES ARTICLES DU CODE PENAL.";
        assert_eq!(
            truecase(input),
            "Vu le mémoire produit ; sur le moyen pris de la violation des articles du code pénal."
        );
    }

    #[test]
    fn capitalizes_after_sentence_dot_not_after_semicolon_or_ellipsis() {
        // « ; » n'ouvre pas une phrase ; « . » oui.
        assert_eq!(
            truecase("ATTENDU QUE ; QUE CECI."),
            "Attendu que ; que ceci."
        );
        // Ellipse d'anonymisation : pas de capitale derrière.
        assert_eq!(
            truecase("LE SIEUR X... ETAIT LA."),
            "Le sieur X... était la."
        );
    }

    #[test]
    fn preserves_anonymized_initials_and_acronyms() {
        assert_eq!(
            truecase("CONDAMNE LA SARL ET X..."),
            "Condamne la SARL et X..."
        );
    }

    #[test]
    fn protects_ambiguous_unaccented_words() {
        // « a » (sujet « il » avant), « sur », « du » : laissés intacts. « statué »
        // tranché par le modèle contextuel (auxiliaire avant) ; « dès la » par la
        // règle fermée (déterminant après « des ») ; « première » par le lexique.
        assert_eq!(
            truecase("IL A STATUE SUR LE FOND DU DROIT DES LA PREMIERE HEURE"),
            "Il a statué sur le fond du droit dès la première heure"
        );
    }

    #[test]
    fn disambiguates_a_preposition_vs_auxiliary() {
        // Préposition « à » : devant déterminant, infinitif, marqueur.
        assert_eq!(
            truecase("CONDAMNE A PAYER LA SOMME"),
            "Condamne à payer la somme"
        );
        assert_eq!(
            truecase("RENVOIE A LA COUR D'APPEL"),
            "Renvoie à la cour d'appel"
        );
        assert_eq!(
            truecase("FIXE A COMPTER DU JUGEMENT"),
            "Fixe à compter du jugement"
        );
        // Auxiliaire « a » (laissé tel quel) ; « statué » accentué par le modèle
        // contextuel (participe après auxiliaire).
        assert_eq!(
            truecase("LA COUR D'APPEL A STATUE"),
            "La cour d'appel a statué"
        );
        // « a » auxiliaire (sujet « il » avant) ; « violé » accentué par la règle
        // participe (auxiliaire avant, forme participe-dominante).
        assert_eq!(truecase("IL A VIOLE LE TEXTE"), "Il a violé le texte");
        assert_eq!(
            truecase("LA DEMANDE A ETE REJETEE"),
            "La demande a été rejetée"
        );
        // Piège « il a la » : le sujet l'emporte sur le déterminant suivant.
        assert_eq!(truecase("IL A LA FACULTE"), "Il a la faculté");
        // « à » devant bénéficiaire/quantité (jamais après l'auxiliaire « a »).
        assert_eq!(
            truecase("CONDAMNE A PAYER A M. DUPONT LA SOMME"),
            "Condamne à payer à M. Dupont la somme"
        );
    }

    #[test]
    fn restores_aux_participle_and_closed_homographs() {
        // Participe en -é restauré derrière un auxiliaire (« a condamné »,
        // « est fondé », « ont formé ») ; sans auxiliaire, on reste prudent.
        assert_eq!(
            truecase("LA COUR A CONDAMNE ET A PRONONCE LA RELAXE"),
            "La cour a condamné et a prononcé la relaxe"
        );
        assert_eq!(truecase("LE MOYEN EST FONDE"), "Le moyen est fondé");
        // Homographes fermés : « dès lors/que », « le jour où », « aurait dû ».
        assert_eq!(
            truecase("DES LORS QUE LA DEMANDE"),
            "Dès lors que la demande"
        );
        assert_eq!(
            truecase("LE JOUR OU LA DECISION A ETE PRISE"),
            "Le jour où la décision a été prise"
        );
        assert_eq!(
            truecase("LE JUGE AURAIT DU STATUER"),
            "Le juge aurait dû statuer"
        );
        // « du » article et « des » article restent intacts (pas de signal).
        assert_eq!(
            truecase("LE PRESIDENT DU TRIBUNAL ET DES PARTIES"),
            "Le président du tribunal et des parties"
        );
    }

    #[test]
    fn disambiguates_masc_participle_by_context() {
        // Participe adjectival après un nom-tête (patient) ⇒ accentué.
        assert_eq!(
            truecase("L'ARRET ATTAQUE A REJETE LA DEMANDE"),
            "L'arrêt attaqué a rejeté la demande"
        );
        // Verbe présent au dispositif (sujet-agent avant) ⇒ non accentué.
        assert_eq!(
            truecase("LE TRIBUNAL CONDAMNE LA SOCIETE"),
            "Le tribunal condamne la société"
        );
        // Passif (« a été condamné ») ⇒ participe.
        assert_eq!(
            truecase("IL A ETE CONDAMNE PAR LE JUGEMENT"),
            "Il a été condamné par le jugement"
        );
    }

    #[test]
    fn restores_place_names_from_gazetteer() {
        assert_eq!(
            truecase("LA COUR D'APPEL DE VERSAILLES ET LE TRIBUNAL DE NIMES"),
            "La cour d'appel de Versailles et le tribunal de Nîmes"
        );
    }

    #[test]
    fn handles_hyphen_and_apostrophe_segments() {
        assert_eq!(
            truecase("AU REZ-DE-CHAUSSEE DE L'IMMEUBLE"),
            "Au rez-de-chaussée de l'immeuble"
        );
    }
}
