//! Page `/email-change` : les deux jetons du changement d'adresse atterrissent
//! ici — celui de l'adresse actuelle d'abord, puis celui de la
//! nouvelle. La consommation exige le POST du bouton, jamais le GET du lien —
//! que les scanners de boîtes suivent. Sans JavaScript : formulaire →
//! `/internal/auth/email-change` → 303 ; le serveur reconnaît l'étape au jeton.

use leptos::prelude::*;
use leptos_meta::{Meta, Title};

use super::login_page::auth_errors::translate_auth_error;

#[component]
pub fn EmailChangePage() -> impl IntoView {
    let query = leptos_router::hooks::use_query_map();
    let token = move || query.with(|q| q.get("token").unwrap_or_default());
    let sent = move || query.with(|q| q.get("sent").is_some());
    let error = move || query.with(|q| q.get("error"));
    let ui = &crate::i18n::use_ui().account;

    view! {
        <Title text=ui.email_change_title />
        <Meta name="robots" content="noindex" />
        <div class="mx-auto flex max-w-sm flex-col gap-4 px-4 py-16">
            <h1 class="font-sans text-2xl text-[var(--color-ink)]">{ui.email_change_heading}</h1>
            <Show
                when=move || !sent() && error().is_none() && !token().is_empty()
                fallback=move || {
                    view! {
                        <Show
                            when=sent
                            fallback=move || {
                                view! {
                                    <p class="text-sm text-red-600">
                                        {move || match error() {
                                            Some(code) => translate_auth_error(ui, Some(&code)),
                                            None => ui.link_incomplete.to_string(),
                                        }}
                                    </p>
                                    <p class="text-sm text-[var(--color-ink-muted)]">
                                        <a
                                            href="/profile"
                                            class="text-[var(--color-accent)] underline hover:opacity-80"
                                        >
                                            {ui.back_to_profile}
                                        </a>
                                    </p>
                                }
                            }
                        >
                            <p class="text-sm text-[var(--color-ink-muted)]">
                                {ui.email_change_sent}
                            </p>
                        </Show>
                    }
                }
            >
                <p class="text-sm text-[var(--color-ink-muted)]">
                    {ui.email_change_intro}
                </p>
                <form method="post" action="/internal/auth/email-change" class="flex flex-col gap-3">
                    <input type="hidden" name="token" value=token />
                    <button
                        type="submit"
                        class="rounded-md bg-[var(--color-accent)] px-4 py-2.5 text-sm font-medium text-white transition-opacity hover:opacity-90"
                    >
                        {ui.confirm}
                    </button>
                </form>
            </Show>
        </div>
    }
}
