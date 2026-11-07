//! Action de l'article, en tête du rail gauche : copier sa référence humaine.

use leptos::prelude::*;
use lj_dtos::UiAction;

#[component]
pub(crate) fn ArticleActions(reference: String) -> impl IntoView {
    let copied = RwSignal::new(false);
    #[cfg(feature = "hydrate")]
    let reference = StoredValue::new(reference);
    #[cfg(not(feature = "hydrate"))]
    let _ = reference;

    let copy = move |_| {
        #[cfg(feature = "hydrate")]
        leptos::task::spawn_local(async move {
            if crate::dom::copy_text(&reference.get_value()).await {
                copied.set(true);
                leptos::prelude::set_timeout(
                    move || copied.set(false),
                    std::time::Duration::from_millis(1800),
                );
            }
        });
    };

    view! {
        <div class="flex flex-nowrap items-center gap-2">
            <button
                type="button"
                on:click=copy
                aria-live="polite"
                aria-label="Copier la référence"
                title="Copier la référence"
                data-usage=UiAction::NormCopyReference.as_str()
                class="inline-flex shrink-0 items-center gap-2 rounded-sm border border-[var(--color-rule)] px-3 py-1.5 text-xs text-[var(--color-ink-muted)] transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)]"
            >
                {move || {
                    if copied.get() {
                        view! {
                            <CheckIcon />
                            " Copié"
                        }
                            .into_any()
                    } else {
                        view! {
                            "Référence "
                            <CopyIcon />
                        }
                            .into_any()
                    }
                }}
            </button>
        </div>
    }
}

#[component]
fn CopyIcon() -> impl IntoView {
    view! {
        <svg aria-hidden="true" viewBox="0 0 16 16" class="h-3.5 w-3.5">
            <rect
                x="6"
                y="6"
                width="8"
                height="8"
                rx="1.5"
                fill="none"
                stroke="currentColor"
                stroke-width="1.3"
            />
            <path
                d="M10 4v-.5A1.5 1.5 0 008.5 2h-5A1.5 1.5 0 002 3.5v5A1.5 1.5 0 003.5 10H4"
                fill="none"
                stroke="currentColor"
                stroke-width="1.3"
                stroke-linecap="round"
                stroke-linejoin="round"
            />
        </svg>
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
