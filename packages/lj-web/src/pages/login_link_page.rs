//! Page `/login-link` : le lien du mail atterrit ici, la session s'ouvre au POST
//! du bouton, jamais au GET que les scanners de boîtes suivent.

use leptos::prelude::*;
use leptos_meta::{Meta, Title};
use leptos_router::components::A;

use super::login_page::auth_errors::translate_auth_error;

#[component]
pub fn LoginLinkPage() -> impl IntoView {
    let query = leptos_router::hooks::use_query_map();
    let token = move || query.with(|q| q.get("token").unwrap_or_default());
    let next = move || query.with(|q| q.get("next").unwrap_or_default());
    let error = move || query.with(|q| q.get("error"));
    let ui = &crate::i18n::use_ui().account;

    view! {
        <Title text=ui.login_title />
        <Meta name="robots" content="noindex" />
        <div class="mx-auto flex max-w-sm flex-col gap-4 px-4 py-16">
            <h1 class="font-sans text-2xl text-[var(--color-ink)]">{ui.login}</h1>
            <Show
                when=move || error().is_none() && !token().is_empty()
                fallback=move || {
                    view! {
                        <p class="text-sm text-red-600">
                            {move || match error() {
                                Some(code) => translate_auth_error(ui, Some(&code)),
                                None => ui.link_invalid.to_string(),
                            }}
                        </p>
                        <p class="text-sm text-[var(--color-ink-muted)]">
                            {ui.link_spent}
                            <A
                                href="/login?mode=link"
                                attr:class="text-[var(--color-accent)] underline hover:opacity-80"
                            >
                                {ui.new_link}
                            </A>
                        </p>
                    }
                }
            >
                <p class="text-sm text-[var(--color-ink-muted)]">
                    {ui.link_opens}
                </p>
                <form method="post" action="/internal/auth/link" class="flex flex-col gap-3">
                    <input type="hidden" name="token" value=token />
                    <input type="hidden" name="next" value=next />
                    <button
                        type="submit"
                        class="rounded-md bg-[var(--color-accent)] px-4 py-2.5 text-sm font-medium text-white transition-opacity hover:opacity-90"
                    >
                        {ui.sign_me_in}
                    </button>
                </form>
            </Show>
        </div>
    }
}
