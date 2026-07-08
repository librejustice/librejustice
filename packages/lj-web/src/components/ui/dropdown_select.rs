//! `DropdownSelect` (port de `ui/dropdown-select.tsx`). Bouton + panneau
//! flottant custom, fermeture au clic exterieur.

use leptos::prelude::*;

use super::SelectOption;

/// Dropdown custom controle. `value` courant, `on_change` nouvelle valeur,
/// `options` la liste, `aria_label` le libelle accessible du bouton.
///
/// Panneau positionne en `position: fixed` (coordonnees mesurees a l'ouverture)
/// plutot qu'en `absolute` : il echappe ainsi a tout ancetre `overflow-hidden`
/// (ex. l'accordeon `FilterGroup` du rail, qui rognait sinon la liste au bas du
/// calendrier). En contrepartie un panneau fixe ne suit pas le scroll de page :
/// on le ferme au scroll (le scroll interne du panneau, cape + `overscroll-contain`,
/// ne remonte pas a la fenetre, donc ne le ferme pas).
#[component]
pub fn DropdownSelect(
    #[prop(into)] value: Signal<String>,
    #[prop(into)] on_change: Callback<String>,
    #[prop(into)] options: Signal<Vec<SelectOption>>,
    #[prop(optional, into)] aria_label: String,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let container_ref = NodeRef::<leptos::html::Div>::new();
    let button_ref = NodeRef::<leptos::html::Button>::new();
    // Coordonnees viewport du panneau `(top, right, min_width, max_height)`
    // mesurees a l'ouverture. `None` au SSR / avant mesure (masque tant que ferme).
    let coords = RwSignal::new(None::<(f64, f64, f64, f64)>);

    // Fermeture au clic exterieur (port du listener `mousedown` document) +
    // fermeture au scroll (le panneau fixe se detacherait du bouton sinon).
    // `window_event_listener` est inerte cote SSR (pas de window) — ilot client.
    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::JsCast;
        let handle = window_event_listener(leptos::ev::mousedown, move |ev| {
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

        let scroll_handle = window_event_listener(leptos::ev::scroll, move |_| {
            if open.get_untracked() {
                open.set(false);
            }
        });
        on_cleanup(move || scroll_handle.remove());
    }

    let current_label = move || {
        options
            .get()
            .into_iter()
            .find(|o| o.value == value.get())
            .map(|o| o.label)
            .unwrap_or_default()
    };

    // Bascule l'ouverture ; a l'ouverture, mesure le bouton pour ancrer le panneau
    // (bord droit du bouton — parite visuelle avec l'ancien `right-0`). Auto-flip :
    // ouvre vers le bas si la place suffit, sinon vers le haut (bouton bas de rail) ;
    // la hauteur est capee a l'espace dispo (240px max) pour ne jamais sortir du
    // viewport.
    let on_toggle = move |_| {
        let next = !open.get_untracked();
        if next {
            #[cfg(feature = "hydrate")]
            if let Some(btn) = button_ref.get_untracked() {
                let rect = btn.get_bounding_client_rect();
                let win_h = window()
                    .inner_height()
                    .ok()
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let cap = 240.0;
                let space_below = win_h - rect.bottom() - 8.0;
                let space_above = rect.top() - 8.0;
                let (top, max_h) = if space_below >= cap {
                    (rect.bottom() + 4.0, cap)
                } else if space_above > space_below {
                    let h = space_above.min(cap);
                    (rect.top() - 4.0 - h, h)
                } else {
                    (rect.bottom() + 4.0, space_below.max(0.0).min(cap))
                };
                coords.set(Some((top, rect.right(), rect.width(), max_h)));
            }
        }
        open.set(next);
    };

    let panel_style = move || {
        coords
            .get()
            .map(|(top, right, width, max_h)| {
                // `width:max-content` : sans largeur explicite, un panneau `fixed`
                // (containing block = viewport) ne shrink-wrap pas comme l'`absolute`
                // d'origine — les options `w-full` l'étiraient à toute la largeur.
                // On le borne au contenu, plancher = largeur du bouton.
                format!(
                    "position:fixed; top:{top}px; left:{right}px; transform:translateX(-100%); width:max-content; min-width:{width}px; max-height:{max_h}px;"
                )
            })
            .unwrap_or_default()
    };

    let wrapper = format!("relative inline-flex {class}");

    view! {
        <div node_ref=container_ref class=wrapper>
            <button
                node_ref=button_ref
                type="button"
                aria-label=aria_label
                aria-expanded=move || open.get().then_some("true")
                on:click=on_toggle
                class="flex items-center gap-1.5 rounded border border-[var(--color-rule)] px-2.5 py-1 text-xs text-[var(--color-ink)] transition-colors hover:border-[var(--color-ink)]"
            >
                <span>{current_label}</span>
                <svg
                    viewBox="0 0 12 8"
                    class=move || {
                        format!(
                            "h-2 w-2.5 text-[var(--color-ink-subtle)] transition-transform {}",
                            if open.get() { "rotate-180" } else { "" },
                        )
                    }
                    fill="none"
                    aria-hidden="true"
                >
                    <path
                        d="M1 1.5l5 5 5-5"
                        stroke="currentColor"
                        stroke-width="1.5"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                    />
                </svg>
            </button>
            <Show when=move || open.get()>
                <div
                    style=panel_style
                    class="z-50 max-h-60 overflow-y-auto overscroll-contain rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] py-1 shadow-lg"
                >
                    <For
                        each=move || options.get()
                        key=|opt| opt.value.clone()
                        let:opt
                    >
                        {
                            let opt_value = opt.value.clone();
                            let selected = move || value.get() == opt_value;
                            let opt_value_click = opt.value.clone();
                            view! {
                                <button
                                    type="button"
                                    on:click=move |_| {
                                        on_change.run(opt_value_click.clone());
                                        open.set(false);
                                    }
                                    class=move || {
                                        // `block` (pas l'`inline-block` par défaut du
                                        // `<button>`) : empile verticalement, sinon
                                        // sous `width:max-content` les options se
                                        // mettent côte à côte (largeur = leur somme).
                                        format!(
                                            "block w-full whitespace-nowrap px-3 py-1.5 text-left text-xs transition-colors {}",
                                            if selected() {
                                                "bg-[var(--color-bordeaux-soft)] text-[var(--color-accent)]"
                                            } else {
                                                "text-[var(--color-ink-muted)] hover:bg-[var(--color-bordeaux-soft)]/40 hover:text-[var(--color-ink)]"
                                            },
                                        )
                                    }
                                >
                                    {opt.label.clone()}
                                </button>
                            }
                        }
                    </For>
                </div>
            </Show>
        </div>
    }
}
