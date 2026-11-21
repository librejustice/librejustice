//! The reader's jurisdiction in the top bar: a panel from `sm` up, a bottom sheet below.

use leptos::either::Either;
use leptos::html;
use leptos::prelude::*;
use lj_dtos::ReaderJurisdiction;

use crate::components::top_bar::close_on_outside_or_nav;
use crate::jurisdiction::use_reader_scope;

const FRAME: &str = "flex shrink-0 items-center gap-1 rounded-md border border-[var(--color-rule)] px-2.5 py-1.5 text-sm leading-4 font-medium lg:leading-5 text-[var(--color-ink-muted)] transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)]";
const CODE: &str = "inline-block w-[1.6em] text-center";
const FLAG: &str = "h-3 w-4 shrink-0 rounded-[2px] object-cover ring-1 ring-[var(--color-rule)]";

/// Off the scoped routes the code is the cookie's, read after hydration: until
/// then a blank chip of the same size holds its place.
#[component]
pub fn JurisdictionChip() -> impl IntoView {
    let scope = use_reader_scope();
    view! {
        <Show
            when=move || scope.settled()
            fallback=|| {
                view! {
                    <span aria-hidden="true" class=format!("invisible {FRAME}")>
                        <span class=FLAG></span>
                        <span class=CODE>"FR"</span>
                        <span class="text-xs">"▾"</span>
                    </span>
                }
            }
        >
            <Chip />
        </Show>
    }
}

#[component]
fn Chip() -> impl IntoView {
    let scope = use_reader_scope();
    let ui = &crate::i18n::use_ui().shell;
    let open = RwSignal::new(false);
    let opened = RwSignal::new(false);
    let container_ref = NodeRef::<html::Div>::new();
    let button_ref = NodeRef::<html::Button>::new();
    let sheet_ref = NodeRef::<html::Dialog>::new();
    close_on_outside_or_nav(open, container_ref);

    #[cfg(feature = "hydrate")]
    Effect::new(move |_| {
        let wide = window()
            .match_media("(min-width: 40rem)")
            .ok()
            .flatten()
            .is_some_and(|m| m.matches());
        let Some(sheet) = sheet_ref.get() else {
            return;
        };
        match (open.get() && !wide, sheet.open()) {
            (true, false) => {
                let _ = sheet.show_modal();
            }
            (false, true) => sheet.close(),
            _ => {}
        }
    });

    let toggle = move |_| {
        let next = !open.get_untracked();
        open.set(next);
        if next && !opened.get_untracked() {
            opened.set(true);
            #[cfg(feature = "hydrate")]
            leptos::task::spawn_local(async move {
                if let Ok(list) = scope.load_known().await {
                    scope.keep_known(list);
                }
            });
        }
    };
    let close_and_focus = move || {
        open.set(false);
        if let Some(button) = button_ref.get_untracked() {
            let _ = button.focus();
        }
    };
    let on_keydown = move |ev: leptos::ev::KeyboardEvent| {
        if ev.key() == "Escape" && open.get_untracked() {
            ev.prevent_default();
            close_and_focus();
        }
    };
    let on_sheet_click = move |ev: leptos::ev::MouseEvent| {
        #[cfg(feature = "hydrate")]
        {
            use wasm_bindgen::JsCast;
            let on_backdrop =
                sheet_ref
                    .get_untracked()
                    .zip(ev.target())
                    .is_some_and(|(sheet, target)| {
                        target.dyn_ref::<web_sys::Node>() == Some(sheet.unchecked_ref())
                    });
            if on_backdrop {
                close_and_focus();
            }
        }
        let _ = ev;
    };

    let navigate = StoredValue::new_local(leptos_router::hooks::use_navigate());
    let location = leptos_router::hooks::use_location();
    let query = leptos_router::hooks::use_query_map();
    let choose = Callback::new(move |code: String| {
        close_and_focus();
        #[cfg(feature = "hydrate")]
        {
            let current = untrack(|| scope.code());
            scope.choose(&code);
            if code == current {
                return;
            }
            let target = location.pathname.with_untracked(|path| {
                query.with_untracked(|q| crate::jurisdiction::rescoped(path, q, &code))
            });
            if let Some(target) = target {
                navigate.with_value(|n| n(&target, Default::default()));
            }
        }
        #[cfg(not(feature = "hydrate"))]
        let _ = (code, navigate, &location, query);
    });

    let choices = move || {
        opened.get().then(|| match scope.known() {
            Some(list) => Either::Left(
                list.into_iter()
                    .map(|j| view! { <Choice jurisdiction=j choose=choose /> })
                    .collect_view(),
            ),
            None => Either::Right(view! {
                <div class="flex flex-col gap-2 px-3 py-2">
                    <crate::components::ui::Skeleton class="h-4 w-3/4" />
                    <crate::components::ui::Skeleton class="h-4 w-1/2" />
                    <crate::components::ui::Skeleton class="h-4 w-2/3" />
                </div>
            }),
        })
    };
    view! {
        <div node_ref=container_ref class="relative shrink-0" on:keydown=on_keydown>
            <button
                type="button"
                node_ref=button_ref
                on:click=toggle
                aria-label=move || {
                    let name = scope.label().unwrap_or_else(|| scope.code().to_ascii_uppercase());
                    (ui.jurisdiction_of)(&name)
                }
                aria-haspopup="dialog"
                aria-expanded=move || if open.get() { "true" } else { "false" }
                class=FRAME
            >
                {move || view! { <Flag code=scope.code() /> }}
                <span class=CODE>{move || scope.code().to_ascii_uppercase()}</span>
                <span aria-hidden="true" class="text-xs">
                    "▾"
                </span>
            </button>
            <div
                role="dialog"
                aria-label=ui.jurisdiction
                data-open=move || open.get().then_some("true")
                class="lj-panel absolute right-0 top-full z-50 mt-2 hidden w-56 overflow-hidden rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] shadow-lg sm:block"
            >
                <div class="max-h-[min(60vh,24rem)] overflow-y-auto py-1">{choices}</div>
            </div>
            <dialog
                node_ref=sheet_ref
                aria-label=ui.jurisdiction
                on:click=on_sheet_click
                on:close=move |_| open.set(false)
                class="m-0 mt-auto w-full max-w-none rounded-t-lg border-t border-[var(--color-rule)] bg-[var(--color-parchment)] p-0 shadow-xl"
                style="overscroll-behavior: contain"
            >
                <div class="flex max-h-[70vh] flex-col pb-[env(safe-area-inset-bottom)]">
                    <div class="overflow-y-auto py-1">{choices}</div>
                </div>
            </dialog>
        </div>
    }
}

#[component]
fn Choice(jurisdiction: ReaderJurisdiction, choose: Callback<String>) -> impl IntoView {
    let scope = use_reader_scope();
    let code = jurisdiction.code;
    let current = {
        let code = code.clone();
        Memo::new(move |_| scope.code() == code)
    };
    let shown = code.to_ascii_uppercase();
    let flag = code.clone();
    view! {
        <button
            type="button"
            on:click=move |_| choose.run(code.clone())
            aria-current=move || current.get().then_some("true")
            class="flex w-full items-center gap-2 px-3 py-2 text-left text-sm text-[var(--color-ink-muted)] transition-colors hover:bg-[var(--color-rule)] hover:text-[var(--color-ink)] aria-[current=true]:text-[var(--color-ink)] sm:py-1.5"
        >
            <span aria-hidden="true" class="w-3 shrink-0 text-[var(--color-accent)]" class=("invisible", move || !current.get())>
                "✓"
            </span>
            <Flag code=flag />
            <span class="min-w-0 flex-1 truncate">{jurisdiction.label}</span>
            <span class="shrink-0 text-xs text-[var(--color-ink-subtle)]">{shown}</span>
        </button>
    }
}

#[component]
fn Flag(code: String) -> impl IntoView {
    view! { <img src=format!("/flags/{code}.svg") alt="" width="16" height="12" class=FLAG /> }
}
