//! Barre superieure. Deux etats, choisis
//! reactivement via le contexte `DecisionBar` : barre par defaut (wordmark +
//! hamburger) ou barre decision (retour + titre + navigation inter-resultats).
//!
//! Les menus du site et le compte vivent dans le hamburger, a toutes les
//! tailles. Le panneau lit l'`AuthState` reactif : anonyme => lien Connexion ;
//! connecte => menu compte (Profil / Mon activite / Deconnexion). L'email est
//! `None` au SSR (session locale) puis peuple cote client : premier rendu =
//! Connexion, swap apres hydratation.

use leptos::either::{Either, EitherOf3};
use leptos::prelude::*;
use leptos_router::components::{ToHref, A};
use leptos_router::hooks::{use_location, use_navigate};
use leptos_router::NavigateOptions;
use lj_dtos::UiAction;

use crate::components::decision_bar::{
    seeded, use_decision_bar, DecisionBarState, ResultNav, ResultNavSeed, WorkbenchSignal,
};
use crate::helpers::cn;

#[component]
pub fn TopBar() -> impl IntoView {
    let bar = use_decision_bar();
    let workbench = expect_context::<WorkbenchSignal>();
    let open_document = move || {
        workbench
            .get()
            .filter(|session| session.status.get() == crate::pages::redact::Status::Ready)
    };
    move || match (open_document(), bar.get()) {
        (Some(session), _) => EitherOf3::A(view! { <WorkbenchTopBar session=session /> }),
        (None, Some(state)) => EitherOf3::B(view! { <DecisionTopBar bar=state /> }),
        (None, None) => EitherOf3::C(view! { <DefaultTopBar /> }),
    }
}

/// The anonymisation workbench with a document open: its controls take the
/// bar, the wordmark giving way below `lg`.
#[component]
fn WorkbenchTopBar(session: crate::pages::redact::Session) -> impl IntoView {
    view! {
        <header class="sticky top-0 z-30 border-b border-[var(--color-rule)] bg-[var(--color-parchment)]/95 shadow-sm backdrop-blur supports-[backdrop-filter]:bg-[var(--color-parchment)]/80">
            <div class="flex h-14 items-center gap-2 px-4 sm:gap-3 sm:px-6 lg:px-8">
                <A
                    href="/"
                    attr:class="hidden shrink-0 items-center text-[var(--color-ink)] no-underline hover:opacity-90 lg:flex"
                >
                    <Wordmark />
                </A>
                <crate::pages::redact::Toolbar session=session />
                <BurgerMenu />
            </div>
        </header>
    }
}

/// The default bar, on every page but a decision's: wordmark and burger, one `h-14` row at every width.
#[component]
fn DefaultTopBar() -> impl IntoView {
    view! {
        <header class="sticky top-0 z-30 border-b border-[var(--color-rule)] bg-[var(--color-parchment)]/95 shadow-sm backdrop-blur supports-[backdrop-filter]:bg-[var(--color-parchment)]/80">
            <div class="mx-auto flex h-14 max-w-7xl items-center justify-between gap-2 px-4 sm:gap-6 sm:px-6 lg:px-8">
                <A
                    href="/"
                    attr:class="flex min-w-0 items-center gap-2 text-[var(--color-ink)] no-underline hover:opacity-90"
                >
                    <Wordmark />
                </A>
                <BurgerMenu />
            </div>
        </header>
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
            Some(fs) => format!("/decision-search{}", fs.search),
            None => "/decision-search".to_string(),
        };
        // Pose la position à restaurer AVANT de naviguer : `ResultsBody` la consomme
        // à son montage et scrolle dès que la liste est peinte.
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
    // Sous `lg`, flèche seule : le libellé passe en `aria-label`.
    view! {
        <button
            type="button"
            on:click=on_back
            data-usage=UiAction::DecisionBackToResults.as_str()
            aria-label=back_label
            class="flex shrink-0 items-center gap-1.5 rounded-md border border-[var(--color-rule)] px-2.5 py-1.5 text-sm font-medium text-[var(--color-ink-muted)] transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)] lg:px-3"
        >
            <span aria-hidden="true">"←"</span>
            <span class="hidden lg:inline">{back_label}</span>
        </button>
    }
}

/// Barre decision : retour (recherche ou resultats), titre, widget de
/// navigation inter-resultats. Les menus du site et le compte sont repliés
/// dans le hamburger à toutes les tailles : la rangée appartient au titre.
/// Deux gabarits :
/// - < lg : rangée unique — retour + titre tronqué + prev/next + hamburger.
/// - ≥ lg : rangée unique 3 colonnes ; colonne centrale `minmax(0,44rem)`.
#[component]
fn DecisionTopBar(bar: DecisionBarState) -> impl IntoView {
    let nav_widget = |bar: &DecisionBarState| {
        bar.nav.clone().map(|nav| {
            view! { <DecisionNavWidget nav=nav current_path=bar.path.clone() from_search=bar.from_search.clone() /> }
        })
    };
    let mobile_widget = nav_widget(&bar);
    let desktop_widget = nav_widget(&bar);
    let mobile_title = bar.title.clone();
    let desktop_title = bar.title.clone();
    let mobile_back = bar.from_search.clone();
    let desktop_back = bar.from_search.clone();

    view! {
        <header class="sticky top-0 z-30 border-b border-[var(--color-rule)] bg-[var(--color-parchment)]/95 shadow-sm backdrop-blur supports-[backdrop-filter]:bg-[var(--color-parchment)]/80">
            <div class="flex h-14 items-center gap-2 px-4 sm:px-6 lg:hidden">
                <BackButton from_search=mobile_back />
                <p class="min-w-0 flex-1 truncate text-center text-sm leading-tight font-semibold text-[var(--color-ink)]">
                    {mobile_title}
                </p>
                {mobile_widget}
                <BurgerMenu />
            </div>
            // Pistes latérales `1fr` = minmax(auto, 1fr) : leur minimum reste le
            // min-content ; seule la colonne titre `minmax(0,…)` absorbe la compression,
            // en tronquant.
            <div class="mx-auto hidden h-14 max-w-7xl grid-cols-[1fr_minmax(0,44rem)_1fr] items-center gap-2 px-8 lg:grid">
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
                    <BurgerMenu />
                </div>
            </div>
        </header>
    }
}

/// Navigation precedent / suivant entre resultats : l'index courant est derive
/// de la position dans `hit_paths`, la graine de la
/// cible part avec son entree d'historique (la barre de la page cible la lit).
#[component]
fn DecisionNavWidget(
    nav: ResultNav,
    current_path: String,
    from_search: Option<crate::components::decision_bar::FromSearch>,
) -> impl IntoView {
    let navigate = StoredValue::new_local(use_navigate());

    let idx = nav.hit_paths.iter().position(|x| *x == current_path);
    let len = nav.hit_paths.len();
    let prev_path = idx.filter(|&i| i > 0).map(|i| nav.hit_paths[i - 1].clone());
    let next_path = idx
        .filter(|&i| i + 1 < len)
        .map(|i| nav.hit_paths[i + 1].clone());
    // Décision hors de la liste servie : le compteur n'a pas de rang à afficher
    // et les deux flèches seraient mortes.
    let Some(rang) = idx else {
        return ().into_any();
    };

    let btn_class = "flex h-7 w-7 items-center justify-center rounded border border-[var(--color-rule)] text-[var(--color-ink-muted)] transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)]";
    let disabled_class = "flex h-7 w-7 items-center justify-center rounded border border-[var(--color-rule)] text-[var(--color-ink-subtle)] opacity-30";

    let go = {
        let hit_paths = nav.hit_paths.clone();
        let from_search = from_search.clone();
        move |target: String| {
            let seed = ResultNavSeed {
                nav: Some(ResultNav {
                    hit_paths: hit_paths.clone(),
                }),
                from_search: from_search.clone(),
            };
            navigate.with_value(|n| n(&target, seeded(&seed)));
        }
    };

    let prev_view = match prev_path {
        Some(pid) => {
            let go = go.clone();
            Either::Left(view! {
                <button
                    type="button"
                    on:click=move |_| go(pid.clone())
                    data-usage=UiAction::DecisionResultStep.as_str()
                    aria-label="Résultat précédent"
                    class=btn_class
                >
                    "←"
                </button>
            })
        }
        None => Either::Right(view! { <span class=disabled_class>"←"</span> }),
    };

    let next_view = match next_path {
        Some(nid) => {
            let go = go.clone();
            Either::Left(view! {
                <button
                    type="button"
                    on:click=move |_| go(nid.clone())
                    data-usage=UiAction::DecisionResultStep.as_str()
                    aria-label="Résultat suivant"
                    class=btn_class
                >
                    "→"
                </button>
            })
        }
        None => Either::Right(view! { <span class=disabled_class>"→"</span> }),
    };

    let position = rang + 1;
    let total = len;
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
    .into_any()
}

pub(crate) fn close_on_outside_or_nav(
    open: RwSignal<bool>,
    container_ref: NodeRef<leptos::html::Div>,
) {
    #[cfg(feature = "hydrate")]
    {
        use crate::components::ui::panel::{dismiss, Dismiss};
        dismiss(
            container_ref,
            move || open.get_untracked(),
            move || open.set(false),
            Dismiss::default(),
        );

        let location = leptos_router::hooks::use_location();
        Effect::new(move |_| {
            location.pathname.track();
            open.set(false);
        });
    }
    #[cfg(not(feature = "hydrate"))]
    let _ = (open, container_ref);
}

/// Corps du menu compte (email + Profil / Signets / Déconnexion), affiché
/// dans le panneau du hamburger (`BurgerMenu`) quand une session est active.
#[component]
fn AccountItems(email: String, open: RwSignal<bool>) -> impl IntoView {
    // `StoredValue` (Copy) : `on_sign_out` doit rester `Copy` pour être `Fn`, il
    // vit dans les enfants de `<Show>` (rappeles a chaque rendu).
    #[cfg(feature = "hydrate")]
    let navigate = StoredValue::new_local(use_navigate());
    #[cfg(feature = "hydrate")]
    let auth = crate::auth::use_auth();
    let on_sign_out = move |_| {
        open.set(false);
        #[cfg(feature = "hydrate")]
        {
            let nav = navigate.get_value();
            leptos::task::spawn_local(async move {
                crate::auth::sign_out().await;
                // Le pilote d'auth relit l'identite, vide le cache de requetes et
                // remet le theme clair.
                auth.bump();
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
            <DropdownItem href="/profile">"Profil"</DropdownItem>
            <div data-usage=UiAction::MenuBookmarks.as_str()>
                <DropdownItem href="/bookmarks">"Signets"</DropdownItem>
            </div>
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

/// Menu hamburger : replie l'accueil, les menus du site et le compte dans un
/// panneau flottant. Unique point d'entrée de la navigation, sur toutes les
/// barres et à toutes les tailles.
#[component]
fn BurgerMenu() -> impl IntoView {
    let open = RwSignal::new(false);
    let container_ref = NodeRef::<leptos::html::Div>::new();
    close_on_outside_or_nav(open, container_ref);
    let auth = crate::auth::use_auth();
    let login_href = login_href();

    view! {
        <div node_ref=container_ref class="relative shrink-0">
            // Déclencheur, même cadre que le bouton retour « ← Recherche ».
            // Morphing hamburger ⇄ croix à emprise constante : géométrie et courbes dans
            // `main.css` (`.lj-burger-*`). Libellé « Menu » sur desktop uniquement.
            <button
                type="button"
                on:click=move |_| open.update(|o| *o = !*o)
                aria-label="Menu du site"
                aria-expanded=move || open.get().then_some("true")
                class="flex shrink-0 items-center gap-2 rounded-md border border-[var(--color-rule)] px-2.5 py-1.5 text-sm font-medium text-[var(--color-ink-muted)] transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)]"
            >
                <span
                    aria-hidden="true"
                    class="relative flex h-4 w-4 items-center justify-center"
                    class=("lj-burger-open", move || open.get())
                >
                    <span class="lj-burger-line lj-burger-line-top"></span>
                    <span class="lj-burger-line lj-burger-line-bottom"></span>
                </span>
                <span class="hidden lg:inline">"Menu"</span>
            </button>
            <div
                data-open=move || open.get().then_some("true")
                class="lj-panel absolute right-0 top-full z-50 mt-2 w-52 overflow-hidden rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] shadow-lg"
            >
                <div class="py-1">
                        <DropdownItem href="/">"Accueil"</DropdownItem>
                        <DropdownItem href="/decision-search">"Décisions"</DropdownItem>
                        <DropdownItem href="/norm-search">"Textes"</DropdownItem>
                        <DropdownItem href="/entity-search">"Annuaire"</DropdownItem>
                        <DropdownItem href="/notice-search">"Annonces"</DropdownItem>
                        <DropdownItem href="/tools">"Outils"</DropdownItem>
                    </div>
                    {move || match auth.email.get() {
                        None => Either::Left(view! {
                            <div class="border-t border-[var(--color-rule)] py-1">
                                <DropdownItem href=login_href>"Connexion"</DropdownItem>
                            </div>
                        }),
                        Some(email) => Either::Right(view! {
                            <div class="border-t border-[var(--color-rule)]">
                                <AccountItems email=email open=open />
                            </div>
                        }),
                    }}
            </div>
        </div>
    }
}

/// `href` du lien Connexion : `/login?next=<page courante>`, pour revenir
/// d'ou l'on vient une fois authentifie. Depuis les pages d'auth elles-memes,
/// pas de `next` — la destination serait la page de connexion.
fn login_href() -> impl Fn() -> String + Copy + Send + Sync + 'static {
    let location = use_location();
    let (pathname, search) = (location.pathname, location.search);
    move || {
        let path = pathname.get();
        if path.starts_with("/login") || path.starts_with("/reset-password") {
            return "/login".to_string();
        }
        let search = search.get();
        let target = if search.is_empty() {
            path
        } else {
            format!("{path}?{search}")
        };
        format!(
            "/login?next={}",
            leptos_router::location::Url::escape(&target)
        )
    }
}

/// Entree du menu compte. `A` pose `aria-current` sur la route active (style via
/// `aria-[current=page]`) ; la fermeture du menu est portee par l'effet de
/// navigation de `close_on_outside_or_nav`.
#[component]
fn DropdownItem<H>(href: H, children: Children) -> impl IntoView
where
    H: ToHref + Send + Sync + 'static,
{
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
