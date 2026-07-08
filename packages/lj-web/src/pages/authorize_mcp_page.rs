//! Page `/authorize-mcp` — port de `authorize-mcp-page.tsx`.
//!
//! Callback OAuth client-only : ecran de consentement qui echange une session
//! Supabase contre un code d'autorisation aupres de `/oauth/approve`, puis
//! redirige vers le `redirect_uri` du client MCP. N'utilise PAS le cache de
//! requetes : un `fetch` direct via le shim `browser` (web-sys indisponible).

use leptos::prelude::*;
use leptos_meta::{Meta, Title};

#[component]
pub fn AuthorizeMcpPage() -> impl IntoView {
    let has_session = RwSignal::new(false);
    let loading = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);

    // Parametres OAuth (defauts `""`, comme `params.get(...) ?? ""`). `client_id`
    // est lu dans la vue (affichage du consentement) ; les trois autres ne sont
    // lus/ecrits que dans les chemins hydrate (Effect + on_approve/on_deny).
    let client_id = RwSignal::new(String::new());
    #[cfg(feature = "hydrate")]
    let redirect_uri = RwSignal::new(String::new());
    #[cfg(feature = "hydrate")]
    let code_challenge = RwSignal::new(String::new());
    #[cfg(feature = "hydrate")]
    let state = RwSignal::new(String::new());

    #[cfg(feature = "hydrate")]
    {
        use super::login_page::browser;
        let query = leptos_router::hooks::use_query_map();
        let navigate = leptos_router::hooks::use_navigate();
        Effect::new(move |_| {
            query.with(|q| {
                client_id.set(q.get("client_id").unwrap_or_default());
                redirect_uri.set(q.get("redirect_uri").unwrap_or_default());
                code_challenge.set(q.get("code_challenge").unwrap_or_default());
                state.set(q.get("state").unwrap_or_default());
            });
            // `self_url` capture au montage (avant tout await) : chemin + query
            // courant, pour repointer vers `/connexion?next=` sans le perdre.
            let self_url = {
                let s = browser::location_path_search();
                if s.is_empty() {
                    "/authorize-mcp".to_string()
                } else {
                    s
                }
            };
            let navigate = navigate.clone();
            leptos::task::spawn_local(async move {
                if browser::has_session().await {
                    has_session.set(true);
                } else {
                    let next = String::from(js_sys::encode_uri_component(&self_url));
                    navigate(&format!("/connexion?next={next}"), Default::default());
                }
            });
        });
    }

    let on_approve = move |_| {
        #[cfg(feature = "hydrate")]
        {
            use super::login_page::browser;
            loading.set(true);
            error.set(None);
            let (cid, ruri, cc, st) = (
                client_id.get_untracked(),
                redirect_uri.get_untracked(),
                code_challenge.get_untracked(),
                state.get_untracked(),
            );
            leptos::task::spawn_local(async move {
                let token = match browser::session_token().await {
                    Some(t) => t,
                    None => {
                        loading.set(false);
                        return;
                    }
                };
                let body = serde_json::json!({
                    "client_id": cid,
                    "code_challenge": cc,
                    "redirect_uri": ruri,
                    "state": if st.is_empty() {
                        serde_json::Value::Null
                    } else {
                        serde_json::Value::String(st.clone())
                    },
                })
                .to_string();
                match browser::oauth_approve(&root_base_url(), &token, &body).await {
                    Ok(code) => {
                        let mut pairs: Vec<(&str, &str)> = vec![("code", code.as_str())];
                        if !st.is_empty() {
                            pairs.push(("state", st.as_str()));
                        }
                        let pairs_json = serde_json::to_string(&pairs).unwrap_or_default();
                        let dest = browser::build_url_with_params(&ruri, &pairs_json);
                        browser::navigate_hard(&dest);
                    }
                    Err(msg) => {
                        error.set(Some(msg));
                        loading.set(false);
                    }
                }
            });
        }
    };

    let on_deny = move |_| {
        #[cfg(feature = "hydrate")]
        {
            use super::login_page::browser;
            let (ruri, st) = (redirect_uri.get_untracked(), state.get_untracked());
            let mut pairs: Vec<(&str, &str)> = vec![("error", "access_denied")];
            if !st.is_empty() {
                pairs.push(("state", st.as_str()));
            }
            let pairs_json = serde_json::to_string(&pairs).unwrap_or_default();
            let dest = browser::build_url_with_params(&ruri, &pairs_json);
            browser::navigate_hard(&dest);
        }
    };

    view! {
        <Title text="Autoriser l'accès MCP — LibreJustice" />
        <Meta name="robots" content="noindex" />
        <Show
            when=move || has_session.get()
            fallback=|| {
                view! {
                    <div
                        aria-hidden="true"
                        class="mx-auto flex max-w-sm flex-col gap-8 px-4 py-16"
                    >
                        <div class="flex flex-col gap-2">
                            <div class="h-7 w-2/3 animate-pulse rounded-sm bg-[var(--color-vellum)]"></div>
                            <div class="h-4 w-full animate-pulse rounded-sm bg-[var(--color-vellum)]"></div>
                        </div>
                        <div class="h-24 w-full animate-pulse rounded-md bg-[var(--color-vellum)]"></div>
                        <div class="flex flex-col gap-3">
                            <div class="h-11 w-full animate-pulse rounded-md bg-[var(--color-vellum)]"></div>
                            <div class="h-11 w-full animate-pulse rounded-md bg-[var(--color-vellum)]"></div>
                        </div>
                    </div>
                }
            }
        >
            <div class="mx-auto flex max-w-sm flex-col gap-8 px-4 py-16">
                <div class="flex flex-col gap-2">
                    <h1 class="font-sans text-2xl text-[var(--color-ink)]">"Autoriser l'accès MCP"</h1>
                    <p class="text-sm text-[var(--color-ink-muted)]">
                        "L'application "
                        <strong class="text-[var(--color-ink)]">
                            {move || {
                                let c = client_id.get();
                                if c.is_empty() { "externe".to_string() } else { c }
                            }}
                        </strong> " demande l'accès en lecture à LibreJustice en votre nom."
                    </p>
                </div>

                <div class="rounded-md border border-[var(--color-rule)] p-4 text-sm text-[var(--color-ink-muted)]">
                    <p class="font-medium text-[var(--color-ink)]">"Accès accordé :"</p>
                    <ul class="mt-2 list-inside list-disc">
                        <li>"Recherche dans les décisions de justice"</li>
                        <li>"Lecture du texte intégral des décisions"</li>
                    </ul>
                </div>

                <Show when=move || error.get().is_some()>
                    <p class="text-sm text-red-600">{move || error.get().unwrap_or_default()}</p>
                </Show>

                <div class="flex flex-col gap-3">
                    <button
                        type="button"
                        on:click=on_approve
                        disabled=move || loading.get()
                        class="rounded-md bg-[var(--color-accent)] px-4 py-2.5 text-sm font-medium text-white transition-opacity hover:opacity-90 disabled:opacity-50"
                    >
                        {move || if loading.get() { "…" } else { "Autoriser" }}
                    </button>
                    <button
                        type="button"
                        on:click=on_deny
                        disabled=move || loading.get()
                        class="rounded-md border border-[var(--color-rule)] px-4 py-2.5 text-sm text-[var(--color-ink-muted)] transition-colors hover:text-[var(--color-ink)]"
                    >
                        "Refuser"
                    </button>
                </div>
            </div>
        </Show>
    }
}

/// Base des endpoints montes a la racine (`/oauth/*`). Port de `rootBaseUrl()`.
/// Cote client = origine courante (chaine vide -> chemins relatifs same-origin).
/// La page ne fetch QUE cote client (consentement OAuth), donc seule la branche
/// `hydrate` existe — pas de branche SSR morte (regle « pas de code defensif »).
#[cfg(feature = "hydrate")]
fn root_base_url() -> String {
    String::new()
}
