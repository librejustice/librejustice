//! Keys of the public API, on the account screen. An island rather than a
//! native form: the secret is shown once, in the response that creates it, and
//! a form would carry it in a redirect URL.

use leptos::prelude::*;
use lj_dtos::ApiKeySummary;

const CHAMP: &str =
    "flex-1 rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] \
     px-3 py-2 text-sm text-[var(--color-ink)] outline-none focus:border-[var(--color-accent)]";
const BOUTON: &str = "rounded-md bg-[var(--color-accent)] px-4 py-2 text-sm font-medium \
     text-white transition-opacity hover:opacity-90 disabled:opacity-50";
const CARTE: &str =
    "flex items-center justify-between rounded-md border border-[var(--color-rule)] px-4 py-3";
const ACTION_RETRAIT: &str =
    "text-sm text-[var(--color-ink-muted)] underline hover:text-[var(--color-ink)]";

/// Fallback label: the API rejects an empty one.
#[cfg(feature = "hydrate")]
fn default_label(ui: &crate::i18n::ProfileUi) -> String {
    let d = js_sys::Date::new_0();
    (ui.default_key_label)(d.get_date(), d.get_month() + 1, d.get_full_year())
}

#[component]
pub fn ApiKeysSettings() -> impl IntoView {
    let ui = &crate::i18n::use_ui().profile;
    #[cfg(feature = "hydrate")]
    let shell = &crate::i18n::use_ui().shell;
    let cles = RwSignal::new(Vec::<ApiKeySummary>::new());
    // The id of the key the secret opens, so its revocation takes the secret off the screen.
    let secret = RwSignal::new(None::<(String, String)>);
    let erreur = RwSignal::new(None::<String>);
    let libelle = RwSignal::new(String::new());
    let en_cours = RwSignal::new(false);

    Effect::new(move |_| {
        #[cfg(feature = "hydrate")]
        leptos::task::spawn_local(async move {
            match crate::api::client::ApiClient::from_context()
                .list_api_keys()
                .await
            {
                Ok(resp) => cles.set(resp.items),
                Err(e) => erreur.set(Some(e.failure.text(shell))),
            }
        });
    });

    let creer = move |_| {
        if en_cours.get() {
            return;
        }
        en_cours.set(true);
        erreur.set(None);
        #[cfg(feature = "hydrate")]
        {
            let saisi = libelle.get().trim().to_string();
            let nom = if saisi.is_empty() {
                default_label(ui)
            } else {
                saisi
            };
            leptos::task::spawn_local(async move {
                match crate::api::client::ApiClient::from_context()
                    .create_api_key(&nom)
                    .await
                {
                    Ok(cree) => {
                        secret.set(Some((cree.key.id.clone(), cree.secret)));
                        cles.update(|c| c.insert(0, cree.key));
                        libelle.set(String::new());
                    }
                    Err(e) => erreur.set(Some(e.failure.text(shell))),
                }
                en_cours.set(false);
            });
        }
        #[cfg(not(feature = "hydrate"))]
        en_cours.set(false);
    };

    let revoquer = move |id: String| {
        #[cfg(feature = "hydrate")]
        leptos::task::spawn_local(async move {
            match crate::api::client::ApiClient::from_context()
                .revoke_api_key(&id)
                .await
            {
                Ok(()) => {
                    if secret.with_untracked(|s| s.as_ref().is_some_and(|(key, _)| *key == id)) {
                        secret.set(None);
                    }
                    cles.update(|c| c.retain(|k| k.id != id));
                }
                Err(e) => erreur.set(Some(e.failure.text(shell))),
            }
        });
        #[cfg(not(feature = "hydrate"))]
        let _ = id;
    };

    view! {
        <div class="flex flex-col gap-3">
            <p class="text-sm text-[var(--color-ink-muted)]">
                {ui.keys_intro}
                <a
                    href=crate::jurisdiction::use_reader_scope().href("/api-guide")
                    class="ml-1 text-[var(--color-accent)] underline hover:opacity-80"
                >
                    {ui.keys_docs}
                </a>
            </p>

            <div class="flex gap-2">
                <input
                    type="text"
                    class=CHAMP
                    placeholder=ui.key_name_placeholder
                    maxlength="64"
                    prop:value=move || libelle.get()
                    on:input:target=move |ev| libelle.set(ev.target().value())
                />
                <button type="button" class=BOUTON on:click=creer disabled=move || en_cours.get()>
                    {ui.create_key}
                </button>
            </div>

            {move || {
                secret
                    .get()
                    .map(|(_, s)| {
                        view! {
                            <div class="flex flex-col gap-2 rounded-md border border-[var(--color-accent)] bg-[var(--color-vellum)] p-4">
                                <p class="text-sm font-medium text-[var(--color-ink)]">
                                    {ui.copy_key_now}
                                </p>
                                <code class="break-all rounded-md bg-[var(--color-parchment)] px-3 py-2 font-mono text-sm text-[var(--color-ink)]">
                                    {s}
                                </code>
                            </div>
                        }
                    })
            }}

            {move || {
                erreur
                    .get()
                    .map(|e| {
                        view! {
                            <p class="text-sm text-red-700 dark:text-red-400">
                                {ui.key_failed} {e}
                            </p>
                        }
                    })
            }}

            <For each=move || cles.get() key=|k| k.id.clone() let:cle>
                {
                    let id = cle.id.clone();
                    view! {
                        <div class=CARTE>
                            <div class="flex flex-col gap-0.5">
                                <p class="text-sm text-[var(--color-ink)]">{cle.label.clone()}</p>
                                <p class="font-mono text-xs text-[var(--color-ink-subtle)]">
                                    {format!("{}…", cle.prefix)}
                                </p>
                            </div>
                            <button
                                type="button"
                                class=ACTION_RETRAIT
                                on:click=move |_| revoquer(id.clone())
                            >
                                {ui.revoke}
                            </button>
                        </div>
                    }
                }
            </For>
        </div>
    }
}
