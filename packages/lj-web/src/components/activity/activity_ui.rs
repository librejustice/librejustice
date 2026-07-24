//! Kit de composants « Mon activite » — port de `activity-ui.tsx`. Memes classes
//! Tailwind, meme structure DOM. Composants partages par les trois panneaux
//! (recherches / lectures / signets).

use leptos::prelude::*;
use leptos_router::components::A;

use lj_dtos::ActivitySource;

use crate::helpers::cn;

/// Onglets du hub (routes distinctes, panneau selon le path).
const TABS: [(&str, &str); 3] = [
    ("/activite/recherches", "Recherches"),
    ("/activite/lectures", "Lectures"),
    ("/activite/signets", "Signets"),
];

/// Shell 2 colonnes (contenu + aside) des pages d'activite.
#[component]
pub fn ActivityShell(
    children: Children,
    #[prop(optional)] aside: Option<AnyView>,
) -> impl IntoView {
    // Chemin courant pour styler l'onglet actif (port de `NavLink isActive`).
    let pathname = {
        #[cfg(feature = "hydrate")]
        {
            let location = leptos_router::hooks::use_location();
            Signal::derive(move || location.pathname.get())
        }
        #[cfg(not(feature = "hydrate"))]
        {
            Signal::derive(String::new)
        }
    };

    view! {
        <div class="mx-auto flex w-full max-w-3xl flex-col gap-7 px-4 py-12">
            <div class="flex flex-col gap-5">
                <h1
                    class="font-sans text-2xl text-[var(--color-ink)]"
                    style="font-variation-settings: 'wght' 300"
                >
                    "Mon activité"
                </h1>
                <div class="flex flex-wrap items-center justify-between gap-x-4 gap-y-3">
                    <nav class="flex w-fit items-center gap-1 rounded-full border border-[var(--color-rule)] bg-[var(--color-vellum)] p-1">
                        {TABS
                            .into_iter()
                            .map(|(to, label)| {
                                let is_active = move || pathname.get().starts_with(to);
                                view! {
                                    <A
                                        href=to
                                        attr:class=move || {
                                            cn([
                                                "rounded-full px-4 py-1.5 text-sm transition-colors",
                                                if is_active() {
                                                    "bg-[var(--color-parchment)] text-[var(--color-ink)] shadow-sm"
                                                } else {
                                                    "text-[var(--color-ink-muted)] hover:text-[var(--color-ink)]"
                                                },
                                            ])
                                        }
                                    >
                                        {label}
                                    </A>
                                }
                            })
                            .collect_view()}
                    </nav>
                    {aside}
                </div>
            </div>
            {children()}
        </div>
    }
}

/// Interrupteur sobre (`role="switch"`).
#[component]
pub fn Switch(
    #[prop(into)] checked: Signal<bool>,
    on_change: impl Fn() + 'static,
    #[prop(into)] disabled: Signal<bool>,
    #[prop(into)] label: String,
) -> impl IntoView {
    view! {
        <button
            type="button"
            role="switch"
            aria-checked=move || checked.get().to_string()
            aria-label=label
            disabled=move || disabled.get()
            on:click=move |_| on_change()
            class=move || {
                cn([
                    "relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors disabled:opacity-50",
                    if checked.get() {
                        "bg-[var(--color-accent)]"
                    } else {
                        "bg-[var(--color-rule)]"
                    },
                ])
            }
        >
            <span
                aria-hidden="true"
                class=move || {
                    cn([
                        "inline-block h-3.5 w-3.5 transform rounded-full bg-[var(--color-parchment)] shadow-sm transition-transform",
                        if checked.get() { "translate-x-[18px]" } else { "translate-x-[3px]" },
                    ])
                }
            ></span>
        </button>
    }
}

/// Barre d'outils d'un panneau : compteur + action « Tout effacer » optionnelle.
#[component]
pub fn PanelToolbar(
    #[prop(into)] count: Signal<i64>,
    #[prop(optional)] on_clear: Option<Callback<()>>,
    #[prop(optional, into)] clear_label: Option<String>,
) -> impl IntoView {
    let clear_label = clear_label.unwrap_or_else(|| "Tout effacer".to_string());
    view! {
        <div class="flex h-5 items-center justify-between">
            <p class="text-xs text-[var(--color-ink-subtle)]">
                {move || {
                    let n = count.get();
                    format!("{n} {}", if n > 1 { "éléments" } else { "élément" })
                }}
            </p>
            {move || {
                on_clear
                    .filter(|_| count.get() > 0)
                    .map(|cb| {
                        let label = clear_label.clone();
                        view! {
                            <button
                                type="button"
                                on:click=move |_| cb.run(())
                                class="text-xs text-[var(--color-ink-subtle)] underline transition-colors hover:text-[var(--color-ink)]"
                            >
                                {label}
                            </button>
                        }
                    })
            }}
        </div>
    }
}

/// Skeleton d'une liste d'activite : barre d'outils + cartes placeholder.
/// Calque la forme de `PanelToolbar` + `CardList`/`ActivityCard` pendant la
/// verification de session puis le chargement de la liste — jamais de zone vide.
#[component]
pub fn ActivityListSkeleton() -> impl IntoView {
    view! {
        <div aria-hidden="true" class="flex flex-col gap-3">
            <div class="h-5 w-24 animate-pulse rounded-sm bg-[var(--color-vellum)]"></div>
            <ul class="flex flex-col gap-3">
                {(0..5)
                    .map(|_| {
                        view! {
                            <li class="h-[68px] animate-pulse rounded-xl border border-[var(--color-rule)] bg-[var(--color-vellum)]/50" />
                        }
                    })
                    .collect_view()}
            </ul>
        </div>
    }
}

/// Ligne d'erreur.
#[component]
pub fn ErrorLine(children: Children) -> impl IntoView {
    view! { <p class="text-sm text-red-600">{children()}</p> }
}

/// Etat vide (encart pointille).
#[component]
pub fn EmptyState(children: Children) -> impl IntoView {
    view! {
        <div class="rounded-xl border border-dashed border-[var(--color-rule)] px-5 py-10 text-center">
            <p class="text-sm text-[var(--color-ink-muted)]">{children()}</p>
        </div>
    }
}

/// Pastille du canal d'origine : web (contour neutre) vs MCP (bleu plein).
#[component]
pub fn SourceBadge(source: ActivitySource) -> impl IntoView {
    match source {
        ActivitySource::Mcp => view! {
            <span
                title="Action effectuée via le connecteur MCP (assistant IA)"
                class="inline-flex shrink-0 items-center gap-1 rounded-full bg-[var(--color-mode-hybrid-soft)] px-2 py-0.5 text-[10px] font-semibold tracking-wide text-[var(--color-mode-hybrid)] uppercase"
            >
                <Dot filled=true />
                " MCP"
            </span>
        }
        .into_any(),
        ActivitySource::Web => view! {
            <span
                title="Action effectuée depuis le site"
                class="inline-flex shrink-0 items-center gap-1 rounded-full border border-[var(--color-rule)] px-2 py-0.5 text-[10px] font-medium tracking-wide text-[var(--color-ink-subtle)] uppercase"
            >
                <Dot filled=false />
                " Web"
            </span>
        }
        .into_any(),
    }
}

/// Petit indicateur rond : plein (MCP) ou cercle (web).
#[component]
fn Dot(filled: bool) -> impl IntoView {
    view! {
        <svg aria-hidden="true" viewBox="0 0 8 8" class="h-1.5 w-1.5">
            <circle
                cx="4"
                cy="4"
                r=if filled { "3" } else { "2.4" }
                fill=if filled { "currentColor" } else { "none" }
                stroke="currentColor"
                stroke-width=if filled { "0" } else { "1.4" }
            ></circle>
        </svg>
    }
}

/// Carte d'activite : titre cliquable, ligne meta, pastille optionnelle, retrait.
#[component]
pub fn ActivityCard(
    #[prop(into)] to: String,
    #[prop(into)] title: String,
    meta: AnyView,
    #[prop(optional)] badge: Option<AnyView>,
    on_delete: impl Fn() + 'static,
    #[prop(into)] delete_label: String,
) -> impl IntoView {
    let delete_label_attr = delete_label.clone();
    view! {
        <li class="group relative flex items-start gap-3 rounded-xl border border-[var(--color-rule)] bg-[var(--color-parchment)] p-4 transition-colors hover:border-[var(--color-ink-subtle)]">
            <div class="flex min-w-0 flex-1 flex-col gap-1.5">
                <A
                    href=to
                    attr:class="line-clamp-2 text-[15px] leading-snug font-medium text-[var(--color-ink)] transition-colors hover:text-[var(--color-accent)]"
                >
                    {title}
                </A>
                <div class="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-[var(--color-ink-subtle)]">
                    {meta}
                </div>
            </div>
            <div class="flex shrink-0 items-center gap-2">
                {badge}
                <button
                    type="button"
                    on:click=move |_| on_delete()
                    aria-label=delete_label_attr
                    title=delete_label
                    class="flex h-6 w-6 items-center justify-center rounded-full text-[var(--color-ink-subtle)] opacity-60 transition-all hover:bg-[var(--color-vellum)] hover:text-[var(--color-ink)] hover:opacity-100"
                >
                    <CloseIcon />
                </button>
            </div>
        </li>
    }
}

/// Liste de cartes.
#[component]
pub fn CardList(children: Children) -> impl IntoView {
    view! { <ul class="flex flex-col gap-3">{children()}</ul> }
}

/// Pastille « libelle : valeur » d'un filtre actif.
#[component]
pub fn FilterChip(#[prop(into)] label: String, #[prop(into)] value: String) -> impl IntoView {
    view! {
        <span class="inline-flex items-center gap-1 rounded-full bg-[var(--color-vellum)] px-2 py-0.5">
            <span class="text-[var(--color-ink-subtle)]">{label}</span>
            <span class="text-[var(--color-ink-muted)]">{value}</span>
        </span>
    }
}

/// Sentinelle de scroll infini : declenche `on_reach` a l'entree dans le
/// viewport (marge 400px). Inerte tant que `has_more` est faux.
#[component]
pub fn InfiniteSentinel(
    #[prop(into)] has_more: Signal<bool>,
    #[prop(into)] is_loading: Signal<bool>,
    on_reach: impl Fn() + 'static,
) -> impl IntoView {
    let node_ref = NodeRef::<leptos::html::Div>::new();

    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::prelude::*;

        #[wasm_bindgen(module = "/src/components/activity/sentinel.js")]
        extern "C" {
            #[wasm_bindgen(js_name = "observeIntersection")]
            fn observe_intersection(node: &JsValue, cb: &Closure<dyn FnMut()>) -> JsValue;
        }

        // L'observateur et sa fn de deconnexion tiennent des handles JS (pas
        // `Send`) : on les garde dans des slots `LocalStorage` et on deconnecte
        // l'observateur precedent a chaque passage (port du `disconnect()` du
        // cleanup React). `on_cleanup` est inutilisable (exige `Send + Sync`).
        let on_reach = std::rc::Rc::new(on_reach);
        let disconnect_slot: StoredValue<Option<JsValue>, leptos::reactive::owner::LocalStorage> =
            StoredValue::new_local(None);
        let closure_slot: StoredValue<
            Option<Closure<dyn FnMut()>>,
            leptos::reactive::owner::LocalStorage,
        > = StoredValue::new_local(None);
        Effect::new(move |_| {
            // Deconnecte l'observateur du passage precedent.
            if let Some(prev) = disconnect_slot.try_update_value(Option::take).flatten() {
                if let Ok(f) = prev.dyn_into::<js_sys::Function>() {
                    let _ = f.call0(&JsValue::NULL);
                }
            }
            let Some(node) = node_ref.get() else {
                return;
            };
            if !has_more.get() {
                return;
            }
            let cb_fn = on_reach.clone();
            let closure = Closure::new(move || cb_fn());
            let node_js: JsValue = node.into();
            let disconnect = observe_intersection(&node_js, &closure);
            disconnect_slot.set_value(Some(disconnect));
            closure_slot.set_value(Some(closure));
        });
    }
    #[cfg(not(feature = "hydrate"))]
    let _ = &on_reach;

    view! {
        <Show when=move || has_more.get() || is_loading.get()>
            <div
                node_ref=node_ref
                class="flex h-8 items-center justify-center"
                aria-hidden=move || (!is_loading.get()).to_string()
            >
                <Show when=move || is_loading.get()>
                    <span class="text-xs text-[var(--color-ink-subtle)]">"Chargement…"</span>
                </Show>
            </div>
        </Show>
    }
}

/// Separateur point (`·`) entre fragments de meta.
#[component]
pub fn Sep() -> impl IntoView {
    view! { <span aria-hidden="true">"·"</span> }
}

/// Icone de fermeture (retrait d'une carte).
#[component]
fn CloseIcon() -> impl IntoView {
    view! {
        <svg aria-hidden="true" viewBox="0 0 16 16" class="h-3.5 w-3.5">
            <path
                d="M4 4l8 8M12 4l-8 8"
                fill="none"
                stroke="currentColor"
                stroke-width="1.4"
                stroke-linecap="round"
            ></path>
        </svg>
    }
}

/// Temps relatif compact FR (« il y a 5 min », « il y a 3 j »). Port de
/// `relativeTime`. `now`/parse via `js_sys::Date` (cote client) ; au SSR la
/// coquille est inerte (ces listes ne sont rendues qu'apres hydratation), on
/// renvoie la chaine ISO brute.
pub fn relative_time(iso: &str) -> String {
    #[cfg(feature = "hydrate")]
    {
        let parsed = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(iso)).get_time();
        if parsed.is_nan() {
            return iso.to_string();
        }
        let now = js_sys::Date::now();
        let diff_sec = ((now - parsed) / 1000.0).max(0.0);
        if diff_sec < 60.0 {
            "à l'instant".to_string()
        } else if diff_sec < 3600.0 {
            format!("il y a {} min", (diff_sec / 60.0).floor() as i64)
        } else if diff_sec < 86400.0 {
            format!("il y a {} h", (diff_sec / 3600.0).floor() as i64)
        } else {
            format!("il y a {} j", (diff_sec / 86400.0).floor() as i64)
        }
    }
    #[cfg(not(feature = "hydrate"))]
    {
        iso.to_string()
    }
}
