//! Pages `/activite/{recherches,lectures,signets}` — port de `activity-page.tsx`.
//!
//! Un seul composant `ActivityPage`, le panneau actif est choisi via le path
//! (`ActivePanel`). Tout est client-side (token Supabase en localStorage, absent
//! au SSR) ; les listes ne fetchent qu'apres hydratation, derriere `AuthGuard`.
//! Les listes paginees (historique, lectures) montent un scroll infini ; les
//! signets sont une liste simple.

#[path = "filters.rs"]
mod filters;

use leptos::prelude::*;
use leptos::reactive::owner::LocalStorage;
use leptos_meta::{Meta, Title};

use lj_dtos::{
    BookmarkItem, DecisionViewItem, DecisionViewsResponse, SearchHistoryEntry,
    SearchHistoryResponse, UserProfile,
};

use crate::api::{ApiClient, PageParams};
use crate::auth::AuthGuard;
use crate::components::activity::activity_ui::{
    relative_time, ActivityCard, ActivityListSkeleton, ActivityShell, CardList, EmptyState,
    ErrorLine, FilterChip, InfiniteSentinel, PanelToolbar, Sep, SourceBadge, Switch,
};

/// Taille de page des listes (port de `ACTIVITY_PAGE_SIZE`).
const PAGE_SIZE: u32 = 50;

#[component]
pub fn ActivityPage() -> impl IntoView {
    view! {
        <Title text="Mon activité - LibreJustice" />
        <Meta name="robots" content="noindex" />
        <AuthGuard fallback=|| {
            view! {
                <ActivityShell>
                    <ActivityListSkeleton />
                </ActivityShell>
            }
        }>
            <ActivityShell aside=view! { <ActivityTrackingToggle /> }.into_any()>
                <ActivePanel />
            </ActivityShell>
        </AuthGuard>
    }
}

/// Construit un client API (base resolue selon la cible).
fn client() -> ApiClient {
    ApiClient::from_context()
}

/// Charge le profil cote client dans un signal mutable (cache de page).
fn profile_signal() -> RwSignal<Option<UserProfile>> {
    let profile = RwSignal::new(None::<UserProfile>);
    Effect::new(move |_| {
        #[cfg(feature = "hydrate")]
        leptos::task::spawn_local(async move {
            if let Ok(p) = client().fetch_me().await {
                profile.set(Some(p));
            }
        });
    });
    profile
}

/// Toggle d'enregistrement d'activite (aside). Couper purge tout (ADR 0056).
#[component]
fn ActivityTrackingToggle() -> impl IntoView {
    let profile = profile_signal();
    let pending = RwSignal::new(false);

    let enabled = Signal::derive(move || profile.get().map(|p| p.track_activity).unwrap_or(false));

    let on_toggle = move || {
        #[cfg(feature = "hydrate")]
        {
            use crate::pages::login_page::browser;
            let currently = enabled.get_untracked();
            if currently
                && !browser::confirm_native(
                    "Désactiver l'enregistrement effacera vos recherches, lectures et signets, \
                     et n'en gardera plus aucun. Continuer ?",
                )
            {
                return;
            }
            pending.set(true);
            leptos::task::spawn_local(async move {
                if let Ok(updated) = client().set_activity_tracking(!currently).await {
                    profile.set(Some(updated));
                }
                pending.set(false);
            });
        }
    };

    view! {
        <Show when=move || profile.get().is_some()>
            <span
                class="flex items-center gap-2 text-sm text-[var(--color-ink-muted)]"
                title="Conserve vos recherches, lectures et signets. Désactiver efface tout et n'enregistre plus rien."
            >
                "Enregistrer mon activité"
                <Switch
                    checked=enabled
                    on_change=on_toggle
                    disabled=pending
                    label="Enregistrer mon activité"
                />
            </span>
        </Show>
    }
}

/// Selecteur de panneau selon le path (port de `ActivePanel`).
#[component]
fn ActivePanel() -> impl IntoView {
    let pathname = current_pathname();
    move || {
        let path = pathname.get();
        if path.starts_with("/activite/signets") {
            view! { <BookmarksPanel /> }.into_any()
        } else if path.starts_with("/activite/lectures") {
            view! { <ViewsPanel /> }.into_any()
        } else {
            view! { <HistoryPanel /> }.into_any()
        }
    }
}

/// Path courant (vide au SSR : la coquille est inerte avant hydratation).
fn current_pathname() -> Signal<String> {
    #[cfg(feature = "hydrate")]
    {
        let loc = leptos_router::hooks::use_location();
        Signal::derive(move || loc.pathname.get())
    }
    #[cfg(not(feature = "hydrate"))]
    {
        Signal::derive(String::new)
    }
}

// ── Historique de recherche ──────────────────────────────────────────────────

#[component]
fn HistoryPanel() -> impl IntoView {
    let state = InfiniteState::<SearchHistoryEntry>::new();
    state.start(|page| async move {
        client()
            .list_search_history(page)
            .await
            .map(unpack_history)
            .map_err(|e| e.message)
    });

    let on_clear = Callback::new(move |_| {
        #[cfg(feature = "hydrate")]
        {
            use crate::pages::login_page::browser;
            if browser::confirm_native("Effacer tout l'historique ?") {
                leptos::task::spawn_local(async move {
                    let _ = client().clear_search_history().await;
                });
                state.clear_local();
            }
        }
    });

    let render_card = move |entry: SearchHistoryEntry| {
        let id = entry.id;
        let described = filters::describe_filters(&entry.filters);
        let created = entry.created_at.clone();
        // Relance vers le moteur d'origine (ADR 0251).
        let engine_path = match entry.engine {
            lj_dtos::SearchEngine::Decisions => "/decisions",
            lj_dtos::SearchEngine::Textes => "/textes",
        };
        let to = format!("{engine_path}?q={}", filters::encode_query(&entry.query));
        let meta = view! {
            <span>{relative_time(&created)}</span>
            {(!described.is_empty())
                .then(|| {
                    view! {
                        <Sep />
                        {described
                            .into_iter()
                            .map(|f| view! { <FilterChip label=f.label value=f.value /> })
                            .collect_view()}
                    }
                })}
        }
        .into_any();
        view! {
            <ActivityCard
                to=to
                title=entry.query.clone()
                badge=view! { <SourceBadge source=entry.source /> }.into_any()
                on_delete=move || {
                    state.remove_by(move |e| e.id != id);
                    #[cfg(feature = "hydrate")]
                    leptos::task::spawn_local(async move {
                        let _ = client().delete_search_history_entry(id).await;
                    });
                }
                delete_label="Supprimer cette recherche"
                meta=meta
            />
        }
    };

    infinite_view(
        state,
        "Aucune recherche enregistrée. Vos recherches connectées — depuis le site ou via MCP — apparaîtront ici.",
        Some(on_clear),
        render_card,
    )
}

// ── Lectures ──────────────────────────────────────────────────────────────────

#[component]
fn ViewsPanel() -> impl IntoView {
    let state = InfiniteState::<DecisionViewItem>::new();
    state.start(|page| async move {
        client()
            .list_decision_views(page)
            .await
            .map(unpack_views)
            .map_err(|e| e.message)
    });

    let on_clear = Callback::new(move |_| {
        #[cfg(feature = "hydrate")]
        {
            use crate::pages::login_page::browser;
            if browser::confirm_native("Effacer tout l'historique de lecture ?") {
                leptos::task::spawn_local(async move {
                    let _ = client().clear_decision_views().await;
                });
                state.clear_local();
            }
        }
    });

    let render_card = move |item: DecisionViewItem| {
        let id = item.id.clone();
        let del_id = item.id.clone();
        let viewed = item.last_viewed_at.clone();
        let count = item.view_count;
        let meta = view! {
            <span>{format!("lu {}", relative_time(&viewed))}</span>
            {(count > 1)
                .then(|| {
                    view! {
                        <Sep />
                        <span>{format!("{count} fois")}</span>
                    }
                })}
        }
        .into_any();
        view! {
            <ActivityCard
                to=format!("/decision/{id}")
                title=item.title.clone()
                badge=view! { <SourceBadge source=item.last_source /> }.into_any()
                on_delete=move || {
                    let del_id2 = del_id.clone();
                    state.remove_by(move |v| v.id != del_id2);
                    #[cfg(feature = "hydrate")]
                    {
                        let del_id = del_id.clone();
                        leptos::task::spawn_local(async move {
                            let _ = client().delete_decision_view(&del_id).await;
                        });
                    }
                }
                delete_label="Retirer de l'historique de lecture"
                meta=meta
            />
        }
    };

    infinite_view(
        state,
        "Aucune lecture pour l'instant. Les décisions que vous ouvrez — depuis le site ou via MCP — apparaîtront ici.",
        Some(on_clear),
        render_card,
    )
}

// ── Signets ──────────────────────────────────────────────────────────────────

#[component]
fn BookmarksPanel() -> impl IntoView {
    let items = RwSignal::new(Vec::<BookmarkItem>::new());
    let error = RwSignal::new(None::<String>);
    let pending = RwSignal::new(true);

    Effect::new(move |_| {
        #[cfg(feature = "hydrate")]
        leptos::task::spawn_local(async move {
            match client().list_bookmarks().await {
                Ok(resp) => {
                    items.set(resp.items);
                    pending.set(false);
                }
                Err(e) => {
                    error.set(Some(e.message));
                    pending.set(false);
                }
            }
        });
    });

    let count = Signal::derive(move || items.with(|i| i.len() as i64));

    view! {
        {move || {
            if let Some(err) = error.get() {
                view! { <ErrorLine>{err}</ErrorLine> }.into_any()
            } else if pending.get() {
                view! { <ActivityListSkeleton /> }.into_any()
            } else if items.with(|i| i.is_empty()) {
                view! {
                    <EmptyState>
                        "Aucun signet pour l'instant. Marquez des décisions depuis leur page pour les retrouver ici."
                    </EmptyState>
                }
                    .into_any()
            } else {
                view! {
                    <div class="flex flex-col gap-3">
                        <PanelToolbar count=count />
                        <CardList>
                            {move || {
                                items
                                    .get()
                                    .into_iter()
                                    .map(|item| {
                                        let del_id = item.id.clone();
                                        let added = item.bookmarked_at.clone();
                                        view! {
                                            <ActivityCard
                                                to=format!("/decision/{}", item.id)
                                                title=item.title.clone()
                                                on_delete=move || {
                                                    let del_id2 = del_id.clone();
                                                    items.update(|list| list.retain(|b| b.id != del_id2));
                                                    #[cfg(feature = "hydrate")]
                                                    {
                                                        let del_id = del_id.clone();
                                                        leptos::task::spawn_local(async move {
                                                            let _ = client().remove_bookmark(&del_id).await;
                                                        });
                                                    }
                                                }
                                                delete_label="Retirer le signet"
                                                meta=view! {
                                                    <span>{format!("ajouté {}", relative_time(&added))}</span>
                                                }
                                                    .into_any()
                                            />
                                        }
                                    })
                                    .collect_view()
                            }}
                        </CardList>
                    </div>
                }
                    .into_any()
            }
        }}
    }
}

// ── Vue commune des panneaux infinis (historique + lectures) ───────────────────

/// Rend un panneau infini (toolbar + cartes + sentinelle), partage entre
/// historique et lectures (memes etats error/pending/empty/liste).
fn infinite_view<T, V>(
    state: InfiniteState<T>,
    empty_text: &'static str,
    on_clear: Option<Callback<()>>,
    render_card: impl Fn(T) -> V + Copy + Send + Sync + 'static,
) -> impl IntoView
where
    T: Clone + 'static,
    V: IntoView + 'static,
{
    move || {
        if let Some(err) = state.error.get() {
            view! { <ErrorLine>{err}</ErrorLine> }.into_any()
        } else if state.is_pending() {
            view! { <ActivityListSkeleton /> }.into_any()
        } else if state.items.with(|i| i.is_empty()) {
            view! { <EmptyState>{empty_text}</EmptyState> }.into_any()
        } else {
            view! {
                <div class="flex flex-col gap-3">
                    {match on_clear {
                        Some(cb) => {
                            view! {
                                <PanelToolbar count=state.total clear_label="Tout effacer" on_clear=cb />
                            }
                                .into_any()
                        }
                        None => view! { <PanelToolbar count=state.total /> }.into_any(),
                    }}
                    <CardList>
                        {move || state.items.get().into_iter().map(render_card).collect_view()}
                    </CardList>
                    <InfiniteSentinel
                        has_more=state.has_more()
                        is_loading=state.loading
                        on_reach=move || state.fetch_next()
                    />
                </div>
            }
                .into_any()
        }
    }
}

// ── Etat de scroll infini (port de pagination.ts + onMutate optimistes) ────────

type PageFut<T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(Vec<T>, i64), String>>>>;
type Fetcher<T> = std::rc::Rc<dyn Fn(PageParams) -> PageFut<T>>;

/// Etat d'une liste infinie : items accumules, total, offset suivant, drapeaux.
/// leptos-fetch 0.4.10 n'expose pas de query infinie ; on compose ici un
/// equivalent fidele de `useInfiniteQuery` (signal de pages + fetch manuel),
/// suffisant car ces listes sont purement client-side. Les items et le fetcher
/// sont en `LocalStorage` (non-`Send`, normal cote wasm mono-thread) ; les
/// drapeaux scalaires restent en stockage par defaut.
struct InfiniteState<T: Clone + 'static> {
    items: RwSignal<Vec<T>, LocalStorage>,
    total: RwSignal<i64>,
    loaded: RwSignal<u32>,
    loading: RwSignal<bool>,
    started: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    fetcher: StoredValue<Option<Fetcher<T>>, LocalStorage>,
}

impl<T: Clone + 'static> Clone for InfiniteState<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: Clone + 'static> Copy for InfiniteState<T> {}

impl<T: Clone + 'static> InfiniteState<T> {
    fn new() -> Self {
        Self {
            items: RwSignal::new_local(Vec::new()),
            total: RwSignal::new(0),
            loaded: RwSignal::new(0),
            loading: RwSignal::new(false),
            started: RwSignal::new(false),
            error: RwSignal::new(None),
            fetcher: StoredValue::new_local(None),
        }
    }

    /// Memorise le fetcher et charge la 1re page (cote client uniquement).
    fn start<Fut, F>(self, fetch: F)
    where
        F: Fn(PageParams) -> Fut + 'static,
        Fut: std::future::Future<Output = Result<(Vec<T>, i64), String>> + 'static,
    {
        let fetch: Fetcher<T> = std::rc::Rc::new(move |page| Box::pin(fetch(page)));
        self.fetcher.set_value(Some(fetch));
        #[cfg(feature = "hydrate")]
        Effect::new(move |_| {
            if self.started.get_untracked() {
                return;
            }
            self.started.set(true);
            self.load_page(0);
        });
    }

    fn is_pending(&self) -> bool {
        !self.started.get() || (self.loading.get() && self.loaded.get() == 0)
    }

    fn has_more(self) -> Signal<bool> {
        let loaded = self.loaded;
        let total = self.total;
        Signal::derive(move || (loaded.get() as i64) < total.get())
    }

    /// Charge la page suivante (offset = nombre d'items deja charges).
    fn fetch_next(self) {
        if self.loading.get_untracked() {
            return;
        }
        let loaded = self.loaded.get_untracked();
        if (loaded as i64) >= self.total.get_untracked() {
            return;
        }
        self.load_page(loaded);
    }

    fn load_page(self, offset: u32) {
        let Some(fetch) = self.fetcher.get_value() else {
            return;
        };
        self.loading.set(true);
        leptos::task::spawn_local(async move {
            let page = PageParams {
                limit: PAGE_SIZE,
                offset,
            };
            match fetch(page).await {
                Ok((items, total)) => {
                    let added = items.len() as u32;
                    self.items.update(|acc| acc.extend(items));
                    self.total.set(total);
                    self.loaded.update(|l| *l += added);
                }
                Err(msg) => self.error.set(Some(msg)),
            }
            self.loading.set(false);
        });
    }

    /// Retrait optimiste : conserve les items pour lesquels `keep` est vrai,
    /// decremente total + compteur charge (port de `dropFromPages`).
    fn remove_by(self, keep: impl Fn(&T) -> bool) {
        let before = self.items.with_untracked(Vec::len);
        self.items.update(|list| list.retain(&keep));
        let removed = before - self.items.with_untracked(Vec::len);
        self.total.update(|t| *t = (*t - removed as i64).max(0));
        self.loaded
            .update(|l| *l = l.saturating_sub(removed as u32));
    }

    /// Vide la liste localement apres une purge serveur (action « tout effacer »,
    /// uniquement cote client).
    #[cfg(feature = "hydrate")]
    fn clear_local(self) {
        self.items.set(Vec::new());
        self.total.set(0);
        self.loaded.set(0);
    }
}

fn unpack_history(resp: SearchHistoryResponse) -> (Vec<SearchHistoryEntry>, i64) {
    (resp.items, resp.total)
}

fn unpack_views(resp: DecisionViewsResponse) -> (Vec<DecisionViewItem>, i64) {
    (resp.items, resp.total)
}
