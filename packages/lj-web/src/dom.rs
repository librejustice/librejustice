//! Helpers DOM par-dessus `web-sys`. Compile UNIQUEMENT sous `hydrate` (le
//! module est vide en SSR — les appels sont gatés `#[cfg(feature = "hydrate")]`
//! côté appelant, comme `spawn_local` dans `DecisionActions`).
#![cfg(feature = "hydrate")]

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

/// Écrit `text` dans le presse-papier. `true` au succès, `false` si refusé
/// (permission, document sans focus, contexte non sécurisé).
pub async fn copy_text(text: &str) -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    JsFuture::from(window.navigator().clipboard().write_text(text))
        .await
        .is_ok()
}

// ── Géométrie de la sélection de texte ──────────────────────────────────────

#[wasm_bindgen]
extern "C" {
    /// Vue sur une `Selection` pour y appeler `toString()` : web-sys ne génère
    /// pas cette méthode, seule de l'interface à manquer, et la règle
    /// d'orphelin interdit de l'ajouter au type importé.
    type SelectionTexte;

    /// `Selection.prototype.toString()` : le texte sélectionné, tel que rendu.
    #[wasm_bindgen(method, js_name = toString)]
    fn texte(this: &SelectionTexte) -> String;
}

/// Text of the current selection when it sits in `article`, with the top, the
/// bottom and the horizontal centre of its rect. `None` on a degenerate rect.
fn mesurer(article: &web_sys::Element) -> Option<(String, f64, f64, f64)> {
    let selection = web_sys::window()?.get_selection().ok().flatten()?;
    if selection.range_count() == 0 || selection.is_collapsed() {
        return None;
    }
    let range = selection.get_range_at(0).ok()?;
    let ancetre = range.common_ancestor_container().ok()?;
    let ancre = if ancetre.node_type() == web_sys::Node::ELEMENT_NODE {
        ancetre.dyn_into::<web_sys::Element>().ok()?
    } else {
        ancetre.parent_element()?
    };
    // Hors de l'article, ou dans une hover card (panneau flottant rendu DANS
    // l'article) : pas de toolbar sur ces sélections.
    if !article.contains(Some(&ancre))
        || ancre.closest("[data-hover-card]").ok().flatten().is_some()
    {
        return None;
    }
    let rect = range.get_bounding_client_rect();
    if rect.width() == 0.0 && rect.height() == 0.0 {
        return None;
    }
    Some((
        selection.unchecked_ref::<SelectionTexte>().texte(),
        rect.top(),
        rect.bottom(),
        rect.left() + rect.width() / 2.0,
    ))
}

/// Suit la sélection dans `article` et rappelle `on_change` à chaque changement,
/// avec `None` quand il n'y a rien à afficher. Le garde rendu retire les trois
/// écouteurs à son `Drop`.
pub fn observe_selection(
    article: web_sys::Element,
    mut on_change: impl FnMut(Option<(String, f64, f64, f64)>) + 'static,
) -> Option<SelectionSubscription> {
    let window = web_sys::window()?;
    let document = window.document()?;
    let closure = Closure::<dyn FnMut()>::new(move || on_change(mesurer(&article)));
    let f = closure.as_ref().unchecked_ref();

    let _ = document.add_event_listener_with_callback("selectionchange", f);
    // Capture : un défilement dans un conteneur interne déplace la sélection
    // sans faire remonter d'événement jusqu'à `window` en phase de bulle.
    let options = web_sys::AddEventListenerOptions::new();
    options.set_capture(true);
    let _ = window
        .add_event_listener_with_callback_and_add_event_listener_options("scroll", f, &options);
    let _ = window.add_event_listener_with_callback("resize", f);

    Some(SelectionSubscription {
        closure,
        window,
        document,
    })
}

/// Garde d'abonnement : conserve le closure vivant et retire les écouteurs.
pub struct SelectionSubscription {
    closure: Closure<dyn FnMut()>,
    window: web_sys::Window,
    document: web_sys::Document,
}

impl Drop for SelectionSubscription {
    fn drop(&mut self) {
        let f = self.closure.as_ref().unchecked_ref();
        let _ = self
            .document
            .remove_event_listener_with_callback("selectionchange", f);
        let options = web_sys::EventListenerOptions::new();
        options.set_capture(true);
        let _ = self
            .window
            .remove_event_listener_with_callback_and_event_listener_options("scroll", f, &options);
        let _ = self.window.remove_event_listener_with_callback("resize", f);
    }
}

/// Vide la sélection courante.
pub fn clear_selection() {
    if let Some(selection) = web_sys::window().and_then(|w| w.get_selection().ok().flatten()) {
        let _ = selection.remove_all_ranges();
    }
}
