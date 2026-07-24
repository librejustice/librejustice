//! En-tête décision + barre d'actions. Port de `decision-header.tsx`.
//!
//! `DecisionHeader` alimente la barre sticky (`components::decision_bar`) côté
//! client, en consommant la graine de navigation posée par la carte résultat /
//! la voisine (origine recherche + prev/next ; SSR : aucune barre, parité
//! `useLayoutEffect`).

use leptos::prelude::*;
use lj_dtos::DecisionDetail;

use crate::pages::decision_page::reference::build_decision_references;

#[cfg(feature = "hydrate")]
#[cfg(feature = "hydrate")]
use crate::api::ApiClient;
#[cfg(feature = "hydrate")]
use crate::components::decision_bar::{
    use_decision_bar, use_result_nav, use_seed_memory, DecisionBarState,
};

/// Pousse les métadonnées dans la barre sticky (port de `setBar`/cleanup).
#[component]
pub fn DecisionHeader(detail: DecisionDetail) -> impl IntoView {
    #[cfg(feature = "hydrate")]
    {
        let title = build_decision_references(&detail).heading;
        let id = detail.id.clone();
        let bar = use_decision_bar();
        let seed = use_result_nav();
        let memory = use_seed_memory();
        Effect::new(move |_| {
            // Consomme la graine (one-shot) en la mémorisant par id ; sans
            // graine (back navigateur depuis un article de loi, lien direct,
            // activité), on restaure le dernier contexte DE CETTE décision —
            // jamais celui d'une autre. Aucun contexte connu → barre sans nav
            // (titre + retour « Recherche »).
            let consumed = match seed.get_untracked() {
                Some(s) => {
                    memory.update_value(|m| {
                        m.insert(id.clone(), s.clone());
                    });
                    s
                }
                None => memory
                    .with_value(|m| m.get(&id).cloned())
                    .unwrap_or_default(),
            };
            seed.set(None);
            bar.set(Some(DecisionBarState {
                title: title.clone(),
                id: id.clone(),
                nav: consumed.nav,
                from_search: consumed.from_search,
            }));
        });
        on_cleanup(move || bar.set(None));
    }
    #[cfg(not(feature = "hydrate"))]
    let _ = &detail;
}

/// Barre d'actions (référence, signet, impression, PDF). Rendue dans
/// `DecisionSimilar`. Port de `DecisionActions`.
#[component]
pub fn DecisionActions(detail: DecisionDetail) -> impl IntoView {
    let references = build_decision_references(&detail);
    let pdf_href = format!("/api/decision/{}/download.pdf", detail.id);
    let pdf_filename = references.filename.clone();
    let decision_id = detail.id.clone();

    let on_print = move |_| {
        #[cfg(feature = "hydrate")]
        {
            let _ = leptos::prelude::window().print();
        }
    };

    view! {
        <div class="flex flex-col gap-2">
            <div class="flex flex-nowrap items-center gap-2">
                <CopyReferenceButton full=references.full.clone() short=references.short.clone() />
                <BookmarkButton decision_id=decision_id />
                <button
                    type="button"
                    on:click=on_print
                    class="inline-flex shrink-0 items-center gap-2 rounded-sm border border-[var(--color-rule)] px-2.5 py-1.5 text-xs text-[var(--color-ink-muted)] transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)]"
                    aria-label="Impression"
                    title="Impression"
                >
                    <PrintIcon />
                </button>
                <a
                    href=pdf_href
                    download=pdf_filename
                    class="inline-flex shrink-0 items-center gap-2 rounded-sm border border-[var(--color-rule)] px-2.5 py-1.5 text-xs text-[var(--color-ink-muted)] no-underline transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)]"
                    aria-label="PDF"
                    title="PDF"
                >
                    <PdfIcon />
                </a>
            </div>
        </div>
    }
    .into_any()
}

/// Bouton « signet », gated par l'auth. Port de `BookmarkButton` : le bouton est
/// masqué tant que la session n'est pas résolue (token absent au SSR), et masqué
/// si l'enregistrement d'activité est coupé (mode ZDR, ADR 0056). Bascule
/// optimiste add/remove avec rollback en erreur.
#[component]
fn BookmarkButton(decision_id: String) -> impl IntoView {
    let has_token = RwSignal::new(None::<bool>);
    // `None` = profil non chargé (affiché, comme React `profile === undefined`).
    let track_activity = RwSignal::new(None::<bool>);
    let is_on = RwSignal::new(false);
    let busy = RwSignal::new(false);
    // `StoredValue` (Copy) : `toggle` est rejoué par `<Show>` (closure `Fn`), il
    // ne peut pas capturer la `String` par move.
    #[cfg(feature = "hydrate")]
    let did = StoredValue::new(decision_id);
    #[cfg(not(feature = "hydrate"))]
    let _ = decision_id;

    #[cfg(feature = "hydrate")]
    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            let token = crate::auth::get_access_token().await;
            has_token.set(Some(token.is_some()));
            if token.is_some() {
                let client = ApiClient::from_context();
                if let Ok(profile) = client.fetch_me().await {
                    track_activity.set(Some(profile.track_activity));
                }
                if let Ok(bookmarks) = client.list_bookmarks().await {
                    let id = did.get_value();
                    is_on.set(bookmarks.items.iter().any(|b| b.id == id));
                }
            }
        });
    });

    let toggle = move |_| {
        #[cfg(feature = "hydrate")]
        {
            if busy.get_untracked() {
                return;
            }
            let id = did.get_value();
            let currently = is_on.get_untracked();
            busy.set(true);
            is_on.set(!currently);
            leptos::task::spawn_local(async move {
                let client = ApiClient::from_context();
                let res = if currently {
                    client.remove_bookmark(&id).await
                } else {
                    client.add_bookmark(&id).await
                };
                if res.is_err() {
                    is_on.set(currently);
                }
                busy.set(false);
            });
        }
    };

    let label = move || {
        if is_on.get() {
            "Retirer des signets"
        } else {
            "Ajouter aux signets"
        }
    };

    view! {
        <Show when=move || {
            has_token.get() == Some(true) && track_activity.get() != Some(false)
        }>
            <button
                type="button"
                on:click=toggle
                disabled=move || busy.get()
                aria-pressed=move || is_on.get().to_string()
                aria-label=label
                title=label
                class="inline-flex shrink-0 items-center gap-2 rounded-sm border border-[var(--color-rule)] px-2.5 py-1.5 text-xs text-[var(--color-ink-muted)] transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)] disabled:opacity-50"
            >
                <BookmarkIcon filled=is_on />
            </button>
        </Show>
    }
}

/// Bouton « Référence » + menu déroulant (copier complète / abrégée).
/// Port de `CopyReferenceButton` : copie via `dom::copy_text`, retour « Copié »
/// 1,8 s, fermeture au clic extérieur (`pointerdown` document).
#[component]
fn CopyReferenceButton(full: String, short: String) -> impl IntoView {
    let open = RwSignal::new(false);
    // Quelle référence vient d'être copiée (`"full"`/`"short"`), `None` sinon.
    let copied = RwSignal::new(None::<&'static str>);
    let container_ref = NodeRef::<leptos::html::Div>::new();

    // Fermeture au clic extérieur (port du listener `pointerdown` document).
    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::JsCast;
        let handle = window_event_listener(leptos::ev::pointerdown, move |ev| {
            if !open.get_untracked() {
                return;
            }
            let inside = container_ref
                .get_untracked()
                .zip(ev.target())
                .and_then(|(el, target)| {
                    target
                        .dyn_ref::<web_sys::Node>()
                        .map(|node| el.contains(Some(node)))
                })
                .unwrap_or(false);
            if !inside {
                open.set(false);
            }
        });
        on_cleanup(move || handle.remove());
    }

    let copy = move |value: String, kind: &'static str| {
        open.set(false);
        #[cfg(feature = "hydrate")]
        leptos::task::spawn_local(async move {
            if crate::dom::copy_text(&value).await {
                copied.set(Some(kind));
                leptos::prelude::set_timeout(
                    move || copied.set(None),
                    std::time::Duration::from_millis(1800),
                );
            }
        });
        #[cfg(not(feature = "hydrate"))]
        let _ = (value, kind);
    };

    let chevron_class = move || {
        format!(
            "h-3 w-3 transition-transform{}",
            if open.get() { " rotate-180" } else { "" }
        )
    };

    let full_copy = full.clone();
    let short_copy = short.clone();

    view! {
        <div node_ref=container_ref class="relative">
            <button
                type="button"
                on:click=move |_| open.update(|o| *o = !*o)
                aria-expanded=move || open.get().then_some("true")
                aria-haspopup="menu"
                aria-live="polite"
                class="inline-flex shrink-0 items-center gap-2 rounded-sm border border-[var(--color-rule)] px-3 py-1.5 text-xs text-[var(--color-ink-muted)] transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)]"
            >
                {move || {
                    if copied.get().is_some() {
                        view! {
                            <CheckIcon />
                            " Copié"
                        }
                            .into_any()
                    } else {
                        view! {
                            "Référence "
                            <svg aria-hidden="true" viewBox="0 0 12 12" class=chevron_class>
                                <path
                                    d="M2.25 4.5L6 8.25 9.75 4.5"
                                    fill="none"
                                    stroke="currentColor"
                                    stroke-width="1.4"
                                    stroke-linecap="round"
                                    stroke-linejoin="round"
                                />
                            </svg>
                        }
                            .into_any()
                    }
                }}
            </button>
            <Show when=move || open.get()>
                <div class="absolute left-0 top-full z-30 mt-2 min-w-72 rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] p-2 shadow-lg">
                    <CopyMenuButton
                        label="Copier la référence complète"
                        value=full.clone()
                        copied=Signal::derive(move || copied.get() == Some("full"))
                        on_copy=Callback::new({
                            let full_copy = full_copy.clone();
                            move |_| copy(full_copy.clone(), "full")
                        })
                    />
                    <CopyMenuButton
                        label="Copier la référence abrégée"
                        value=short.clone()
                        copied=Signal::derive(move || copied.get() == Some("short"))
                        on_copy=Callback::new({
                            let short_copy = short_copy.clone();
                            move |_| copy(short_copy.clone(), "short")
                        })
                    />
                </div>
            </Show>
        </div>
    }
    .into_any()
}

#[component]
fn CopyMenuButton(
    label: &'static str,
    value: String,
    #[prop(into)] copied: Signal<bool>,
    on_copy: Callback<()>,
) -> impl IntoView {
    view! {
        <button
            type="button"
            on:click=move |_| on_copy.run(())
            class="flex w-full flex-col items-start gap-1 rounded-sm px-3 py-2 text-left transition-colors hover:bg-[var(--color-vellum)]"
        >
            <span class="text-sm text-[var(--color-ink)]">
                {move || if copied.get() { "Copié" } else { label }}
            </span>
            <span class="text-sm text-[var(--color-ink-subtle)]">{value}</span>
        </button>
    }
}

#[component]
fn BookmarkIcon(#[prop(into)] filled: Signal<bool>) -> impl IntoView {
    view! {
        <svg aria-hidden="true" viewBox="0 0 16 16" class="h-3.5 w-3.5">
            <path
                d="M4 2.5h8v11l-4-2.5-4 2.5z"
                fill=move || if filled.get() { "currentColor" } else { "none" }
                stroke="currentColor"
                stroke-width="1.3"
                stroke-linejoin="round"
            />
        </svg>
    }
}

#[component]
fn CheckIcon() -> impl IntoView {
    view! {
        <svg aria-hidden="true" viewBox="0 0 16 16" class="h-3.5 w-3.5">
            <path
                d="M3 8.5l3.5 3.5L13 5"
                fill="none"
                stroke="currentColor"
                stroke-width="1.6"
                stroke-linecap="round"
                stroke-linejoin="round"
            />
        </svg>
    }
}

#[component]
fn PrintIcon() -> impl IntoView {
    view! {
        <svg aria-hidden="true" viewBox="0 0 16 16" class="h-3.5 w-3.5">
            <path
                d="M4 5V2.5h8V5M4 11h8v2.5H4zM3 6.5h10a1.5 1.5 0 011.5 1.5v2A1.5 1.5 0 0113 11.5H3A1.5 1.5 0 011.5 10v-2A1.5 1.5 0 013 6.5z"
                fill="none"
                stroke="currentColor"
                stroke-width="1.3"
                stroke-linejoin="round"
            />
            <circle cx="11.75" cy="8.75" r=".75" fill="currentColor" />
        </svg>
    }
}

#[component]
fn PdfIcon() -> impl IntoView {
    view! {
        <svg aria-hidden="true" viewBox="0 0 16 16" class="h-3.5 w-3.5">
            <path
                d="M4 1.75h5.5L13 5.25v8A1.75 1.75 0 0111.25 15h-7.5A1.75 1.75 0 012 13.25v-9.5A1.75 1.75 0 013.75 2z"
                fill="none"
                stroke="currentColor"
                stroke-width="1.3"
                stroke-linejoin="round"
            />
            <path d="M9.5 1.75v3.5H13" fill="none" stroke="currentColor" stroke-width="1.3" />
            <path
                d="M4.5 11h7"
                fill="none"
                stroke="currentColor"
                stroke-width="1.3"
                stroke-linecap="round"
            />
        </svg>
    }
}
