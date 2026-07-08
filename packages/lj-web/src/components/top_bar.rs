//! Barre superieure. Port de `components/layout/top-bar.tsx`. Deux etats, choisis
//! reactivement via le contexte `DecisionBar` : barre par defaut (wordmark + MCP +
//! auth) ou barre decision (retour + titre + navigation inter-resultats).
//!
//! `AuthButton` lit l'`AuthState` reactif : anonyme => lien Connexion ; connecte
//! => avatar (initiale de l'email) + menu compte (Profil / Mon activite /
//! Deconnexion). L'email est `None` au SSR (session locale) puis peuple cote
//! client => premier rendu = Connexion, swap apres hydratation (pas de mismatch).

use leptos::either::Either;
use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::components::decision_bar::{
    use_decision_bar, use_result_nav, DecisionBarState, ResultNav, ResultNavSeed,
};
use crate::helpers::cn;

#[component]
pub fn TopBar() -> impl IntoView {
    let bar = use_decision_bar();
    move || match bar.get() {
        Some(state) => Either::Left(view! { <DecisionTopBar bar=state /> }),
        None => Either::Right(view! { <DefaultTopBar /> }),
    }
}

/// Barre par defaut (toutes les pages hors decision). Port de la branche `!bar`.
#[component]
fn DefaultTopBar() -> impl IntoView {
    view! {
        <header class="sticky top-0 z-30 border-b border-[var(--color-rule)] bg-[var(--color-parchment)]/95 backdrop-blur supports-[backdrop-filter]:bg-[var(--color-parchment)]/80">
            <div class="mx-auto flex h-14 max-w-7xl items-center justify-between gap-2 px-4 sm:gap-6 sm:px-6 lg:px-8">
                <A
                    href="/"
                    attr:class="flex min-w-0 items-center gap-2 text-[var(--color-ink)] no-underline hover:opacity-90"
                >
                    <Wordmark />
                </A>
                <SiteNav />
            </div>
        </header>
    }
}

/// Menus du site (Décisions / Textes / MCP / compte), partagés par la barre
/// par défaut et les deux variantes de la barre décision.
#[component]
fn SiteNav() -> impl IntoView {
    view! {
        <nav class="flex shrink-0 items-center gap-0.5 sm:gap-1">
            <NavItem href="/recherche">"Décisions"</NavItem>
            <NavItem href="/textes">"Textes"</NavItem>
            <NavItem href="/mcp-guide">"MCP"</NavItem>
            <AuthButton />
        </nav>
    }
}

/// Bouton retour vers la recherche : liste exacte + scroll restauré quand
/// l'origine est connue (`from_search`), page recherche vierge sinon.
#[component]
fn BackButton(from_search: Option<crate::components::decision_bar::FromSearch>) -> impl IntoView {
    let navigate = StoredValue::new_local(use_navigate());
    let restore_scroll = crate::components::decision_bar::use_restore_scroll();
    let back_label = if from_search.is_some() {
        "Résultats"
    } else {
        "Recherche"
    };
    let on_back = move |_| {
        let target = match &from_search {
            Some(fs) => format!("/recherche{}", fs.search),
            None => "/recherche".to_string(),
        };
        // Pose la position à restaurer AVANT de naviguer : `ResultsBody` la
        // consomme à son montage et scrolle dès que la liste est peinte (pas de
        // saut, contrairement à un scroll différé après la nav qui peignait
        // d'abord le haut de page).
        if let Some(fs) = &from_search {
            restore_scroll.set(Some(fs.scroll_y));
        }
        navigate.with_value(|n| {
            n(
                &target,
                NavigateOptions {
                    replace: true,
                    ..Default::default()
                },
            )
        });
    };
    // Sous `lg`, flèche seule (le libellé passe en `aria-label`) : la rangée
    // unique mobile rend chaque pixel au titre.
    view! {
        <button
            type="button"
            on:click=on_back
            aria-label=back_label
            class="flex shrink-0 items-center gap-1.5 rounded-md border border-[var(--color-rule)] px-2.5 py-1.5 text-sm font-medium text-[var(--color-ink-muted)] transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)] lg:px-3"
        >
            <span aria-hidden="true">"←"</span>
            <span class="hidden lg:inline">{back_label}</span>
        </button>
    }
}

/// Barre decision : retour (recherche ou resultats), titre, widget de
/// navigation inter-resultats. Deux gabarits :
/// - **< lg** : rangée unique — retour + titre tronqué + prev/next + menu
///   hamburger qui replie les menus du site et le compte (rien ne disparaît,
///   tout tient sur une ligne).
/// - **≥ lg** : rangée unique 3 colonnes ; colonne centrale `minmax(0,34rem)`
///   (un plancher `20rem` faisait déborder la grille vers ~1024 px et poussait
///   les menus hors écran).
#[component]
fn DecisionTopBar(bar: DecisionBarState) -> impl IntoView {
    let nav_widget = |bar: &DecisionBarState| {
        bar.nav.clone().map(|nav| {
            view! { <DecisionNavWidget nav=nav current_id=bar.id.clone() from_search=bar.from_search.clone() /> }
        })
    };
    let mobile_widget = nav_widget(&bar);
    let desktop_widget = nav_widget(&bar);
    let mobile_title = bar.title.clone();
    let desktop_title = bar.title.clone();
    let mobile_back = bar.from_search.clone();
    let desktop_back = bar.from_search.clone();

    view! {
        <header class="sticky top-0 z-30 border-b border-[var(--color-rule)] bg-[var(--color-parchment)]/95 backdrop-blur supports-[backdrop-filter]:bg-[var(--color-parchment)]/80">
            <div class="flex h-14 items-center gap-2 px-4 sm:px-6 lg:hidden">
                <BackButton from_search=mobile_back />
                <p class="min-w-0 flex-1 truncate text-sm leading-tight font-semibold text-[var(--color-ink)]">
                    {mobile_title}
                </p>
                {mobile_widget}
                <BurgerMenu />
            </div>
            // Pistes latérales `1fr` = minmax(auto, 1fr) : leur minimum reste le
            // min-content (PAS de `min-w-0` sur la colonne gauche — il mettait
            // son minimum à 0 et son contenu `shrink-0` débordait sous le
            // titre) ; seule la colonne titre `minmax(0,…)` absorbe la
            // compression, en tronquant.
            <div class="mx-auto hidden h-14 max-w-7xl grid-cols-[1fr_minmax(0,34rem)_1fr] items-center gap-2 px-8 lg:grid">
                <div class="flex items-center gap-3">
                    <A
                        href="/"
                        attr:class="flex shrink-0 items-center gap-2 text-[var(--color-ink)] no-underline hover:opacity-90"
                    >
                        <Wordmark />
                    </A>
                    <span aria-hidden="true" class="text-[var(--color-rule)]">
                        "·"
                    </span>
                    <BackButton from_search=desktop_back />
                </div>
                <p class="min-w-0 truncate text-center text-base leading-tight font-semibold text-[var(--color-ink)]">
                    {desktop_title}
                </p>
                <div class="flex items-center justify-end gap-3">
                    {desktop_widget}
                    <span aria-hidden="true" class="text-[var(--color-rule)]">
                        "·"
                    </span>
                    <SiteNav />
                </div>
            </div>
        </header>
    }
}

/// Navigation precedent / suivant entre resultats. Port de `DecisionNavWidget` :
/// l'index courant est derive de `hit_ids.indexOf(current_id)`, la graine de la
/// cible est posee avant de naviguer (la barre de la page cible la consomme).
#[component]
fn DecisionNavWidget(
    nav: ResultNav,
    current_id: String,
    from_search: Option<crate::components::decision_bar::FromSearch>,
) -> impl IntoView {
    let navigate = StoredValue::new_local(use_navigate());
    let seed = use_result_nav();

    let idx = nav.hit_ids.iter().position(|x| *x == current_id);
    let len = nav.hit_ids.len();
    let prev_id = idx.filter(|&i| i > 0).map(|i| nav.hit_ids[i - 1].clone());
    let next_id = idx
        .filter(|&i| i + 1 < len)
        .map(|i| nav.hit_ids[i + 1].clone());

    let btn_class = "flex h-7 w-7 items-center justify-center rounded border border-[var(--color-rule)] text-[var(--color-ink-muted)] transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)]";
    let disabled_class = "flex h-7 w-7 items-center justify-center rounded border border-[var(--color-rule)] text-[var(--color-ink-subtle)] opacity-30";

    let go = {
        let hit_ids = nav.hit_ids.clone();
        let total = nav.total;
        let from_search = from_search.clone();
        move |target_idx: usize, target_id: String| {
            seed.set(Some(ResultNavSeed {
                nav: Some(ResultNav {
                    position: target_idx as i64 + 1,
                    total,
                    hit_ids: hit_ids.clone(),
                }),
                from_search: from_search.clone(),
            }));
            navigate.with_value(|n| {
                n(
                    &format!("/decision/{target_id}"),
                    NavigateOptions::default(),
                )
            });
        }
    };

    let prev_view = match (idx, prev_id) {
        (Some(i), Some(pid)) => {
            let go = go.clone();
            Either::Left(view! {
                <button
                    type="button"
                    on:click=move |_| go(i - 1, pid.clone())
                    aria-label="Résultat précédent"
                    class=btn_class
                >
                    "←"
                </button>
            })
        }
        _ => Either::Right(view! { <span class=disabled_class>"←"</span> }),
    };

    let next_view = match (idx, next_id) {
        (Some(i), Some(nid)) => {
            let go = go.clone();
            Either::Left(view! {
                <button
                    type="button"
                    on:click=move |_| go(i + 1, nid.clone())
                    aria-label="Résultat suivant"
                    class=btn_class
                >
                    "→"
                </button>
            })
        }
        _ => Either::Right(view! { <span class=disabled_class>"→"</span> }),
    };

    let position = nav.position;
    let total = nav.total;
    view! {
        <div class="flex shrink-0 items-center gap-2 text-sm text-[var(--color-ink-subtle)]">
            {prev_view}
            <span class="tabular-nums">
                {position}
                <span class="mx-1 text-[var(--color-ink-subtle)]">"/"</span>
                {total}
            </span>
            {next_view}
        </div>
    }
}

/// Lien de navigation. `A` pose `aria-current` automatiquement sur la route
/// active ; on s'appuie dessus pour le style actif via le selecteur
/// `aria-[current]`.
#[component]
fn NavItem(href: &'static str, children: Children) -> impl IntoView {
    // `text-xs px-1.5` sous `sm` : wordmark (~115 px) + 4 entrées à `text-sm
    // px-2` (~272 px) débordaient les ~358 px utiles d'un écran de 390 —
    // la nav peinte par-dessus le wordmark comprimé.
    let class = cn([
        "px-1.5 py-2 text-xs transition-colors sm:px-3 sm:text-sm",
        "text-[var(--color-ink-muted)] hover:text-[var(--color-ink)]",
        "aria-[current=page]:text-[var(--color-ink)]",
    ]);
    view! {
        <A href=href attr:class=class>
            {children()}
        </A>
    }
}

/// Bouton d'authentification. Anonyme => lien Connexion ; connecte => avatar +
/// menu compte. Bascule reactivement sur l'email de l'`AuthState`.
#[component]
fn AuthButton() -> impl IntoView {
    let auth = crate::auth::use_auth();
    move || match auth.email.get() {
        None => Either::Left(view! { <NavItem href="/connexion">"Connexion"</NavItem> }),
        Some(email) => Either::Right(view! { <AccountMenu email=email /> }),
    }
}

/// Fermeture d'un panneau flottant au clic exterieur (listener `mousedown`) et
/// a tout changement de route (ilots client, inertes au SSR).
fn close_on_outside_or_nav(open: RwSignal<bool>, container_ref: NodeRef<leptos::html::Div>) {
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

        let location = leptos_router::hooks::use_location();
        Effect::new(move |_| {
            location.pathname.track();
            open.set(false);
        });
    }
    #[cfg(not(feature = "hydrate"))]
    let _ = (open, container_ref);
}

/// Corps du menu compte (email + Profil / Mon activité / Déconnexion), partagé
/// par l'avatar (`AccountMenu`) et le hamburger mobile (`BurgerMenu`).
#[component]
fn AccountItems(email: String, open: RwSignal<bool>) -> impl IntoView {
    // `StoredValue` (Copy) : `on_sign_out` reste un closure `Copy` => `Fn`,
    // requis car il vit dans les enfants de `<Show>` (rappeles a chaque rendu).
    #[cfg(feature = "hydrate")]
    let navigate = StoredValue::new_local(use_navigate());
    let on_sign_out = move |_| {
        open.set(false);
        #[cfg(feature = "hydrate")]
        {
            let nav = navigate.get_value();
            leptos::task::spawn_local(async move {
                crate::auth::sign_out().await;
                nav(
                    "/",
                    NavigateOptions {
                        replace: true,
                        ..Default::default()
                    },
                );
            });
        }
    };

    view! {
        <div class="border-b border-[var(--color-rule)] px-3 py-2.5">
            <p class="truncate text-xs text-[var(--color-ink-subtle)]">{email}</p>
        </div>
        <div class="py-1">
            <DropdownItem href="/profil">"Profil"</DropdownItem>
            <DropdownItem href="/recherches">"Mon activité"</DropdownItem>
        </div>
        <div class="border-t border-[var(--color-rule)] py-1">
            <button
                type="button"
                on:click=on_sign_out
                class="w-full px-3 py-1.5 text-left text-sm text-[var(--color-ink-muted)] transition-colors hover:bg-[var(--color-rule)] hover:text-[var(--color-ink)]"
            >
                "Déconnexion"
            </button>
        </div>
    }
}

/// Avatar (initiale de l'email) ouvrant un menu compte flottant. Port de
/// `AuthButton` (branche connectee) de `top-bar.tsx`.
#[component]
fn AccountMenu(email: String) -> impl IntoView {
    let open = RwSignal::new(false);
    let container_ref = NodeRef::<leptos::html::Div>::new();
    close_on_outside_or_nav(open, container_ref);

    let initial = email
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string());

    view! {
        <div node_ref=container_ref class="relative">
            <button
                type="button"
                on:click=move |_| open.update(|o| *o = !*o)
                aria-label="Menu du compte"
                aria-expanded=move || open.get().then_some("true")
                class="flex h-7 w-7 items-center justify-center rounded-full bg-[var(--color-accent)] text-xs font-semibold text-white transition-opacity hover:opacity-85"
            >
                {initial}
            </button>
            <Show when=move || open.get()>
                <div class="absolute right-0 top-full z-50 mt-2 w-52 overflow-hidden rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] shadow-lg">
                    <AccountItems email=email.clone() open=open />
                </div>
            </Show>
        </div>
    }
}

/// Menu hamburger de la barre decision mobile : replie l'accueil, les menus du
/// site et le compte dans un panneau flottant, pour laisser la rangée unique
/// au retour + titre + navigation inter-résultats.
#[component]
fn BurgerMenu() -> impl IntoView {
    let open = RwSignal::new(false);
    let container_ref = NodeRef::<leptos::html::Div>::new();
    close_on_outside_or_nav(open, container_ref);
    let auth = crate::auth::use_auth();

    view! {
        <div node_ref=container_ref class="relative shrink-0">
            <button
                type="button"
                on:click=move |_| open.update(|o| *o = !*o)
                aria-label="Menu du site"
                aria-expanded=move || open.get().then_some("true")
                class="flex h-7 w-7 items-center justify-center rounded border border-[var(--color-rule)] text-[var(--color-ink-muted)] transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)]"
            >
                "☰"
            </button>
            <Show when=move || open.get()>
                <div class="absolute right-0 top-full z-50 mt-2 w-52 overflow-hidden rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] shadow-lg">
                    <div class="py-1">
                        <DropdownItem href="/">"Accueil"</DropdownItem>
                        <DropdownItem href="/recherche">"Décisions"</DropdownItem>
                        <DropdownItem href="/textes">"Textes"</DropdownItem>
                        <DropdownItem href="/mcp-guide">"MCP"</DropdownItem>
                    </div>
                    {move || match auth.email.get() {
                        None => Either::Left(view! {
                            <div class="border-t border-[var(--color-rule)] py-1">
                                <DropdownItem href="/connexion">"Connexion"</DropdownItem>
                            </div>
                        }),
                        Some(email) => Either::Right(view! {
                            <div class="border-t border-[var(--color-rule)]">
                                <AccountItems email=email open=open />
                            </div>
                        }),
                    }}
                </div>
            </Show>
        </div>
    }
}

/// Entree du menu compte. `A` pose `aria-current` sur la route active (style via
/// `aria-[current=page]`) ; la fermeture du menu est portee par l'effet de
/// navigation d'`AccountMenu`.
#[component]
fn DropdownItem(href: &'static str, children: Children) -> impl IntoView {
    let class = cn([
        "block px-3 py-1.5 text-sm transition-colors",
        "text-[var(--color-ink-muted)] hover:bg-[var(--color-rule)] hover:text-[var(--color-ink)]",
        "aria-[current=page]:text-[var(--color-ink)]",
    ]);
    view! {
        <A href=href attr:class=class>
            {children()}
        </A>
    }
}

/// Logotype textuel.
#[component]
pub fn Wordmark() -> impl IntoView {
    view! {
        <span class="flex items-baseline">
            <span
                aria-hidden="true"
                class="font-sans text-base leading-none tracking-[0.02em] text-[var(--color-ink-muted)] sm:text-xl"
                style="font-variation-settings: 'wght' 300"
            >
                "Libre"
            </span>
            <span
                aria-hidden="true"
                class="font-sans text-base leading-none tracking-[-0.02em] text-[var(--color-accent)] sm:text-xl"
                style="font-variation-settings: 'wght' 650"
            >
                "Justice"
            </span>
            <span class="sr-only">"LibreJustice"</span>
        </span>
    }
}
