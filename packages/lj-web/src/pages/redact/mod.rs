//! The anonymisation workbench. One interface for the three kinds of document
//! the tool takes, and nothing leaves the machine: the file is read, drawn,
//! searched and rewritten in the page that shows it.

mod bridge;
mod engine;
mod fault;
mod host;
mod lock;
pub mod mcp_app;
mod outlet;
mod sidebar;
mod state;
mod toolbar;
mod viewer;

use leptos::prelude::*;

pub use fault::Fault;
use outlet::{Outlet, Outlets};
use state::Offline;
pub use state::{Session, Status};
pub use toolbar::Toolbar;

const ACCEPT: &str = ".pdf,.docx,.odt,.doc,.htm,.html,.png,.jpg,.jpeg,.webp";
const CHOOSE: &str = "h-10 rounded-md bg-[var(--color-ink)] px-4 text-sm font-medium text-[var(--color-parchment)] transition-colors hover:bg-[var(--color-ink-muted)] disabled:opacity-60";
const SECONDARY: &str = "h-9 rounded-md border border-[var(--color-rule)] px-3 text-sm font-medium text-[var(--color-ink-muted)] transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)] disabled:pointer-events-none disabled:opacity-40";
const FRAME: &str = "flex min-h-0 w-full flex-1 flex-col overflow-hidden md:flex-row rounded-lg border border-[var(--color-rule)] bg-[var(--color-parchment)]";

/// The workbench as a page of the site, whose top bar carries its controls.
#[component]
pub fn SiteWorkbench() -> impl IntoView {
    let session = Session::new(&crate::i18n::use_ui().redact);
    let bar = expect_context::<crate::components::decision_bar::WorkbenchSignal>();
    bar.set(Some(session));
    on_cleanup(move || bar.set(None));
    provide_context(Outlets(vec![Outlet::device()]));
    #[cfg(feature = "hydrate")]
    {
        lock::lock(None, &["/internal/me", "/internal/auth/logout"]);
        on_cleanup(|| {
            let _ = web_sys::window().map(|window| window.location().reload());
        });
    }
    view! { <RedactTool session=session /> }
}

/// The workbench itself, wherever it is mounted: what is done with the file is
/// the [`Outlets`] in context.
#[component]
fn RedactTool(session: Session) -> impl IntoView {
    #[cfg(feature = "hydrate")]
    {
        engine::bind_keys(session);
        let texts = Memo::new(move |_| {
            session
                .needles
                .with(|list| list.iter().map(|n| n.text.clone()).collect::<Vec<_>>())
        });
        Effect::new(move |_| {
            let _ = session.generation.get();
            engine::refresh(session, texts.get());
        });
        Effect::new(move |_| {
            let _ = session.generation.get();
            engine::relabel(session);
        });
    }

    let on_change = move |event: leptos::ev::Event| {
        let _ = &event;
        #[cfg(feature = "hydrate")]
        {
            use wasm_bindgen::JsCast;
            if let Some(input) = event
                .target()
                .and_then(|target| target.dyn_into::<web_sys::HtmlInputElement>().ok())
            {
                if let Some(file) = input.files().and_then(|files| files.get(0)) {
                    engine::adopt(session, file);
                }
                input.set_value("");
            }
        }
    };
    let on_over = move |event: leptos::ev::DragEvent| event.prevent_default();
    let on_drop = move |event: leptos::ev::DragEvent| {
        event.prevent_default();
        #[cfg(feature = "hydrate")]
        if let Some(file) = event
            .data_transfer()
            .and_then(|transfer| transfer.files())
            .and_then(|files| files.get(0))
        {
            engine::adopt(session, file);
        }
    };

    view! {
        <div class="flex min-h-0 flex-1 flex-col gap-2">
            <input
                id="redact-file"
                type="file"
                accept=ACCEPT
                class="sr-only"
                on:change=on_change
            />
            <div class=FRAME on:dragover=on_over on:drop=on_drop>
                {move || match session.status.get() {
                    Status::Ready => view! { <Workbench session=session /> }.into_any(),
                    Status::Loading => view! { <Waiting /> }.into_any(),
                    Status::Failed(why) => view! { <Refused why=why /> }.into_any(),
                    Status::Empty => view! { <Dropzone session=session /> }.into_any(),
                }}
            </div>
        </div>
    }
}

#[component]
fn Padlock(class: &'static str) -> impl IntoView {
    view! {
        <svg
            viewBox="0 0 24 24"
            class=class
            fill="none"
            stroke="currentColor"
            stroke-width="1.75"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
        >
            <rect x="5" y="11" width="14" height="9" rx="2" />
            <path d="M8 11 V8 a4 4 0 0 1 8 0 v3" />
        </svg>
    }
}

#[component]
fn Workbench(session: Session) -> impl IntoView {
    let ui = &crate::i18n::use_ui().redact;
    view! {
        <sidebar::Sidebar session=session />
        <div class="relative flex min-h-0 min-w-0 flex-1 flex-col">
            <viewer::Viewer session=session />
            <p class="pointer-events-none absolute bottom-4 right-5 flex items-center gap-1.5 rounded-full bg-[var(--color-ink-muted)]/80 px-3 py-1.5 text-xs font-medium text-[var(--color-parchment)] shadow-sm backdrop-blur-sm">
                <Padlock class="h-3.5 w-3.5 shrink-0" />
                {ui.local_processing}
            </p>
        </div>
    }
}

#[component]
fn Waiting() -> impl IntoView {
    let ui = &crate::i18n::use_ui().redact;
    view! {
        <div class="flex w-full flex-1 items-center justify-center p-10">
            <p class="text-sm text-[var(--color-ink-muted)]">{ui.reading}</p>
        </div>
    }
}

#[component]
fn Refused(why: Fault) -> impl IntoView {
    let on_pick = move |_| {
        #[cfg(feature = "hydrate")]
        engine::pick();
    };
    let ui = &crate::i18n::use_ui().redact;
    view! {
        <div class="flex w-full flex-1 flex-col items-center justify-center gap-4 p-10 text-center">
            <p class="font-sans text-base text-[var(--color-ink)]">{ui.not_opened}</p>
            <p class="max-w-md text-sm leading-relaxed text-[var(--color-ink-muted)]">
                {(ui.fault)(&why)}
            </p>
            <button type="button" class=CHOOSE on:click=on_pick>
                {ui.choose_other}
            </button>
        </div>
    }
}

#[component]
fn Dropzone(session: Session) -> impl IntoView {
    let hydrated = RwSignal::new(false);
    Effect::new(move |_| hydrated.set(true));
    let on_pick = move |_| {
        #[cfg(feature = "hydrate")]
        engine::pick();
    };
    let ui = &crate::i18n::use_ui().redact;
    view! {
        <div class="flex w-full flex-1 flex-col items-center justify-center gap-8 overflow-auto p-6 text-center sm:gap-10 sm:p-10">
            <div class="flex max-w-xl flex-col items-center gap-3">
                <Padlock class="h-9 w-9 text-[var(--color-accent)]" />
                <p class="text-balance font-sans text-2xl font-medium tracking-[-0.015em] text-[var(--color-ink)] sm:text-3xl">
                    {ui.stays_local}
                </p>
                <p class="text-sm leading-relaxed text-[var(--color-ink-muted)] sm:text-base">
                    {ui.stays_local_body}
                </p>
            </div>
            <div class="flex flex-col items-center gap-4">
                <div class="flex flex-col gap-1">
                    <p class="font-sans text-base text-[var(--color-ink)]">{ui.drop_here}</p>
                    <p class="text-sm text-[var(--color-ink-muted)]">{ui.formats}</p>
                </div>
                <button
                    type="button"
                    class=CHOOSE
                    disabled=move || !hydrated.get()
                    on:click=on_pick
                >
                    {move || match hydrated.get() {
                        true => ui.choose,
                        false => ui.loading_tool,
                    }}
                </button>
            </div>
            <Prepare session=session hydrated=hydrated />
            <a
                href="https://github.com/librejustice/librejustice"
                class="text-xs text-[var(--color-ink-subtle)] underline underline-offset-2 hover:text-[var(--color-ink)]"
            >
                {ui.public_code}
            </a>
        </div>
    }
}

/// The offer to fetch the whole tool before any document, and where it stands.
#[component]
fn Prepare(session: Session, hydrated: RwSignal<bool>) -> impl IntoView {
    let on_prepare = move |_| {
        #[cfg(feature = "hydrate")]
        engine::prepare(session);
    };
    let ui = &crate::i18n::use_ui().redact;
    view! {
        <div class="flex max-w-sm flex-col items-center gap-2 text-balance text-xs leading-relaxed text-[var(--color-ink-muted)]">
            {move || match session.offline.get() {
                Offline::Online => {
                    view! {
                        <button
                            type="button"
                            class=SECONDARY
                            disabled=move || !hydrated.get()
                            on:click=on_prepare
                        >
                            {ui.prepare}
                        </button>
                        <p>{ui.prepare_body}</p>
                    }
                        .into_any()
                }
                Offline::Loading(at) => {
                    view! {
                        <div class="h-1.5 w-48 overflow-hidden rounded-full bg-[var(--color-rule)]">
                            <div
                                class="h-full rounded-full bg-[var(--color-accent)]"
                                style:width=format!("{:.0}%", at * 100.0)
                            />
                        </div>
                        <p>{(ui.downloading)(at)}</p>
                    }
                        .into_any()
                }
                Offline::Ready => {
                    view! {
                        <p class="flex items-center gap-1.5 text-sm font-medium text-[var(--color-ink)]">
                            <svg
                                viewBox="0 0 24 24"
                                class="h-4 w-4 shrink-0 text-[var(--color-accent)]"
                                fill="none"
                                stroke="currentColor"
                                stroke-width="2"
                                stroke-linecap="round"
                                stroke-linejoin="round"
                                aria-hidden="true"
                            >
                                <path d="M5 12.5 L10 17.5 L19 7" />
                            </svg>
                            {ui.ready_offline}
                        </p>
                        <p>{ui.ready_offline_body}</p>
                    }
                        .into_any()
                }
                Offline::Failed(why) => {
                    view! {
                        <p>{(ui.prepare_stopped)(&(ui.fault)(&why))}</p>
                        <button type="button" class=SECONDARY on:click=on_prepare>
                            {ui.retry}
                        </button>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}
