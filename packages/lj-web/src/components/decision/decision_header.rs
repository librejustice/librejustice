//! En-tête décision + barre d'actions.
//!
//! `DecisionHeader` feeds the sticky bar (`components::decision_bar`) on the
//! client from the seed of the visit: search origin and prev/next. No bar on the
//! server.

use leptos::prelude::*;
use lj_dtos::{DecisionDetail, UiAction};

use crate::i18n::use_ui;
use crate::pages::decision_page::reference::build_decision_references;

#[cfg(feature = "hydrate")]
use crate::components::decision_bar::{use_decision_bar, visit_seed, DecisionBarState};

/// Pousse les métadonnées dans la barre sticky.
#[component]
pub fn DecisionHeader(detail: DecisionDetail) -> impl IntoView {
    #[cfg(feature = "hydrate")]
    {
        let title = build_decision_references(&detail).heading;
        let id = detail.id.clone();
        let path = lj_dtos::decision_path(
            detail.jurisdiction_type,
            detail.jurisdiction_code.as_deref(),
            &id,
        );
        let bar = use_decision_bar();
        let location = leptos_router::hooks::use_location();
        Effect::new(move |_| {
            let seed = visit_seed(&path, &location);
            bar.set(Some(DecisionBarState {
                title: title.clone(),
                path: path.clone(),
                nav: seed.nav,
                from_search: seed.from_search,
            }));
        });
        on_cleanup(move || bar.set(None));
    }
    #[cfg(not(feature = "hydrate"))]
    let _ = &detail;
}

/// Barre d'actions (référence, impression, PDF). Rendue dans
/// `DecisionSimilar`.
#[component]
pub fn DecisionActions(detail: DecisionDetail) -> impl IntoView {
    let references = build_decision_references(&detail);
    let ui = &use_ui().decision;
    let pdf_href = match crate::components::decision::decision_texts::chosen_text(&detail) {
        Some(lang) => format!("/internal/decision/{}/download.pdf?lang={lang}", detail.id),
        None => format!("/internal/decision/{}/download.pdf", detail.id),
    };

    let on_print = move |_| {
        #[cfg(feature = "hydrate")]
        {
            let _ = leptos::prelude::window().print();
        }
    };

    view! {
        <div class="flex flex-col gap-2">
            <div class="flex flex-nowrap items-center gap-2">
                <CopyReferenceButton full=references.full.clone() short=references.short.clone() />
                <button
                    type="button"
                    on:click=on_print
                    data-usage=UiAction::DecisionPrint.as_str()
                    class="inline-flex shrink-0 items-center gap-2 rounded-sm border border-[var(--color-rule)] px-2.5 py-1.5 text-xs text-[var(--color-ink-muted)] transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)]"
                    aria-label=ui.print
                    title=ui.print
                >
                    <PrintIcon />
                </button>
                // No `download`, so a navigation: the WAF challenges this route, and a challenge
                // only renders on one. The filename comes from the API's `Content-Disposition`.
                <a
                    href=pdf_href
                    data-usage=UiAction::DecisionPdf.as_str()
                    rel="external nofollow noopener"
                    target="_blank"
                    class="inline-flex shrink-0 items-center gap-2 rounded-sm border border-[var(--color-rule)] px-2.5 py-1.5 text-xs text-[var(--color-ink-muted)] no-underline transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)]"
                    aria-label=ui.pdf
                    title=ui.pdf
                >
                    <PdfIcon />
                </a>
            </div>
        </div>
    }
    .into_any()
}

#[component]
fn CopyReferenceButton(full: String, short: String) -> impl IntoView {
    let open = RwSignal::new(false);
    let ui = &use_ui().decision;
    // Quelle référence vient d'être copiée (`"full"`/`"short"`), `None` sinon.
    let copied = RwSignal::new(None::<&'static str>);
    let container_ref = NodeRef::<leptos::html::Div>::new();

    #[cfg(feature = "hydrate")]
    {
        use crate::components::ui::panel::{dismiss, Dismiss};
        dismiss(
            container_ref,
            move || open.get_untracked(),
            move || open.set(false),
            Dismiss::default(),
        );
    }

    let copy = move |value: String, kind: &'static str| {
        open.set(false);
        #[cfg(feature = "hydrate")]
        leptos::task::spawn_local(async move {
            if crate::dom::copy_text(&value).await {
                copied.set(Some(kind));
                leptos::prelude::set_timeout(
                    move || copied.set(None),
                    std::time::Duration::from_millis(1800),
                );
            }
        });
        #[cfg(not(feature = "hydrate"))]
        let _ = (value, kind);
    };

    let chevron_class = move || {
        format!(
            "h-3 w-3 transition-transform{}",
            if open.get() { " rotate-180" } else { "" }
        )
    };

    let full_copy = full.clone();
    let short_copy = short.clone();

    view! {
        <div node_ref=container_ref class="relative">
            <button
                type="button"
                on:click=move |_| open.update(|o| *o = !*o)
                aria-expanded=move || open.get().then_some("true")
                aria-haspopup="menu"
                aria-live="polite"
                class="inline-flex shrink-0 items-center gap-2 rounded-sm border border-[var(--color-rule)] px-3 py-1.5 text-xs text-[var(--color-ink-muted)] transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)]"
            >
                {move || {
                    if copied.get().is_some() {
                        view! {
                            <CheckIcon />
                            " "
                            {ui.copied}
                        }
                            .into_any()
                    } else {
                        view! {
                            {ui.reference}
                            " "
                            <svg aria-hidden="true" viewBox="0 0 12 12" class=chevron_class>
                                <path
                                    d="M2.25 4.5L6 8.25 9.75 4.5"
                                    fill="none"
                                    stroke="currentColor"
                                    stroke-width="1.4"
                                    stroke-linecap="round"
                                    stroke-linejoin="round"
                                />
                            </svg>
                        }
                            .into_any()
                    }
                }}
            </button>
            <div
                data-open=move || open.get().then_some("true")
                class="lj-panel absolute left-0 top-full z-30 mt-2 min-w-72 rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] p-2 shadow-lg"
            >
                <CopyMenuButton
                        label=ui.copy_full_reference
                        usage=UiAction::DecisionCopyFull
                        value=full.clone()
                        copied=Signal::derive(move || copied.get() == Some("full"))
                        on_copy=Callback::new({
                            let full_copy = full_copy.clone();
                            move |_| copy(full_copy.clone(), "full")
                        })
                    />
                    <CopyMenuButton
                        label=ui.copy_short_reference
                        usage=UiAction::DecisionCopyShort
                        value=short.clone()
                        copied=Signal::derive(move || copied.get() == Some("short"))
                        on_copy=Callback::new({
                            let short_copy = short_copy.clone();
                            move |_| copy(short_copy.clone(), "short")
                        })
                    />
            </div>
        </div>
    }
    .into_any()
}

#[component]
fn CopyMenuButton(
    label: &'static str,
    usage: UiAction,
    value: String,
    #[prop(into)] copied: Signal<bool>,
    on_copy: Callback<()>,
) -> impl IntoView {
    let copied_label = use_ui().decision.copied;
    view! {
        <button
            type="button"
            on:click=move |_| on_copy.run(())
            data-usage=usage.as_str()
            class="flex w-full flex-col items-start gap-1 rounded-sm px-3 py-2 text-left transition-colors hover:bg-[var(--color-vellum)]"
        >
            <span class="text-sm text-[var(--color-ink)]">
                {move || if copied.get() { copied_label } else { label }}
            </span>
            <span class="text-sm text-[var(--color-ink-subtle)]">{value}</span>
        </button>
    }
}

#[component]
fn CheckIcon() -> impl IntoView {
    view! {
        <svg aria-hidden="true" viewBox="0 0 16 16" class="h-3.5 w-3.5">
            <path
                d="M3 8.5l3.5 3.5L13 5"
                fill="none"
                stroke="currentColor"
                stroke-width="1.6"
                stroke-linecap="round"
                stroke-linejoin="round"
            />
        </svg>
    }
}

#[component]
fn PrintIcon() -> impl IntoView {
    view! {
        <svg aria-hidden="true" viewBox="0 0 16 16" class="h-3.5 w-3.5">
            <path
                d="M4 5V2.5h8V5M4 11h8v2.5H4zM3 6.5h10a1.5 1.5 0 011.5 1.5v2A1.5 1.5 0 0113 11.5H3A1.5 1.5 0 011.5 10v-2A1.5 1.5 0 013 6.5z"
                fill="none"
                stroke="currentColor"
                stroke-width="1.3"
                stroke-linejoin="round"
            />
            <circle cx="11.75" cy="8.75" r=".75" fill="currentColor" />
        </svg>
    }
}

#[component]
fn PdfIcon() -> impl IntoView {
    view! {
        <svg aria-hidden="true" viewBox="0 0 16 16" class="h-3.5 w-3.5">
            <path
                d="M4 1.75h5.5L13 5.25v8A1.75 1.75 0 0111.25 15h-7.5A1.75 1.75 0 012 13.25v-9.5A1.75 1.75 0 013.75 2z"
                fill="none"
                stroke="currentColor"
                stroke-width="1.3"
                stroke-linejoin="round"
            />
            <path d="M9.5 1.75v3.5H13" fill="none" stroke="currentColor" stroke-width="1.3" />
            <path
                d="M4.5 11h7"
                fill="none"
                stroke="currentColor"
                stroke-width="1.3"
                stroke-linecap="round"
            />
        </svg>
    }
}
