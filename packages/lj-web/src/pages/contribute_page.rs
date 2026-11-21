//! `/decision/contribute`: a native multipart form to `/internal/contribution`, which
//! bounces back here in 303, `sent=1` or `error=<code>`.

use leptos::prelude::*;
use leptos_meta::{Meta, Title};

use crate::auth::AuthGuard;
use crate::components::prose;

#[component]
pub fn ContributePage() -> impl IntoView {
    let query = leptos_router::hooks::use_query_map();
    let sent = move || query.with(|q| q.get("sent").is_some());
    let ui = &crate::i18n::use_ui().tools;
    let error = move || {
        query
            .with(|q| q.get("error"))
            .map(|code| (ui.contribute_error)(&code))
    };
    let ticked = RwSignal::new(false);

    view! {
        <Title text=ui.contribute_title />
        <Meta name="robots" content="noindex" />
        <div class="mx-auto flex w-full max-w-lg flex-1 flex-col px-4 py-16 sm:px-6">
            <h1 class="font-sans text-2xl text-[var(--color-ink)]">{ui.contribute.title}</h1>
            <p class="mt-3 text-sm text-[var(--color-ink-muted)]">
                {prose(ui.contribute_intro, "underline underline-offset-2 hover:text-[var(--color-ink)]")}
            </p>
            <AuthGuard>
                <p class="mt-4 min-h-5 text-sm text-[var(--color-ink-muted)]">
                    {move || sent().then_some(ui.contribute_sent)}
                </p>
                <p class="min-h-5 text-sm text-red-600">{error}</p>
                <form
                    method="post"
                    action="/internal/contribution"
                    enctype="multipart/form-data"
                    class="mt-2 flex flex-col gap-5"
                >
                    <label class="flex cursor-pointer flex-col items-center gap-3 rounded-lg border border-dashed border-[var(--color-rule)] px-4 py-10 text-center">
                        <span class="font-sans text-base text-[var(--color-ink)]">
                            {ui.contribute_drop}
                        </span>
                        <input
                            type="file"
                            name="fichier"
                            required
                            accept=".pdf,.docx,application/pdf,application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                            class="text-sm text-[var(--color-ink-muted)]"
                        />
                    </label>
                    <label class="flex items-start gap-2 text-sm text-[var(--color-ink)]">
                        <input
                            type="checkbox"
                            name="atteste"
                            class="mt-0.5"
                            on:change=move |event| ticked.set(event_target_checked(&event))
                        />
                        {ui.contribute_attest}
                    </label>
                    <button
                        type="submit"
                        disabled=move || !ticked.get()
                        class="self-center rounded-md bg-[var(--color-accent)] px-4 py-2.5 text-sm font-medium text-white transition-opacity hover:opacity-90 disabled:pointer-events-none disabled:opacity-40"
                    >
                        {ui.send}
                    </button>
                </form>
            </AuthGuard>
        </div>
    }
}
