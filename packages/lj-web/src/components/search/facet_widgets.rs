//! Briques partagées des filtres décisions (extraites de l'ex-`filter_rail.rs`,
//! rail vertical remplacé par la barre horizontale gabarit de référence) :
//! navigation (`Nav`), clés de filtre, arbre 2 niveaux, lignes checkbox
//! (`CheckboxOption`, `TreeRow`), recherche intra-facette, checklist plate
//! plafonnée (`FacetChecklist`) et anti scroll-chaining (`scroll_lock_ref`).
//!
//! Les options viennent des facettes servies (`FacetChoice` : valeur + libellé +
//! `parent`) — aucune table de libellés compilée. Une valeur sélectionnée absente
//! de la facette (URL partagée, compteur nul) reste affichée en ligne orpheline
//! (libellé = valeur brute, compteur 0).

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;
use lj_dtos::FacetChoice;

use crate::helpers::cn;

use super::compact_search::query_state;

/// `NodeRef` d'un conteneur scrollable, câblé à l'anti scroll-chaining. Côté
/// wasm, un `Effect` appelle `lockScrollChaining` sur le div (handler `wheel`
/// non-passif) ; la fn de déconnexion vit dans un slot `LocalStorage` (les
/// handles JS ne sont pas `Send`). En SSR, le ref n'est pas utilisé.
pub fn scroll_lock_ref() -> NodeRef<leptos::html::Div> {
    let node_ref = NodeRef::<leptos::html::Div>::new();

    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::prelude::*;

        #[wasm_bindgen(module = "/src/components/search/scroll_lock.js")]
        extern "C" {
            #[wasm_bindgen(js_name = "lockScrollChaining")]
            fn lock_scroll_chaining(el: &JsValue) -> JsValue;
        }

        let disconnect_slot: StoredValue<Option<JsValue>, leptos::reactive::owner::LocalStorage> =
            StoredValue::new_local(None);
        Effect::new(move |_| {
            let Some(node) = node_ref.get() else {
                return;
            };
            let node_js: JsValue = node.into();
            let disconnect = lock_scroll_chaining(&node_js);
            disconnect_slot.set_value(Some(disconnect));
        });
    }

    node_ref
}

/// Clés de filtre décisions (parité `FILTER_KEYS`).
pub fn filter_keys() -> Vec<&'static str> {
    vec![
        "jur",
        "office",
        "jcode",
        "domaine",
        "solution",
        "portee",
        "publication",
        "li",
        "la",
        "from",
        "to",
    ]
}

/// Regroupe une facette plate en arbre 2 niveaux : racines (`parent = None`)
/// puis enfants rattachés par `parent` == `value` racine (contrat ADR 0146 :
/// le `parent` d'un enfant reprend la `value` de sa racine verbatim).
pub fn build_tree(choices: &[FacetChoice]) -> Vec<(FacetChoice, Vec<FacetChoice>)> {
    let mut roots: Vec<(FacetChoice, Vec<FacetChoice>)> = choices
        .iter()
        .filter(|c| c.parent.is_none())
        .map(|c| (c.clone(), Vec::new()))
        .collect();
    for child in choices.iter().filter(|c| c.parent.is_some()) {
        if let Some((_, children)) = roots
            .iter_mut()
            .find(|(root, _)| Some(&root.value) == child.parent.as_ref())
        {
            children.push(child.clone());
        }
    }
    roots
}

/// Valeur de toggle (clé d'URL `jur`) d'une racine juridiction :
/// `juridiction:TJ` → `TJ`.
pub fn juridiction_root_value(uid: &str) -> String {
    match uid.split_once(':') {
        Some((_, suffix)) => suffix.to_string(),
        None => uid.to_string(),
    }
}

type NavFn = Box<dyn Fn(&str, NavigateOptions) + Send + Sync>;

/// Navigateur partagé : applique une query string et navigue (replace=true).
///
/// `navigate` (Send+Sync) est stocké en `StoredValue` (SyncStorage) : le handle
/// est Copy, donc `Nav` est Copy et peut être capturé par les `Callback::new`
/// (qui exigent Send+Sync) des sous-composants. NB : `new_local` (LocalStorage)
/// attacherait la valeur à l'owner courant, introuvable depuis le contexte
/// détaché d'un `Callback` → `with_value` no-op silencieux (la nav ne partait
/// pas : filtres, dates et pagination restaient sans effet). SyncStorage rend la
/// valeur lisible partout.
#[derive(Clone, Copy)]
pub struct Nav {
    navigate: StoredValue<NavFn>,
}

impl Nav {
    /// À appeler dans un contexte de composant (résout `use_navigate`).
    pub fn new() -> Self {
        Self {
            navigate: StoredValue::new(Box::new(use_navigate()) as NavFn),
        }
    }

    pub fn go(&self, qs: String) {
        self.navigate.with_value(|n| {
            n(
                &query_state::search_href(&qs),
                NavigateOptions {
                    replace: true,
                    // `scroll` défaut `true` : le router remonterait en haut à
                    // chaque mutation de facette. Les filtres affinent la
                    // recherche courante (replace=true), ils ne naviguent pas —
                    // on conserve la position de scroll.
                    scroll: false,
                    ..Default::default()
                },
            )
        });
    }
}

impl Default for Nav {
    fn default() -> Self {
        Self::new()
    }
}

// ── CheckboxOption ────────────────────────────────────────────────────────────

#[component]
pub fn CheckboxOption(
    // `label` réactif : pour « Textes cités » le libellé (titre catalogue) arrive
    // avec les facettes, après la ligne (token d'URL). Les autres call-sites
    // passent un `String` (converti via `into`).
    #[prop(into)] label: Signal<String>,
    // `count` réactif : sur arrivée des facettes (nouvelle recherche) les compteurs
    // changent en place sans reconstruire la ligne — parité React (re-render keyé).
    #[prop(into)] count: Signal<i64>,
    #[prop(into)] selected: Signal<bool>,
    #[prop(into)] on_toggle: Callback<()>,
    #[prop(optional)] indent: bool,
) -> impl IntoView {
    let button_class = move || {
        cn([
            "flex w-full items-center gap-2.5 rounded px-2 py-1 text-left text-sm transition-colors",
            if indent { "pl-7" } else { "" },
            if selected.get() {
                "bg-[var(--color-bordeaux-soft)] text-[var(--color-accent)]"
            } else {
                "text-[var(--color-ink-muted)] hover:text-[var(--color-ink)]"
            },
        ])
    };
    let box_class = move || {
        cn([
            "flex h-4 w-4 shrink-0 items-center justify-center rounded-sm border transition-colors",
            if selected.get() {
                "border-[var(--color-accent)] bg-[var(--color-accent)]"
            } else {
                "border-[var(--color-rule)] bg-transparent"
            },
        ])
    };
    view! {
        <button type="button" on:click=move |_| on_toggle.run(()) class=button_class>
            <span class=box_class>
                <Show when=move || selected.get()>
                    <svg
                        viewBox="0 0 12 10"
                        class="h-2.5 w-3 text-white"
                        fill="none"
                        aria-hidden="true"
                    >
                        <path
                            d="M1 5l3.5 3.5L11 1"
                            stroke="currentColor"
                            stroke-width="1.8"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                        />
                    </svg>
                </Show>
            </span>
            <span class="flex-1 leading-tight">{label}</span>
            <Show when=move || { count.get() > 0 }>
                <span class="tabular-nums text-xs text-[var(--color-ink-subtle)]">
                    {move || count.get()}
                </span>
            </Show>
        </button>
    }
}

// ── FilterSearchInput ─────────────────────────────────────────────────────────

#[component]
pub fn FilterSearchInput(
    value: RwSignal<String>,
    placeholder: &'static str,
    #[prop(optional)] on_enter: Option<Callback<()>>,
) -> impl IntoView {
    let on_keydown = move |ev: leptos::ev::KeyboardEvent| {
        if ev.key() == "Enter" {
            if let Some(cb) = on_enter {
                ev.prevent_default();
                cb.run(());
            }
        }
    };
    view! {
        <div class="relative mb-1">
            <svg
                viewBox="0 0 16 16"
                class="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-[var(--color-ink-subtle)]"
                fill="none"
                aria-hidden="true"
            >
                <circle cx="6.5" cy="6.5" r="4.5" stroke="currentColor" stroke-width="1.4" />
                <path d="M10.5 10.5l3 3" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
            </svg>
            <input
                type="search"
                prop:value=move || value.get()
                on:input=move |ev| value.set(event_target_value(&ev))
                on:keydown=on_keydown
                placeholder=placeholder
                class="w-full rounded border border-[var(--color-rule)] bg-transparent py-1 pl-7 pr-2 text-xs text-[var(--color-ink)] placeholder:text-[var(--color-ink-subtle)] focus:border-[var(--color-accent)] focus:outline-none"
            />
        </div>
    }
}

// ── TreeRow ───────────────────────────────────────────────────────────────────

/// Ligne racine d'un arbre 2 niveaux (Juridiction, Domaine) : chevron +
/// checkbox racine, enfants `FacetChoice` dépliables (lazy-mount, auto-dépli
/// quand un enfant est sélectionné).
#[component]
pub fn TreeRow(
    #[prop(into)] label: Signal<String>,
    #[prop(into)] count: Signal<i64>,
    #[prop(into)] selected: Signal<bool>,
    #[prop(into)] child_choices: Signal<Vec<FacetChoice>>,
    /// Valeurs sélectionnées de la clé des enfants (sélection globale — la ligne
    /// n'en lit que les valeurs présentes dans SES enfants).
    #[prop(into)]
    selected_children: Signal<Vec<String>>,
    #[prop(into)] on_toggle: Callback<()>,
    #[prop(into)] on_toggle_child: Callback<String>,
) -> impl IntoView {
    let has_children = Signal::derive(move || !child_choices.get().is_empty());
    // `Memo` (pas `Signal::derive`) : ne notifie qu'au changement effectif du bool,
    // pour que l'`Effect` d'auto-dépli ne se redéclenche pas à chaque toggle.
    let has_selection = Memo::new(move |_| {
        let selected = selected_children.get();
        child_choices
            .get()
            .iter()
            .any(|c| selected.contains(&c.value))
    });
    let expanded = RwSignal::new(has_selection.get_untracked());
    // Auto-dépli sens unique : on déplie quand une sélection apparaît (URL
    // partagée, coche), jamais l'inverse — le repli reste piloté par le chevron.
    Effect::new(move |_| {
        if has_selection.get() {
            expanded.set(true);
        }
    });
    // Lazy-mount des enfants : on ne construit les lignes-enfants (jusqu'à
    // plusieurs centaines de codes pour un type de juridiction) qu'au premier
    // dépli — sinon leurs compteurs recalculeraient à chaque arrivée de facettes.
    let has_opened = RwSignal::new(has_selection.get_untracked());
    Effect::new(move |_| {
        if expanded.get() {
            has_opened.set(true);
        }
    });

    let chevron_class = move || {
        cn([
            "flex h-5 w-5 shrink-0 items-center justify-center rounded text-[var(--color-ink-subtle)] transition-colors hover:text-[var(--color-ink)]",
            if !has_children.get() { "pointer-events-none opacity-0" } else { "" },
        ])
    };

    view! {
        <div class="flex flex-col gap-1">
            <div class="flex items-center gap-1">
                <button
                    type="button"
                    aria-label=move || if expanded.get() { "Réduire" } else { "Développer" }
                    on:click=move |_| expanded.update(|v| *v = !*v)
                    class=chevron_class
                >
                    <svg
                        viewBox="0 0 12 12"
                        class=move || {
                            cn([
                                "h-3 w-3 transition-transform",
                                if expanded.get() { "rotate-90" } else { "" },
                            ])
                        }
                        fill="none"
                        aria-hidden="true"
                    >
                        <path
                            d="M4 2l4 4-4 4"
                            stroke="currentColor"
                            stroke-width="1.5"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                        />
                    </svg>
                </button>
                <CheckboxOption
                    label=label
                    count=count
                    selected=selected
                    on_toggle=on_toggle
                    indent=false
                />
            </div>
            <Show when=move || has_children.get()>
                <div
                    class="grid"
                    style=move || {
                        format!(
                            "grid-template-rows: {}; transition: grid-template-rows 200ms ease-out",
                            if expanded.get() { "1fr" } else { "0fr" },
                        )
                    }
                >
                    <div class="min-h-0 overflow-hidden">
                        <div class="flex flex-col gap-1 pl-5 pt-0.5">
                            // `<For>` keyé par valeur : compteur + libellé + état coché
                            // par ligne sont des signaux dérivés, mis à jour en place.
                            {move || {
                                has_opened
                                    .get()
                                    .then(|| {
                                        view! {
                                            <For
                                                each=move || child_choices.get()
                                                key=|c: &FacetChoice| c.value.clone()
                                                children=move |c: FacetChoice| {
                                                    let value = c.value.clone();
                                                    let lookup = c.value.clone();
                                                    let row = Memo::new(move |_| {
                                                        child_choices
                                                            .with(|l| {
                                                                l.iter().find(|x| x.value == lookup).cloned()
                                                            })
                                                    });
                                                    let fallback = c.value.clone();
                                                    let row_label = Signal::derive(move || {
                                                        row.get()
                                                            .map(|x| x.label)
                                                            .unwrap_or(fallback.clone())
                                                    });
                                                    let row_count = Signal::derive(move || {
                                                        row.get().map(|x| x.count).unwrap_or(0)
                                                    });
                                                    let sel_value = c.value.clone();
                                                    let is_sel = Signal::derive(move || {
                                                        selected_children.get().contains(&sel_value)
                                                    });
                                                    view! {
                                                        <CheckboxOption
                                                            label=row_label
                                                            count=row_count
                                                            selected=is_sel
                                                            on_toggle=Callback::new(move |_| on_toggle_child
                                                                .run(value.clone()))
                                                            indent=true
                                                        />
                                                    }
                                                }
                                            />
                                        }
                                    })
                            }}
                        </div>
                    </div>
                </div>
            </Show>
        </div>
    }
}

// ── FacetChecklist ────────────────────────────────────────────────────────────

/// Liste plate à cocher pilotée par les facettes servies (Publication,
/// Dispositif — sections du modal « Plus de filtres »). Lignes = sélection
/// orpheline (libellé = valeur brute, compteur 0) ∪ facette. Plafonnée à 6
/// lignes + « Voir plus » (gabarit de référence A5).
#[component]
pub fn FacetChecklist(
    #[prop(into)] choices: Signal<Vec<FacetChoice>>,
    #[prop(into)] selected: Signal<Vec<String>>,
    #[prop(into)] on_toggle: Callback<String>,
) -> impl IntoView {
    const CAP: usize = 6;
    let show_all = RwSignal::new(false);
    // `Memo` : lu par les signaux `count`/`label` de chaque ligne.
    let rows = Memo::new(move |_| {
        let choices = choices.get();
        let mut out: Vec<FacetChoice> = selected
            .get()
            .into_iter()
            .filter(|s| !choices.iter().any(|c| &c.value == s))
            .map(|s| FacetChoice {
                label: s.clone(),
                value: s,
                count: 0,
                parent: None,
            })
            .collect();
        out.extend(choices);
        out
    });
    let overflow = Signal::derive(move || rows.with(|r| r.len()) > CAP);
    let visible = Memo::new(move |_| {
        let rows = rows.get();
        if show_all.get() {
            rows
        } else {
            rows.into_iter().take(CAP).collect()
        }
    });
    view! {
        <div class="flex flex-col gap-1.5">
            <For
                each=move || visible.get()
                key=|c: &FacetChoice| c.value.clone()
                children=move |c: FacetChoice| {
                    let value = c.value.clone();
                    let lookup = c.value.clone();
                    let row = Memo::new(move |_| {
                        rows.with(|l| l.iter().find(|x| x.value == lookup).cloned())
                    });
                    let fallback = c.value.clone();
                    let row_label = Signal::derive(move || {
                        row.get().map(|x| x.label).unwrap_or(fallback.clone())
                    });
                    let row_count =
                        Signal::derive(move || row.get().map(|x| x.count).unwrap_or(0));
                    let sel_value = c.value.clone();
                    let is_sel =
                        Signal::derive(move || selected.get().contains(&sel_value));
                    view! {
                        <CheckboxOption
                            label=row_label
                            count=row_count
                            selected=is_sel
                            on_toggle=Callback::new(move |_| on_toggle.run(value.clone()))
                            indent=false
                        />
                    }
                }
            />
            <Show when=move || overflow.get()>
                <button
                    type="button"
                    on:click=move |_| show_all.update(|v| *v = !*v)
                    class="self-start px-2 text-xs text-[var(--color-accent)] hover:underline"
                >
                    {move || if show_all.get() { "Voir moins" } else { "Voir plus" }}
                </button>
            </Show>
        </div>
    }
}
