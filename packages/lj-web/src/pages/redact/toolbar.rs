//! The reader controls: which page, how big, what the pointer does.

use leptos::prelude::*;

use super::state::{Mode, Session, View};

#[cfg(feature = "hydrate")]
use super::engine;

const GROUP: &str = "flex shrink-0 items-center gap-1 rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] p-0.5";
const ICON: &str = "inline-flex h-7 w-7 items-center justify-center rounded-sm text-[var(--color-ink)] transition-colors hover:bg-[var(--color-vellum)] disabled:pointer-events-none disabled:opacity-40";
const TAB: &str = "h-7 rounded-sm px-3 text-sm transition-colors";
const TAB_ON: &str = "bg-[var(--color-ink)] text-[var(--color-parchment)]";
const TAB_OFF: &str = "text-[var(--color-ink-muted)] hover:bg-[var(--color-vellum)]";
const BUTTON: &str = "flex shrink-0 items-center gap-2 whitespace-nowrap rounded-md border border-[var(--color-rule)] px-2.5 py-1.5 text-sm font-medium text-[var(--color-ink-muted)] transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)] disabled:pointer-events-none disabled:opacity-40";
const ITEM: &str = "flex w-full items-center justify-between px-3 py-1.5 text-left text-sm text-[var(--color-ink-muted)] transition-colors hover:bg-[var(--color-rule)] hover:text-[var(--color-ink)] disabled:pointer-events-none disabled:opacity-40";

#[component]
fn Chevron(#[prop(default = false)] back: bool) -> impl IntoView {
    let path = match back {
        true => "M15 5 L8 12 L15 19",
        false => "M9 5 L16 12 L9 19",
    };
    view! {
        <svg
            viewBox="0 0 24 24"
            class="h-4 w-4"
            fill="none"
            stroke="currentColor"
            stroke-width="1.75"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
        >
            <path d=path />
        </svg>
    }
}

#[component]
fn Magnifier(#[prop(default = false)] minus: bool) -> impl IntoView {
    let sign = match minus {
        true => "M8 11 L14 11",
        false => "M8 11 L14 11 M11 8 L11 14",
    };
    view! {
        <svg
            viewBox="0 0 24 24"
            class="h-4 w-4"
            fill="none"
            stroke="currentColor"
            stroke-width="1.75"
            stroke-linecap="round"
            aria-hidden="true"
        >
            <circle cx="11" cy="11" r="6" />
            <path d="M16 16 L20 20" />
            <path d=sign />
        </svg>
    }
}

/// The workbench controls, set in the site's top bar while a document is open.
/// The file name gives way first; page turning stays in sight at every width,
/// and the rest folds into one menu when the bar itself runs short.
#[component]
pub fn Toolbar(session: Session) -> impl IntoView {
    view! {
        <div class="@container min-w-0 flex-1">
            <div class="flex items-center gap-2">
                <p
                    class="min-w-0 flex-1 truncate font-sans text-sm text-[var(--color-ink)] @max-[20rem]:invisible"
                    title=move || session.name.get()
                >
                    {move || session.name.get()}
                </p>
                <Pages session=session />
                <div class="hidden items-center gap-2 @min-[50rem]:flex">
                    <Zoom session=session />
                    <Views session=session />
                    <DrawToggle session=session />
                    <OtherFile />
                </div>
                <Options session=session />
            </div>
        </div>
    }
}

fn go(session: Session, page: usize) {
    let _ = (session, page);
    #[cfg(feature = "hydrate")]
    engine::go_to(session, page);
}

fn pick() {
    #[cfg(feature = "hydrate")]
    engine::pick();
}

#[component]
fn Pages(session: Session) -> impl IntoView {
    let ui = &crate::i18n::use_ui().redact;
    let pages = move || session.pages.with(Vec::len);
    view! {
        <div class=GROUP>
            <button
                type="button"
                class=ICON
                aria-label=ui.previous_page
                disabled=move || session.page.get() == 0
                on:click=move |_| go(session, session.page.get_untracked().saturating_sub(1))
            >
                <Chevron back=true />
            </button>
            <span class="px-1 text-sm tabular-nums text-[var(--color-ink-muted)]">
                {move || format!("{} / {}", session.page.get() + 1, pages().max(1))}
            </span>
            <button
                type="button"
                class=ICON
                aria-label=ui.next_page
                disabled=move || { session.page.get() + 1 >= pages() }
                on:click=move |_| go(session, (session.page.get_untracked() + 1).min(pages() - 1))
            >
                <Chevron />
            </button>
        </div>
    }
}

#[component]
fn Zoom(session: Session) -> impl IntoView {
    let ui = &crate::i18n::use_ui().redact;
    let zoom_by = move |factor: f64| {
        session.fit.set(false);
        session
            .zoom
            .update(|zoom| *zoom = (*zoom * factor).clamp(0.1, 6.0));
    };
    let on_fit = move |_| {
        session.fit.set(true);
        #[cfg(feature = "hydrate")]
        engine::fit(session);
    };
    view! {
        <div class=GROUP>
            <button type="button" class=ICON aria-label=ui.zoom_out on:click=move |_| zoom_by(1.0 / 1.2)>
                <Magnifier minus=true />
            </button>
            <span class="w-14 text-center text-sm tabular-nums text-[var(--color-ink-muted)]">
                {move || (ui.percent)((session.zoom.get() * 100.0).round())}
            </span>
            <button type="button" class=ICON aria-label=ui.zoom_in on:click=move |_| zoom_by(1.2)>
                <Magnifier />
            </button>
            <button type="button" class=format!("{TAB} {TAB_OFF}") on:click=on_fit>
                {ui.fit}
            </button>
        </div>
    }
}

#[component]
fn Views(session: Session) -> impl IntoView {
    let ui = &crate::i18n::use_ui().redact;
    let tab = move |view: View| {
        let on = session.view.get() == view;
        format!("{TAB} {}", if on { TAB_ON } else { TAB_OFF })
    };
    view! {
        <Show when=move || { session.characters.get() > 0 }>
            <div class=GROUP>
                <button type="button" class=move || tab(View::Pages) on:click=move |_| session.view.set(View::Pages)>
                    {ui.original}
                </button>
                <button
                    type="button"
                    class=move || tab(View::Text)
                    on:click=move |_| {
                        session.mode.set(Mode::Read);
                        session.view.set(View::Text);
                    }
                >
                    {ui.text}
                </button>
            </div>
        </Show>
    }
}

#[component]
fn DrawToggle(session: Session) -> impl IntoView {
    let ui = &crate::i18n::use_ui().redact;
    view! {
        <Show when=move || session.format.get().has_pixels()>
            <button
                type="button"
                disabled=move || session.view.get() != View::Pages
                aria-pressed=move || (session.mode.get() == Mode::Draw).to_string()
                class=move || {
                    match session.mode.get() {
                        Mode::Draw => format!("{BUTTON} border-[var(--color-ink)] text-[var(--color-ink)]"),
                        Mode::Read => BUTTON.to_string(),
                    }
                }
                on:click=move |_| {
                    session
                        .mode
                        .update(|mode| {
                            *mode = match mode {
                                Mode::Draw => Mode::Read,
                                Mode::Read => Mode::Draw,
                            };
                        })
                }
            >
                {ui.mask_area}
            </button>
        </Show>
    }
}

#[component]
fn OtherFile() -> impl IntoView {
    let ui = &crate::i18n::use_ui().redact;
    view! {
        <button type="button" class=BUTTON on:click=move |_| pick()
        >
            {ui.other_file}
        </button>
    }
}

/// What the bar has no room for when it runs short: the zoom, the view,
/// drawing and another file, in one panel.
#[component]
fn Options(session: Session) -> impl IntoView {
    let ui = &crate::i18n::use_ui().redact;
    let open = RwSignal::new(false);
    let container = NodeRef::<leptos::html::Div>::new();
    #[cfg(feature = "hydrate")]
    crate::components::ui::panel::dismiss(
        container,
        move || open.get_untracked(),
        move || open.set(false),
        crate::components::ui::panel::Dismiss::default(),
    );
    let zoom_by = move |factor: f64| {
        session.fit.set(false);
        session
            .zoom
            .update(|zoom| *zoom = (*zoom * factor).clamp(0.1, 6.0));
    };
    let on_fit = move |_| {
        open.set(false);
        session.fit.set(true);
        #[cfg(feature = "hydrate")]
        engine::fit(session);
    };
    let view_item = move |view: View| {
        let on = session.view.get() == view;
        format!("{TAB} flex-1 {}", if on { TAB_ON } else { TAB_OFF })
    };
    view! {
        <div node_ref=container class="relative shrink-0 @min-[50rem]:hidden">
            <button
                type="button"
                class=BUTTON
                aria-expanded=move || open.get().then_some("true")
                on:click=move |_| open.update(|o| *o = !*o)
            >
                {ui.options}
                <svg
                    viewBox="0 0 24 24"
                    class="h-3.5 w-3.5 transition-transform"
                    class=("rotate-180", move || open.get())
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.75"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    aria-hidden="true"
                >
                    <path d="M6 9 L12 15 L18 9" />
                </svg>
            </button>
            <div
                data-open=move || open.get().then_some("true")
                class="lj-panel absolute right-0 top-full z-50 mt-2 w-60 overflow-hidden rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] shadow-lg"
            >
                <div class="flex items-center justify-between gap-2 px-3 py-2">
                    <span class="text-sm text-[var(--color-ink-muted)]">{ui.zoom}</span>
                    <div class="flex items-center">
                        <button type="button" class=ICON aria-label=ui.zoom_out on:click=move |_| zoom_by(1.0 / 1.2)>
                            <Magnifier minus=true />
                        </button>
                        <span class="w-12 text-center text-sm tabular-nums text-[var(--color-ink)]">
                            {move || (ui.percent)((session.zoom.get() * 100.0).round())}
                        </span>
                        <button type="button" class=ICON aria-label=ui.zoom_in on:click=move |_| zoom_by(1.2)>
                            <Magnifier />
                        </button>
                    </div>
                </div>
                <button type="button" class=ITEM on:click=on_fit>
                    {ui.fit_window}
                </button>
                <Show when=move || { session.characters.get() > 0 }>
                    <div class="border-t border-[var(--color-rule)] px-3 py-2">
                        <div class="flex rounded-md border border-[var(--color-rule)] p-0.5">
                            <button
                                type="button"
                                class=move || view_item(View::Pages)
                                on:click=move |_| session.view.set(View::Pages)
                            >
                                {ui.original}
                            </button>
                            <button
                                type="button"
                                class=move || view_item(View::Text)
                                on:click=move |_| {
                                    session.mode.set(Mode::Read);
                                    session.view.set(View::Text);
                                }
                            >
                                {ui.text}
                            </button>
                        </div>
                    </div>
                </Show>
                <div class="border-t border-[var(--color-rule)] py-1">
                    <Show when=move || session.format.get().has_pixels()>
                        <button
                            type="button"
                            class=ITEM
                            disabled=move || session.view.get() != View::Pages
                            aria-pressed=move || (session.mode.get() == Mode::Draw).to_string()
                            on:click=move |_| {
                                open.set(false);
                                session
                                    .mode
                                    .update(|mode| {
                                        *mode = match mode {
                                            Mode::Draw => Mode::Read,
                                            Mode::Read => Mode::Draw,
                                        };
                                    });
                            }
                        >
                            {ui.mask_area}
                            <Show when=move || session.mode.get() == Mode::Draw>
                                <svg
                                    viewBox="0 0 24 24"
                                    class="h-4 w-4 text-[var(--color-ink)]"
                                    fill="none"
                                    stroke="currentColor"
                                    stroke-width="2"
                                    stroke-linecap="round"
                                    stroke-linejoin="round"
                                    aria-hidden="true"
                                >
                                    <path d="M5 12 L10 17 L19 7" />
                                </svg>
                            </Show>
                        </button>
                    </Show>
                    <button
                        type="button"
                        class=ITEM
                        on:click=move |_| {
                            open.set(false);
                            pick();
                        }
                    >
                        {ui.other_file}
                    </button>
                </div>
            </div>
        </div>
    }
}
