//! Commandes de la liste de résultats, toutes deux à la durée de la session. La
//! vue des cartes reste hors de l'URL, qui n'a pas à imposer une façon de lire ;
//! le tri IA y reste, puisqu'il change la réponse.

use leptos::prelude::*;
use lj_dtos::{QueryMode, UiAction};

/// Vue de contenu d'une carte de résultat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardView {
    Snippet,
    Summary,
}

/// Choix manuel de vue, et la requête sous laquelle il a été fait. Porter la
/// requête est ce qui périme le choix : une nouvelle question retombe sur
/// l'auto, un changement de filtre, de tri ou de page le garde.
#[derive(Clone, Copy)]
pub struct CardViewPref(pub RwSignal<Option<(String, CardView)>>);

impl CardViewPref {
    /// Pins `view` on `query`, as the toggle does, for a search about to open.
    pub fn pin(self, query: String, view: CardView) {
        self.0.set(Some((query, view)));
    }
}

/// À appeler une fois dans `App` : la racine survit aux navigations, donc le
/// choix survit à un aller-retour vers une décision.
pub fn provide_card_view_pref() {
    provide_context(CardViewPref(RwSignal::new(None)));
}

fn pref() -> RwSignal<Option<(String, CardView)>> {
    use_context::<CardViewPref>()
        .map(|p| p.0)
        .unwrap_or_else(|| RwSignal::new(None))
}

/// Lexical opens on the snippet, the passage that answered the operators.
/// Reference has no matched passage (`synthetic_title_only`), so its snippet is
/// the head of the text and only the summary tells its carriers apart.
fn auto_view(mode: Option<QueryMode>) -> CardView {
    match mode {
        Some(QueryMode::Hybrid) | Some(QueryMode::Reference) => CardView::Summary,
        _ => CardView::Snippet,
    }
}

/// Vue par défaut effective des cartes : le choix manuel s'il porte sur la
/// requête courante, sinon l'auto du mode résolu.
pub fn effective_card_view(
    query: Signal<String>,
    mode: Signal<Option<QueryMode>>,
) -> Signal<CardView> {
    let pref = pref();
    Signal::derive(move || {
        let current = query.get();
        match pref.get() {
            Some((pinned_for, view)) if pinned_for == current => view,
            _ => auto_view(mode.get()),
        }
    })
}

/// Commandes de la liste : vue des cartes et tri IA sur une ligne, `children` à
/// leur suite (le tri, sur mobile). `spread` écarte les deux aux bords du rail.
#[component]
pub fn ViewControls(
    #[prop(into)] query: Signal<String>,
    /// Last resolved mode: it holds the page's shape, so it survives a refetch.
    #[prop(into)]
    mode: Signal<Option<QueryMode>>,
    /// The same, `None` while a response is in flight: it asserts, so it greys
    /// on nothing rather than on the previous query's mode.
    #[prop(into)]
    served: Signal<Option<QueryMode>>,
    #[prop(into)] rerank: Signal<bool>,
    #[prop(into)] on_rerank: Callback<bool>,
    #[prop(into)] reranked: Signal<Option<bool>>,
    #[prop(optional)] spread: bool,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = if spread {
        "flex flex-wrap items-center justify-between gap-2"
    } else {
        "flex flex-wrap items-center gap-2"
    };
    view! {
        <div class=class data-usage=UiAction::SearchView.as_str()>
            <CardViewToggle query=query mode=mode />
            <RerankToggle active=rerank on_toggle=on_rerank mode=served reranked=reranked />
            {children.map(|c| c())}
        </div>
    }
}

/// Bascule Extrait / Résumé. L'auto n'est pas une position du bouton : c'est ce
/// qu'on a tant qu'on n'y a pas touché.
#[component]
fn CardViewToggle(
    #[prop(into)] query: Signal<String>,
    #[prop(into)] mode: Signal<Option<QueryMode>>,
) -> impl IntoView {
    let pref = pref();
    let current = effective_card_view(query, mode);
    let ui = &crate::i18n::use_ui().search;
    let pin = move |view: CardView| {
        pref.set(Some((query.get_untracked(), view)));
    };

    view! {
        <div
            role="group"
            aria-label=ui.card_content
            class="flex items-center gap-0.5 rounded border border-[var(--color-rule)] p-0.5"
        >
            <SegmentButton
                label=ui.excerpt
                selected=Signal::derive(move || current.get() == CardView::Snippet)
                on_select=Callback::new(move |_| pin(CardView::Snippet))
            />
            <SegmentButton
                label=ui.summary
                selected=Signal::derive(move || current.get() == CardView::Summary)
                on_select=Callback::new(move |_| pin(CardView::Summary))
            />
        </div>
    }
}

/// Segment d'une bascule : même grammaire visuelle que les boutons de vue au
/// pied des cartes, pour qu'on lise les deux comme une seule commande.
#[component]
fn SegmentButton(
    label: &'static str,
    #[prop(into)] selected: Signal<bool>,
    #[prop(into)] on_select: Callback<()>,
) -> impl IntoView {
    let class = move || {
        crate::helpers::cn([
            "flex-1 rounded px-2 py-0.5 text-xs font-medium transition-colors",
            if selected.get() {
                "bg-[var(--color-accent)] text-[var(--color-accent-foreground)]"
            } else {
                "text-[var(--color-ink-subtle)] hover:text-[var(--color-ink)]"
            },
        ])
    };
    view! {
        <button
            type="button"
            aria-pressed=move || selected.get().to_string()
            on:click=move |_| on_select.run(())
            class=class
        >
            {label}
        </button>
    }
}

/// Greying is visual only: the toggle stays live and its position keeps showing
/// the setting, which the next semantic search honours. Disabling it would hide
/// a state the caller owns and cannot otherwise reach. It is the only thing on
/// the page saying whether the LLM sorted, so it greys on the server's answer
/// too, not on the three conditions the front can predict.
#[component]
fn RerankToggle(
    #[prop(into)] active: Signal<bool>,
    #[prop(into)] on_toggle: Callback<bool>,
    #[prop(into)] mode: Signal<Option<QueryMode>>,
    #[prop(into)] reranked: Signal<Option<bool>>,
) -> impl IntoView {
    let query_map = leptos_router::hooks::use_query_map();
    let par_date = Signal::derive(move || {
        matches!(
            query_map.get().get("sort").as_deref(),
            Some("date_desc") | Some("date_asc")
        )
    });
    let hors_mode = Signal::derive(move || matches!(mode.get(), Some(m) if m != QueryMode::Hybrid));
    let renonce = Signal::derive(move || active.get() && reranked.get() == Some(false));
    let sans_effet = Signal::derive(move || hors_mode.get() || par_date.get() || renonce.get());

    let class = move || {
        crate::helpers::cn([
            "flex items-center gap-2 rounded border px-2 py-1 text-xs transition-all",
            if sans_effet.get() {
                "border-[var(--color-rule)] text-[var(--color-ink-subtle)] opacity-50 hover:opacity-100"
            } else {
                "border-[var(--color-rule)] text-[var(--color-ink)] hover:border-[var(--color-ink)]"
            },
        ])
    };
    let track = move || {
        crate::helpers::cn([
            "relative h-3.5 w-6 rounded-full transition-colors",
            if active.get() {
                "bg-[var(--color-accent)]"
            } else {
                "bg-[var(--color-rule)]"
            },
        ])
    };
    let knob = move || {
        crate::helpers::cn([
            "absolute top-0.5 h-2.5 w-2.5 rounded-full bg-[var(--color-parchment)] transition-all",
            if active.get() { "left-3" } else { "left-0.5" },
        ])
    };
    let ui = &crate::i18n::use_ui().search;
    let title = move || {
        if par_date.get() {
            ui.ai_sort_by_date
        } else if hors_mode.get() {
            ui.ai_sort_other_mode
        } else if renonce.get() {
            ui.ai_sort_declined
        } else if active.get() {
            ui.ai_sort_on
        } else {
            ui.ai_sort_off
        }
    };

    view! {
        <button
            type="button"
            role="switch"
            aria-checked=move || active.get().to_string()
            on:click=move |_| on_toggle.run(!active.get_untracked())
            title=title
            class=class
        >
            <span>{ui.ai_sort}</span>
            <span aria-hidden="true" class=track>
                <span class=knob></span>
            </span>
        </button>
    }
}
