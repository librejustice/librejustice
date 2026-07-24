//! `ClientOnly` : rend ses enfants uniquement côté client (ADR 0063).
//!
//! Au SSR et au 1er rendu d'hydratation, rend le `fallback` (squelette statique) —
//! les deux coïncident donc l'hydratation du fallback ne diverge pas. Une fois le
//! document hydraté, bascule sur les enfants, montés *frais* : aucun HTML SSR à
//! réconcilier → aucun *hydration mismatch* possible pour le contenu
//! (interrupteurs `localStorage`, widgets dérivés de l'URL, etc.).
//!
//! Le gate est **global et à sens unique** (`Hydrated`, fourni par `App`), pas
//! par-instance : il ne protège que l'unique hydratation initiale du document.
//! Après bascule, un (re)montage via nav SPA — p. ex. retour arrière vers
//! `/decisions` — monte les enfants *directement*, sans re-flasher le squelette
//! (ces nœuds sont créés frais, jamais réconciliés → rien à éviter). Un gate
//! par-instance, lui, refaisait le fallback à chaque montage → blink à chaque
//! entrée sur la route.
//!
//! Généralise le pattern d'`AuthGuard` (qui fait déjà ce gating, conditionné à la
//! session). Réservé aux routes non-SEO : le contenu n'est pas dans le HTML SSR,
//! donc invisible aux crawlers.

use leptos::prelude::*;

/// Vrai une fois le document hydraté — une seule fois, jamais re-faux. Fourni au
/// contexte racine par `App` via [`provide_hydrated`] ; lu par `ClientOnly`.
#[derive(Clone, Copy)]
pub struct Hydrated(pub RwSignal<bool>);

/// À appeler une fois dans `App`. Fournit le signal [`Hydrated`] (`false` au SSR
/// et au 1er rendu client → les fallbacks `ClientOnly` hydratent à l'identique),
/// puis le bascule `true` via un `Effect` client après l'hydratation.
pub fn provide_hydrated() {
    let hydrated = RwSignal::new(false);
    provide_context(Hydrated(hydrated));
    #[cfg(feature = "hydrate")]
    Effect::new(move |_| hydrated.set(true));
}

/// Monte `children` une fois le document hydraté ; rend `fallback` avant.
#[component]
pub fn ClientOnly(
    children: ChildrenFn,
    /// Rendu au SSR + 1er rendu client (squelette statique, sans état réactif
    /// dépendant de `window`/`localStorage`, pour une hydratation sans divergence).
    #[prop(optional, into)]
    fallback: ViewFn,
) -> impl IntoView {
    let Hydrated(hydrated) = expect_context::<Hydrated>();
    let children = StoredValue::new(children);
    move || {
        if hydrated.get() {
            children.with_value(|c| c()).into_any()
        } else {
            fallback.run().into_any()
        }
    }
}
