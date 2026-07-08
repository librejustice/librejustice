//! `Pagination` (port de `pagination.tsx`). Fenêtre de pages avec ellipses ;
//! chaque page est un point d'historique (replace=false) ; scroll top au clic.

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_query_map};

use crate::helpers::cn;

use super::compact_search::query_state;

/// Élément de la fenêtre de pagination : un numéro de page ou une ellipse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageItem {
    Page(u32),
    Ellipsis,
}

/// Fenêtre `[1] … [current±2] … [total]`. Port verbatim de `buildPageRange`.
fn build_page_range(current: u32, total: u32) -> Vec<PageItem> {
    if total <= 7 {
        return (1..=total).map(PageItem::Page).collect();
    }
    let mut pages: Vec<PageItem> = Vec::new();
    let push_page = |pages: &mut Vec<PageItem>, p: u32| {
        if !pages.contains(&PageItem::Page(p)) {
            pages.push(PageItem::Page(p));
        }
    };
    push_page(&mut pages, 1);
    let start = (current.saturating_sub(2)).max(2);
    let end = (current + 2).min(total - 1);
    if start > 2 {
        pages.push(PageItem::Ellipsis);
    }
    for p in start..=end {
        push_page(&mut pages, p);
    }
    if end < total - 1 {
        pages.push(PageItem::Ellipsis);
    }
    push_page(&mut pages, total);
    pages
}

type NavFn = Box<dyn Fn(&str, leptos_router::NavigateOptions) + Send + Sync>;

/// Navigue vers `page` (drop `page` si 1) + scroll top. Extrait pour rester Copy
/// dans les `Callback`.
fn navigate_to_page(
    nav: StoredValue<NavFn>,
    query_map: leptos::prelude::Memo<leptos_router::params::ParamsMap>,
    page: u32,
) {
    let qs = if page == 1 {
        query_state::with_param(&query_map.get_untracked(), "page", None, false)
    } else {
        query_state::with_param(
            &query_map.get_untracked(),
            "page",
            Some(&page.to_string()),
            false,
        )
    };
    nav.with_value(|n| {
        n(
            &query_state::search_href(&qs),
            leptos_router::NavigateOptions::default(),
        )
    });
    scroll_to_top();
}

#[cfg(feature = "hydrate")]
fn scroll_to_top() {
    if let Some(window) = web_sys::window() {
        window.scroll_to_with_x_and_y(0.0, 0.0);
    }
}

#[cfg(feature = "ssr")]
fn scroll_to_top() {}

#[component]
pub fn Pagination(
    #[prop(into)] current_page: Signal<u32>,
    #[prop(into)] total_pages: Signal<u32>,
) -> impl IntoView {
    let query_map = use_query_map();
    // `navigate` (Send+Sync) stocké en `StoredValue` (SyncStorage) : le *handle*
    // est Copy, donc les `Callback::new` (qui exigent Send+Sync) peuvent le
    // capturer. `query_map` (Memo) est déjà Copy. NB : `new_local` (LocalStorage)
    // donnerait une valeur attachée à l'owner courant, introuvable depuis le
    // contexte détaché d'un `Callback` → `with_value` no-op silencieux (la nav ne
    // partait pas). SyncStorage rend la valeur lisible partout.
    let nav: StoredValue<NavFn> = StoredValue::new(Box::new(use_navigate()) as NavFn);
    // Helper Copy (capture seulement `nav` + `query_map`, deux handles Copy) :
    // reconstruit dans chaque `Callback::new` sans déplacer un état partagé.
    let go_to = move |page: u32| navigate_to_page(nav, query_map, page);

    view! {
        <Show when=move || { total_pages.get() > 1 }>
            <nav aria-label="Pagination" class="flex items-center justify-center gap-1 pt-4">
                <PageButton
                    label="←"
                    aria_label="Page précédente"
                    disabled=Signal::derive(move || current_page.get() == 1)
                    active=Signal::derive(|| false)
                    on_click=Callback::new(move |_| go_to(current_page.get() - 1))
                />
                {move || {
                    build_page_range(current_page.get(), total_pages.get())
                        .into_iter()
                        .enumerate()
                        .map(|(i, item)| match item {
                            PageItem::Ellipsis => {
                                view! {
                                    <span
                                        class="px-2 text-sm text-[var(--color-ink-subtle)]"
                                        data-key=format!("e{i}")
                                    >
                                        "…"
                                    </span>
                                }
                                    .into_any()
                            }
                            PageItem::Page(p) => {
                                view! {
                                    <PageButton
                                        label=p.to_string()
                                        aria_label=format!("Page {p}")
                                        disabled=Signal::derive(|| false)
                                        active=Signal::derive(move || current_page.get() == p)
                                        on_click=Callback::new(move |_| go_to(p))
                                    />
                                }
                                    .into_any()
                            }
                        })
                        .collect::<Vec<_>>()
                }}
                <PageButton
                    label="→"
                    aria_label="Page suivante"
                    disabled=Signal::derive(move || { current_page.get() == total_pages.get() })
                    active=Signal::derive(|| false)
                    on_click=Callback::new(move |_| go_to(current_page.get() + 1))
                />
            </nav>
        </Show>
    }
}

#[component]
fn PageButton(
    #[prop(into)] label: String,
    #[prop(into)] aria_label: String,
    #[prop(into)] disabled: Signal<bool>,
    #[prop(into)] active: Signal<bool>,
    #[prop(into)] on_click: Callback<()>,
) -> impl IntoView {
    let class = move || {
        cn([
            "flex h-8 min-w-8 items-center justify-center rounded px-2 text-sm transition-colors",
            if active.get() {
                "bg-[var(--color-ink)] text-[var(--color-parchment)]"
            } else {
                "text-[var(--color-ink-muted)] hover:bg-[var(--color-vellum)] hover:text-[var(--color-ink)]"
            },
            if disabled.get() {
                "pointer-events-none opacity-30"
            } else {
                ""
            },
        ])
    };
    view! {
        <button
            type="button"
            on:click=move |_| on_click.run(())
            prop:disabled=move || disabled.get()
            aria-label=aria_label
            aria-current=move || active.get().then_some("page")
            class=class
        >
            {label}
        </button>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_total_lists_all() {
        assert_eq!(
            build_page_range(1, 5),
            vec![
                PageItem::Page(1),
                PageItem::Page(2),
                PageItem::Page(3),
                PageItem::Page(4),
                PageItem::Page(5),
            ]
        );
    }

    #[test]
    fn windowed_with_leading_ellipsis() {
        // current=8, total=10 ⇒ [1, …, 6,7,8,9,10] (end touche total-1).
        let r = build_page_range(8, 10);
        assert_eq!(r[0], PageItem::Page(1));
        assert_eq!(r[1], PageItem::Ellipsis);
        assert!(r.contains(&PageItem::Page(8)));
        assert_eq!(*r.last().unwrap(), PageItem::Page(10));
    }

    #[test]
    fn windowed_with_both_ellipses() {
        // current=5, total=10 ⇒ [1, …, 3,4,5,6,7, …, 10].
        let r = build_page_range(5, 10);
        assert_eq!(r[0], PageItem::Page(1));
        assert_eq!(r[1], PageItem::Ellipsis);
        assert_eq!(*r.last().unwrap(), PageItem::Page(10));
        let ellipses = r.iter().filter(|i| **i == PageItem::Ellipsis).count();
        assert_eq!(ellipses, 2);
    }
}
