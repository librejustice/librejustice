//! `HeroSearch` : îlot interactif de la landing :
//! eyebrow + titre + accroche + formulaire + exemples groupés par univers.
//!
//! La barre porte un sélecteur de portée (Décisions | Textes | Annonces |
//! Annuaire) : la soumission route vers `/decision-search`, `/norm-search`, `/annonces` ou
//! `/annuaire`, et le suggest comme le pied de barre (aide à la recherche, lien
//! pleine page) suivent la portée. Les chips d'exemples sont groupées par
//! univers — le groupe enseigne ce que le site couvre, la chip pose le mode et
//! route vers le bon moteur.
//!
//! L'`<input>` est rendu inline (mêmes classes que `ui::Input`, taille `h-14`)
//! pour un champ contrôlé : l'`Input` substrate ne propage ni valeur ni
//! `on:input`.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_navigate;
use lj_dtos::UiAction;

use crate::helpers::encode_query;

use super::compact_search::query_state;
use super::query_length::{truncate_to_cap, QueryLengthNotice};
use super::search_submit::{SearchSubmit, SubmitSize};
use super::suggest_box::{SuggestController, SuggestPanel};
use super::syntax_hint::{HelpCorpus, SyntaxHint};

/// Univers ciblé par la barre : la soumission route vers son moteur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Decisions,
    Textes,
    Annonces,
    Annuaire,
}

impl Scope {
    fn label(self) -> &'static str {
        match self {
            Scope::Decisions => "Décisions",
            Scope::Textes => "Textes",
            Scope::Annonces => "Annonces",
            Scope::Annuaire => "Annuaire",
        }
    }

    fn suggest_mode(self) -> &'static str {
        match self {
            Scope::Decisions => "jurisprudence",
            Scope::Textes => "textes",
            // Le pot Annonces n'a pas d'index de suggestion : la fenêtre
            // vivante se renouvelle plus vite qu'un index de complétion.
            Scope::Annonces => "",
            Scope::Annuaire => "annuaire",
        }
    }

    /// The engine's full page.
    fn full_page_path(self) -> &'static str {
        match self {
            Scope::Decisions => "/decision-search",
            Scope::Textes => "/norm-search",
            Scope::Annonces => "/notice-search",
            Scope::Annuaire => "/entity-search",
        }
    }

    /// The label of the link under the bar, to the engine's full page: the
    /// search engines show their filters, the directory is browsed by category.
    fn full_page_label(self) -> &'static str {
        match self {
            Scope::Decisions | Scope::Textes | Scope::Annonces => "Recherche avancée",
            Scope::Annuaire => "Parcourir l'annuaire",
        }
    }
}

const SCOPES: [Scope; 4] = [
    Scope::Decisions,
    Scope::Textes,
    Scope::Annonces,
    Scope::Annuaire,
];

/// Groupe d'exemples d'un univers : le libellé enseigne la couverture, chaque
/// chip soumet vers le moteur du groupe.
struct ExampleGroup {
    label: &'static str,
    scope: Scope,
    examples: &'static [&'static str],
}

const EXAMPLE_GROUPS: &[ExampleGroup] = &[
    ExampleGroup {
        label: "Jurisprudence",
        scope: Scope::Decisions,
        examples: &[
            "responsabilité médicale infection nosocomiale",
            "trouble anormal de voisinage indemnisation",
        ],
    },
    ExampleGroup {
        label: "Lois, codes, conventions collectives",
        scope: Scope::Textes,
        examples: &[
            "garantie des vices cachés code civil",
            "jours fériés convention collective de la boulangerie",
        ],
    },
    ExampleGroup {
        label: "Marchés publics, vie des entreprises",
        scope: Scope::Annonces,
        examples: &["voirie communale", "redressement judiciaire"],
    },
    ExampleGroup {
        label: "Annuaire",
        scope: Scope::Annuaire,
        examples: &["SNCF"],
    },
];

#[component]
pub fn HeroSearch() -> impl IntoView {
    let navigate = use_navigate();
    let query = RwSignal::new(String::new());
    let scope = RwSignal::new(Scope::Decisions);
    // Autocomplétion : le mode suit le sélecteur de portée.
    let suggest = SuggestController::new(query, Signal::derive(move || scope.get().suggest_mode()));

    // Soumission : route vers le moteur de la portée. Depuis la landing, pas
    // de filtres existants à conserver, donc `q` seul.
    let submit = move |value: String, target: Scope| {
        let trimmed = truncate_to_cap(value.trim());
        // Le pot d'annonces répond sans requête — « ce qui est ouvert en ce
        // moment » est un résultat. Les autres portées n'ont rien à dire sans
        // mots.
        if trimmed.is_empty() && target != Scope::Annonces {
            return;
        }
        let href = match target {
            Scope::Decisions => {
                // Reclassement au défaut : la landing ne le règle pas, et son
                // absence de param vaut actif.
                let qs = query_state::with_query(
                    &leptos_router::params::ParamsMap::new(),
                    &trimmed,
                    None,
                );
                format!("/decision-search?{qs}")
            }
            Scope::Textes => format!("/norm-search?q={}", encode_query(&trimmed)),
            Scope::Annonces if trimmed.is_empty() => "/notice-search".to_string(),
            Scope::Annonces => format!("/notice-search?q={}", encode_query(&trimmed)),
            Scope::Annuaire => format!("/entity-search?q={}", encode_query(&trimmed)),
        };
        navigate(&href, Default::default());
    };

    let submit_form = submit.clone();
    let on_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        submit_form(query.get_untracked(), scope.get_untracked());
    };

    // Sous-blocs erasés en `AnyView` : casse la profondeur du type-tuple SSR
    // pour limiter la pression sur `recursion_limit`.
    let intro = view! {
        <div class="flex flex-col gap-4">
            <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                "Accès libre et gratuit"
            </p>
            <h1
                class="text-balance font-sans text-3xl leading-[1.02] tracking-tight text-[var(--color-ink)] sm:text-5xl lg:text-[3.85rem]"
                style="font-variation-settings: 'wght' 300"
            >
                "Le droit français, "
                <em
                    class="not-italic text-[var(--color-accent)]"
                    style="font-variation-settings: 'wght' 750"
                >
                    "pour tous."
                </em>
            </h1>
            <p class="max-w-2xl text-base leading-relaxed text-[var(--color-ink-muted)] sm:text-lg">
                "Cherchez dans les décisions de justice, les lois, les codes, les conventions "
                "collectives et les marchés publics, ou dans l'annuaire des parties et des avocats."
            </p>
        </div>
    }
    .into_any();

    let scope_selector = view! { <ScopeDropdown scope=scope /> }.into_any();

    let form = view! {
        <form on:submit=on_submit class="flex flex-col gap-2.5">
            <div class="flex flex-col gap-2 sm:flex-row">
                <div class="group relative flex h-14 w-full min-w-0 items-center gap-2 rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 text-base transition-colors has-[:focus-visible]:border-[var(--color-ink)] sm:flex-1">
                    {scope_selector}
                    <span class="text-[var(--color-ink-subtle)]" aria-hidden="true">
                        <SearchIcon />
                    </span>
                    <input
                        aria-label="Mots-clés ou question"
                        size="1"
                        autocomplete="off"
                        placeholder="Mots-clés, question, ou expression exacte…"
                        autofocus
                        prop:value=move || query.get()
                        on:input=move |ev| query.set(event_target_value(&ev))
                        on:keydown=suggest.on_keydown()
                        on:focus=suggest.on_focus()
                        on:blur=suggest.on_blur()
                        class="h-full min-w-0 flex-1 bg-transparent text-[var(--color-ink)] outline-none placeholder:text-[var(--color-ink-subtle)]"
                    />
                    <SuggestPanel ctrl=suggest />
                </div>
                <SearchSubmit size=SubmitSize::Lg />
            </div>
            <QueryLengthNotice query=query.into() />
            // Le pied de barre suit la portée : aide du moteur sélectionné
            // (l'annuaire n'en a pas), lien vers sa page pleine.
            <div class="flex flex-wrap items-center gap-x-4 gap-y-2">
                {move || match scope.get() {
                    Scope::Decisions => Some(view! { <SyntaxHint /> }),
                    Scope::Textes => Some(view! { <SyntaxHint corpus=HelpCorpus::Textes /> }),
                    Scope::Annonces | Scope::Annuaire => None,
                }}
                <div class="ml-auto flex items-center gap-4">
                    {move || {
                        let s = scope.get();
                        view! {
                            <A
                                href=s.full_page_path()
                                attr:class="flex items-center gap-1.5 text-xs text-[var(--color-ink-muted)] underline-offset-2 transition-colors hover:text-[var(--color-ink)] hover:underline"
                            >
                                <svg
                                    viewBox="0 0 14 14"
                                    class="h-3.5 w-3.5"
                                    fill="none"
                                    aria-hidden="true"
                                >
                                    <path
                                        d="M2 7h10M7 2l5 5-5 5"
                                        stroke="currentColor"
                                        stroke-width="1.4"
                                        stroke-linecap="round"
                                        stroke-linejoin="round"
                                    />
                                </svg>
                                {s.full_page_label()}
                            </A>
                        }
                    }}
                </div>
            </div>
        </form>
    }
    .into_any();

    // Exemples groupés par univers : le groupe enseigne, la chip pose le mode
    // du sélecteur et soumet vers le moteur du groupe.
    let examples = view! {
        <div class="flex flex-col gap-3" data-usage=UiAction::HomeExample.as_str()>
            {EXAMPLE_GROUPS
                .iter()
                .map(|group| {
                    let group_scope = group.scope;
                    let chips = group
                        .examples
                        .iter()
                        .map(|example| {
                            let submit = submit.clone();
                            let ex = (*example).to_string();
                            view! {
                                <li>
                                    <button
                                        type="button"
                                        on:click=move |_| {
                                            scope.set(group_scope);
                                            submit(ex.clone(), group_scope);
                                        }
                                        class="rounded-full border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 py-1 text-sm text-[var(--color-ink-muted)] transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)]"
                                    >
                                        {*example}
                                    </button>
                                </li>
                            }
                        })
                        .collect_view();
                    view! {
                        <div class="flex flex-col gap-1.5 sm:flex-row sm:items-baseline sm:gap-3">
                            <p class="shrink-0 text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)] sm:w-44">
                                {group.label}
                            </p>
                            <ul class="flex flex-wrap gap-2">{chips}</ul>
                        </div>
                    }
                })
                .collect::<Vec<_>>()}
        </div>
    }
    .into_any();

    view! {
        <section class="mx-auto flex max-w-3xl flex-col gap-6 px-4 pt-8 pb-5 sm:px-6 sm:pt-10 sm:pb-4 lg:pt-12 lg:pb-4">
            {intro}
            {form}
            {examples}
        </section>
    }
}

/// Sélecteur de portée intégré à la barre du hero. Déclencheur fondu dans la
/// barre (elle porte la bordure) : libellé de la portée courante + chevron,
/// pleine hauteur, filet séparateur à droite. Le panneau prolonge la barre
/// vers le bas (mêmes bords, même fond, pas de bordure haute) et ne liste que
/// les autres portées — la courante est déjà le libellé du bouton.
#[component]
fn ScopeDropdown(scope: RwSignal<Scope>) -> impl IntoView {
    let open = RwSignal::new(false);
    let container_ref = NodeRef::<leptos::html::Span>::new();

    #[cfg(feature = "hydrate")]
    {
        use crate::components::ui::panel::{dismiss, Dismiss};
        dismiss(
            container_ref,
            move || open.get_untracked(),
            move || open.set(false),
            Dismiss {
                escape: true,
                scroll: false,
            },
        );
    }

    view! {
        <span
            node_ref=container_ref
            class="relative flex h-full shrink-0 items-center border-r border-[var(--color-rule)] pr-2.5"
        >
            <button
                type="button"
                aria-label="Portée de la recherche"
                aria-expanded=move || open.get().then_some("true")
                on:click=move |_| open.update(|o| *o = !*o)
                class="flex h-full cursor-pointer items-center gap-1.5 text-sm text-[var(--color-ink)] transition-colors hover:text-[var(--color-accent)]"
            >
                // Libellés empilés dans une grille, seul le courant visible :
                // le bouton garde la largeur du plus long quel que soit le
                // sélectionné (le panneau, calé sur ses bords, ne coupe
                // jamais une option).
                <span class="grid text-left">
                    {SCOPES
                        .iter()
                        .map(|s| {
                            let this = *s;
                            view! {
                                <span
                                    class="col-start-1 row-start-1"
                                    class:invisible=move || scope.get() != this
                                >
                                    {this.label()}
                                </span>
                            }
                        })
                        .collect_view()}
                </span>
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
            // Panneau dans la continuité de la barre : collé sous le déclencheur
            // (`top-full` recouvre la bordure basse de la barre), étiré du bord gauche
            // de la barre (`-left-[13px]` = padding `px-3` + bordure 1px) au filet
            // séparateur (`right-0`), sans bordure haute et arrondi en bas seulement.
            <div
                data-open=move || open.get().then_some("true")
                class="lj-panel absolute -left-[13px] right-0 top-full z-50 overflow-hidden rounded-b-md border border-t-0 border-[var(--color-rule)] bg-[var(--color-parchment)] pb-1 shadow-lg"
            >
                {move || {
                        let current = scope.get();
                        SCOPES
                            .iter()
                            .filter(|s| **s != current)
                            .map(|s| {
                                let target = *s;
                                view! {
                                    <button
                                        type="button"
                                        on:click=move |_| {
                                            scope.set(target);
                                            open.set(false);
                                        }
                                        class="block w-full whitespace-nowrap px-[13px] py-2.5 text-left text-sm text-[var(--color-ink)] transition-colors hover:bg-[var(--color-vellum)]"
                                    >
                                        {target.label()}
                                    </button>
                                }
                            })
                            .collect_view()
                    }}
            </div>
        </span>
    }
}

#[component]
fn SearchIcon() -> impl IntoView {
    view! {
        <svg aria-hidden="true" viewBox="0 0 20 20" class="h-5 w-5">
            <circle cx="9" cy="9" r="6" fill="none" stroke="currentColor" stroke-width="1.6" />
            <path
                d="M14 14l4 4"
                fill="none"
                stroke="currentColor"
                stroke-width="1.6"
                stroke-linecap="round"
            />
        </svg>
    }
}
