//! Barre de décision sticky : contexte partagé (port de `DecisionBarProvider` /
//! `useDecisionBar`) + passage d'état recherche→décision.
//!
//! `leptos_router` n'a pas d'équivalent riche à `location.state` (React Router).
//! On le remplace par deux contextes réactifs fournis au shell :
//! - `DecisionBarSignal` : ce que la `TopBar` affiche (titre, nav, retour).
//! - `ResultNavSignal` : « graine » posée au clic par une carte résultat / une
//!   voisine / le widget prev-next, lue UNE fois par `DecisionHeader` au montage
//!   pour construire la barre. One-shot (consommée au montage) : un lien décision
//!   sans graine (activité, lien direct) ne réutilise pas une nav obsolète.

use leptos::prelude::*;

/// Origine « recherche » d'une consultation : permet au bouton retour de revenir
/// à la liste exacte (query string) et de restaurer le scroll. Port de
/// `fromSearch` (`{pathname:"/decisions", search, scrollY}` ; pathname implicite).
#[derive(Clone, Debug, PartialEq)]
pub struct FromSearch {
    /// Query string complète (`?q=…&page=…`), telle quelle.
    pub search: String,
    /// Position de scroll à restaurer au retour (px).
    pub scroll_y: f64,
}

/// Navigation inter-résultats. Port de `DecisionNavState` : `position`/`total`
/// pour l'affichage, `hit_ids` pour dériver précédent/suivant à l'`id` courant.
#[derive(Clone, Debug, PartialEq)]
pub struct ResultNav {
    pub position: i64,
    pub total: i64,
    pub hit_ids: Vec<String>,
}

/// État affiché par la barre sticky. Port de `DecisionBarState`.
#[derive(Clone, Debug, PartialEq)]
pub struct DecisionBarState {
    pub title: String,
    pub id: String,
    pub nav: Option<ResultNav>,
    pub from_search: Option<FromSearch>,
}

/// Graine posée au clic, consommée par `DecisionHeader`. Port du `state` passé à
/// `navigate(...)` (resultNav + fromSearch).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResultNavSeed {
    pub nav: Option<ResultNav>,
    pub from_search: Option<FromSearch>,
}

pub type DecisionBarSignal = RwSignal<Option<DecisionBarState>>;
pub type ResultNavSignal = RwSignal<Option<ResultNavSeed>>;
/// Graines consommées, par id de décision. Filet du back/forward navigateur :
/// décision → article de loi → retour arrière remonte la page SANS graine (le
/// one-shot a été consommé à la première visite) — sans mémoire, la barre
/// perdait l'origine recherche (« Résultats » redevenait « Recherche ») et le
/// prev/next. Clé = id : une décision atteinte sans graine ne récupère que SON
/// dernier contexte, jamais celui d'une autre (le danger que le one-shot pare).
pub type SeedMemorySignal = StoredValue<std::collections::HashMap<String, ResultNavSeed>>;
/// Position de scroll à restaurer sur la page résultats au retour depuis une
/// décision. Posée par le bouton retour de la barre décision, consommée UNE fois
/// par `ResultsBody` à son montage, qui défère l'application au `requestAnimationFrame`
/// (après le layout de la liste, avant le premier paint) → la page peint
/// directement à la bonne position, sans saut.
pub type RestoreScrollSignal = RwSignal<Option<f64>>;

/// Fournit les signaux au contexte (appelé par `AppShell`, ancêtre de la
/// `TopBar` et des pages routées — donc persistant à travers les navigations).
pub fn provide_decision_bar_contexts() {
    provide_context::<DecisionBarSignal>(RwSignal::new(None));
    provide_context::<ResultNavSignal>(RwSignal::new(None));
    provide_context::<RestoreScrollSignal>(RwSignal::new(None));
    provide_context::<SeedMemorySignal>(StoredValue::new(std::collections::HashMap::new()));
}

/// Signal de la barre décision (ce que la `TopBar` lit).
pub fn use_decision_bar() -> DecisionBarSignal {
    expect_context::<DecisionBarSignal>()
}

/// Signal de la graine de navigation (posé au clic, consommé au montage).
pub fn use_result_nav() -> ResultNavSignal {
    expect_context::<ResultNavSignal>()
}

/// Signal du scroll à restaurer (posé par le bouton retour, consommé au montage
/// de la liste de résultats).
pub fn use_restore_scroll() -> RestoreScrollSignal {
    expect_context::<RestoreScrollSignal>()
}

/// Mémoire des graines consommées (par id de décision).
pub fn use_seed_memory() -> SeedMemorySignal {
    expect_context::<SeedMemorySignal>()
}
