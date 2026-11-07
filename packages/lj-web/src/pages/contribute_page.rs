//! `/decision/contribute`: a native multipart form to `/internal/contribution`, which
//! bounces back here in 303, `sent=1` or `error=<code>`.

use leptos::prelude::*;
use leptos_meta::{Meta, Title};

use crate::auth::AuthGuard;

#[component]
pub fn ContributePage() -> impl IntoView {
    let query = leptos_router::hooks::use_query_map();
    let sent = move || query.with(|q| q.get("sent").is_some());
    let error = move || query.with(|q| q.get("error")).map(|code| said(&code));
    let ticked = RwSignal::new(false);

    view! {
        <Title text="Verser une décision - LibreJustice" />
        <Meta name="robots" content="noindex" />
        <div class="mx-auto flex w-full max-w-lg flex-1 flex-col px-4 py-16 sm:px-6">
            <h1 class="font-sans text-2xl text-[var(--color-ink)]">"Verser une décision"</h1>
            <p class="mt-3 text-sm text-[var(--color-ink-muted)]">
                "Passez-la d'abord dans l'"
                <a class="underline underline-offset-2 hover:text-[var(--color-ink)]" href="/redact">
                    "atelier d'anonymisation"
                </a>
                "."
            </p>
            <AuthGuard>
                <p class="mt-4 min-h-5 text-sm text-[var(--color-ink-muted)]">
                    {move || sent().then_some("Merci, la décision est envoyée.")}
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
                            "Déposez la décision ici"
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
                        "Je l'ai anonymisée et j'ai le droit de la partager."
                    </label>
                    <button
                        type="submit"
                        disabled=move || !ticked.get()
                        class="self-center rounded-md bg-[var(--color-accent)] px-4 py-2.5 text-sm font-medium text-white transition-opacity hover:opacity-90 disabled:pointer-events-none disabled:opacity-40"
                    >
                        "Envoyer"
                    </button>
                </form>
            </AuthGuard>
        </div>
    }
}

fn said(code: &str) -> &'static str {
    match code {
        "format" => "Seuls les fichiers PDF et Word sont acceptés.",
        "trop_lourd" => "Ce fichier dépasse 5 Mo.",
        "trop" => "Trop d'envois coup sur coup. Réessayez dans une minute.",
        "case" => "Cochez la case pour envoyer la décision.",
        _ => "L'envoi n'a pas abouti. Réessayez.",
    }
}
