//! Gazetteer CCN (ADR 0123) : snappe une forme citée « convention collective …»
//! sur une **vraie** convention collective du catalogue KALI (KALICONT), ou
//! renvoie `None`. PUR (règle #1) : la table des titres est embarquée
//! (`data/ccn_gazetteer.json`, régénérée depuis le fond KALI), la normalisation
//! vit ici.
//!
//! Motivation (cf. `docs/working-notes/2026-06-22-convention-collective-extraction-debt.md`) :
//! ~66 % des décisions citant un « convention collective » générique nomment en
//! fait une CCN précise. Les décisions la citent en **forme courte** (« …des
//! services de l'automobile ») ; le catalogue porte le **titre long** daté
//! (« Convention collective nationale des services de l'automobile du 15 janvier
//! 1981 »). On matche par **squelette de tokens distinctifs** (hors mots-outils,
//! dates, numéros) avec un seuil de containment + unicité, pour snapper sans
//! jamais mislinker (« Crédit agricole » ne se rabat pas sur « Crédit maritime »).

use std::collections::BTreeSet;
use std::sync::LazyLock;

use crate::data::ccn_gazetteer_raw;

/// Une convention collective du catalogue KALI.
#[derive(Debug, Clone)]
pub struct CcnEntry {
    /// `KALICONT…` — cible de résolution (text_uid du `legal_text`).
    pub kalicont: String,
    /// Titre canonique (forme catalogue).
    pub title: String,
    /// Tokens distinctifs du titre (squelette de matching).
    skeleton: BTreeSet<String>,
}

/// Mots-outils + têtes de famille CCN : présents dans (presque) tout titre de
/// convention, donc NON distinctifs — exclus du squelette.
const SKELETON_STOP: &[&str] = &[
    "convention",
    "collective",
    "collectives",
    "nationale",
    "nationales",
    "national",
    // « régionale »/« interrégionale » NE sont PAS mots-outils : ils
    // distinguent une CCN régionale d'une nationale homonyme (ouvriers du
    // bâtiment…) — gardés comme tokens distinctifs.
    "de",
    "des",
    "du",
    "la",
    "le",
    "les",
    "l",
    "d",
    "travail",
    "pour",
    "en",
    "et",
    "a",
    "au",
    "aux",
    "sur",
    "dans",
    "applicable",
    "applicables",
    "relative",
    "relatif",
    "concernant",
    "personnel",
    "personnels",
    "salaries",
    "entreprises",
    "etablissements",
];

const MONTHS: &[&str] = &[
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

/// Plie accents → ASCII, minuscule (port léger de la logique `fold` partagée).
fn fold_ascii(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            let lc = c.to_lowercase().next().unwrap_or(c);
            let folded = match lc {
                'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => 'a',
                'ç' => 'c',
                'è' | 'é' | 'ê' | 'ë' => 'e',
                'ì' | 'í' | 'î' | 'ï' => 'i',
                'ñ' => 'n',
                'ò' | 'ó' | 'ô' | 'ö' | 'õ' => 'o',
                'ù' | 'ú' | 'û' | 'ü' => 'u',
                'ý' | 'ÿ' => 'y',
                other => other,
            };
            Some(folded)
        })
        .collect()
}

/// Squelette : tokens distinctifs (≥2 chars, hors mots-outils, mois, années,
/// numéros). « Convention collective nationale des services de l'automobile du
/// 15 janvier 1981 » → {services, automobile}.
fn skeleton(raw: &str) -> BTreeSet<String> {
    let folded = fold_ascii(raw);
    folded
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 2)
        .filter(|t| !t.chars().all(|c| c.is_ascii_digit())) // années, numéros nus
        .filter(|t| !SKELETON_STOP.contains(t))
        .filter(|t| !MONTHS.contains(t))
        .map(|t| t.to_string())
        .collect()
}

/// Le gazetteer embarqué, squelettes pré-calculés.
pub struct CcnGazetteer {
    entries: Vec<CcnEntry>,
}

static GAZETTEER: LazyLock<CcnGazetteer> = LazyLock::new(|| {
    let raw = ccn_gazetteer_raw();
    let entries = raw
        .conventions
        .into_iter()
        .map(|c| CcnEntry {
            skeleton: skeleton(&c.title),
            kalicont: c.kalicont,
            title: c.title,
        })
        .filter(|e| !e.skeleton.is_empty())
        .collect();
    CcnGazetteer { entries }
});

/// Accès au gazetteer embarqué (singleton).
pub fn gazetteer() -> &'static CcnGazetteer {
    &GAZETTEER
}

impl CcnGazetteer {
    /// Snappe une forme citée « convention collective … » sur la CCN du
    /// catalogue, ou `None` si rien de sûr (précision d'abord : jamais de
    /// mislink). Critère :
    /// - le squelette cité a ≥2 tokens distinctifs (sinon trop ambigu — « …de la
    ///   métallurgie » nu ne tranche pas entre les CCN métallurgie) ;
    /// - tous ses tokens sont présents dans le candidat (containment = 1) ;
    /// - parmi les meilleurs (Jaccard max), si plusieurs squelettes DISTINCTS
    ///   sont ex æquo → ambiguïté entre CCN différentes → on s'abstient ; si les
    ///   ex æquo partagent le MÊME squelette (variantes datées/avenants d'une
    ///   même CCN), on prend un représentant déterministe (plus petit KALICONT) ;
    /// - garde anti-mislink : si le candidat ajoute >1 token distinctif au-delà
    ///   du cité ET que le Jaccard < 0,5 (« commerce de gros » happé par
    ///   « commerce de détail et de gros à prédominance alimentaire »), on
    ///   s'abstient.
    pub fn snap(&self, cited: &str) -> Option<&CcnEntry> {
        let q = skeleton(cited);
        if q.len() < 2 {
            return None;
        }
        let jac = |e: &CcnEntry| q.len() as f64 / e.skeleton.union(&q).count() as f64;
        let best_jac = self
            .entries
            .iter()
            .filter(|e| q.is_subset(&e.skeleton))
            .map(&jac)
            .fold(f64::NEG_INFINITY, f64::max);
        if !best_jac.is_finite() {
            return None;
        }
        let top: Vec<&CcnEntry> = self
            .entries
            .iter()
            .filter(|e| q.is_subset(&e.skeleton) && (jac(e) - best_jac).abs() <= 1e-9)
            .collect();
        // Ex æquo entre CCN DIFFÉRENTES (squelettes distincts) → abstention.
        let first = &top[0].skeleton;
        if top.iter().any(|e| &e.skeleton != first) {
            return None;
        }
        // Même CCN (variantes) → représentant déterministe.
        let chosen = top.iter().min_by(|a, b| a.kalicont.cmp(&b.kalicont))?;
        if chosen.skeleton.len() - q.len() > 1 && best_jac < 0.5 {
            return None;
        }
        Some(chosen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Les formes citées (gold du loop d'annotation) doivent snapper sur la bonne
    /// CCN — ou s'abstenir, jamais mislinker.
    #[test]
    fn snaps_cited_short_forms_to_real_ccn() {
        let g = gazetteer();
        // (forme citée, fragment attendu dans le titre snappé)
        let cases = [
            (
                "convention collective nationale des réseaux de transports publics urbains de voyageurs",
                "réseaux de transports publics urbains",
            ),
            (
                "convention collective nationale des ingénieurs et cadres de la métallurgie",
                "ingénieurs et cadres",
            ),
            (
                "convention collective nationale des activités du déchet",
                "activités du déchet",
            ),
            (
                "convention collective des Hôtels, Cafés et Restaurants",
                "hôtels",
            ),
            (
                "convention collective nationale de l'industrie du pétrole",
                "pétrole",
            ),
        ];
        for (cited, frag) in cases {
            let snapped = g.snap(cited);
            assert!(snapped.is_some(), "devrait snapper: {cited}");
            let title = snapped.unwrap().title.to_lowercase();
            assert!(
                fold_ascii(&title).contains(&fold_ascii(frag)),
                "{cited} → {title} (attendu fragment {frag})"
            );
        }
    }

    /// Sur-capture de prose et formes nues : aucun snap (pas de CCN identifiable).
    #[test]
    fn refuses_prose_overcapture_and_bare() {
        let g = gazetteer();
        for junk in [
            "convention collective et la possibilité expresse de se faire assister par le secrétaire",
            "convention collective à compter de janvier 2000 et un rappel de salaire",
            "convention collective",            // nu : 0 token distinctif
            "convention collective applicable",  // générique
            "convention collective de la métallurgie", // 1 token distinctif : ambigu
            // Garde anti-mislink : une CCN distincte au titre seulement
            // sur-ensemble (« commerce de gros » ⊂ « commerce de détail et de gros
            // à prédominance alimentaire ») ne doit PAS être happée.
            "convention collective du commerce de gros",
        ] {
            assert!(g.snap(junk).is_none(), "ne devrait PAS snapper: {junk}");
        }
    }
}
