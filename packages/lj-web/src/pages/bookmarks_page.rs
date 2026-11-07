//! `/bookmarks`: the account's bookmarks, fetched client-side after hydration.

use leptos::prelude::*;
use leptos_meta::{Meta, Title};
use leptos_router::components::A;

use lj_dtos::BookmarkItem;

#[cfg(feature = "hydrate")]
use crate::api::ApiClient;
use crate::auth::AuthGuard;

#[component]
pub fn BookmarksPage() -> impl IntoView {
    view! {
        <Title text="Signets - LibreJustice" />
        <Meta name="robots" content="noindex" />
        <Shell>
            <AuthGuard fallback=|| view! { <ListSkeleton /> }>
                <BookmarksList />
            </AuthGuard>
        </Shell>
    }
}

#[component]
fn Shell(children: Children) -> impl IntoView {
    view! {
        <div class="mx-auto flex w-full max-w-3xl flex-col gap-7 px-4 py-12">
            <h1
                class="font-sans text-2xl text-[var(--color-ink)]"
                style="font-variation-settings: 'wght' 300"
            >
                "Signets"
            </h1>
            {children()}
        </div>
    }
}

#[component]
fn BookmarksList() -> impl IntoView {
    let items = RwSignal::new(Vec::<BookmarkItem>::new());
    let error = RwSignal::new(None::<String>);
    let pending = RwSignal::new(true);

    Effect::new(move |_| {
        #[cfg(feature = "hydrate")]
        leptos::task::spawn_local(async move {
            match ApiClient::from_context().list_bookmarks().await {
                Ok(resp) => items.set(resp.items),
                Err(e) => error.set(Some(e.message)),
            }
            pending.set(false);
        });
    });

    move || {
        if let Some(err) = error.get() {
            view! { <p class="text-sm text-red-600">{err}</p> }.into_any()
        } else if pending.get() {
            view! { <ListSkeleton /> }.into_any()
        } else if items.with(|i| i.is_empty()) {
            view! {
                <div class="rounded-xl border border-dashed border-[var(--color-rule)] px-5 py-10 text-center">
                    <p class="text-sm text-[var(--color-ink-muted)]">
                        "Aucun signet pour l'instant. Marquez des décisions depuis leur page pour les retrouver ici."
                    </p>
                </div>
            }
                .into_any()
        } else {
            view! {
                <div class="flex flex-col gap-3">
                    <p class="flex h-5 items-center text-xs text-[var(--color-ink-subtle)]">
                        {move || {
                            let n = items.with(Vec::len);
                            format!("{n} {}", if n > 1 { "éléments" } else { "élément" })
                        }}
                    </p>
                    <ul class="flex flex-col gap-3">
                        {move || {
                            items
                                .get()
                                .into_iter()
                                .map(|item| view! { <BookmarkCard item=item items=items /> })
                                .collect_view()
                        }}
                    </ul>
                </div>
            }
            .into_any()
        }
    }
}

#[component]
fn BookmarkCard(item: BookmarkItem, items: RwSignal<Vec<BookmarkItem>>) -> impl IntoView {
    let id = item.id.clone();
    let on_delete = move |_| {
        let id = id.clone();
        items.update(|list| list.retain(|b| b.id != id));
        #[cfg(feature = "hydrate")]
        leptos::task::spawn_local(async move {
            let _ = ApiClient::from_context().remove_bookmark(&id).await;
        });
    };
    view! {
        <li class="group relative flex items-start gap-3 rounded-xl border border-[var(--color-rule)] bg-[var(--color-parchment)] p-4 transition-colors hover:border-[var(--color-ink-subtle)]">
            <div class="flex min-w-0 flex-1 flex-col gap-1.5">
                <A
                    href=lj_dtos::decision_path(item.jurisdiction_type, item.jurisdiction_code.as_deref(), &item.id)
                    attr:class="line-clamp-2 text-[15px] leading-snug font-medium text-[var(--color-ink)] transition-colors hover:text-[var(--color-accent)]"
                >
                    {item.title}
                </A>
                <div class="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-[var(--color-ink-subtle)]">
                    <span>{format!("ajouté {}", relative_time(&item.bookmarked_at))}</span>
                </div>
            </div>
            <button
                type="button"
                on:click=on_delete
                aria-label="Retirer le signet"
                title="Retirer le signet"
                class="flex h-6 w-6 shrink-0 items-center justify-center rounded-full text-[var(--color-ink-subtle)] opacity-60 transition-all hover:bg-[var(--color-vellum)] hover:text-[var(--color-ink)] hover:opacity-100"
            >
                <svg aria-hidden="true" viewBox="0 0 16 16" class="h-3.5 w-3.5">
                    <path
                        d="M4 4l8 8M12 4l-8 8"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="1.4"
                        stroke-linecap="round"
                    ></path>
                </svg>
            </button>
        </li>
    }
}

#[component]
fn ListSkeleton() -> impl IntoView {
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

/// The ISO string as is on the server, where the shell stays inert.
fn relative_time(iso: &str) -> String {
    #[cfg(feature = "hydrate")]
    {
        let parsed = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(iso)).get_time();
        if parsed.is_nan() {
            return iso.to_string();
        }
        let diff_sec = ((js_sys::Date::now() - parsed) / 1000.0).max(0.0);
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
