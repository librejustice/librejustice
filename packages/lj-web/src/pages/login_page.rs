//! Page `/login`.
//!
//! Connexion, inscription et mot de passe oublié en formulaires natifs, sans
//! JavaScript : chaque action POST `/internal/auth/*` et répond 303. Une erreur
//! attendue rebondit ici avec son code en query (`error=`), jamais l'adresse ;
//! un mail parti affiche l'écran `sent=`. Le mode (`mode=signup|reset`) et la
//! destination `next` vivent dans l'URL. Un porteur de session n'atteint pas la
//! page : la garde serveur (`page_gates`) répond 303 vers `next` avant le rendu.
//!
//! Héberge aussi les modules partagés de la tranche auth :
//! - `auth_errors` : traduction FR des erreurs d'auth — module pur, compilé des
//!   deux côtés : les pages auth traduisent au rendu SSR.
//! - `browser` : bindings DOM page-level (hydrate seulement).

#[path = "auth_errors.rs"]
pub(crate) mod auth_errors;
#[cfg(feature = "hydrate")]
#[path = "browser.rs"]
pub(crate) mod browser;

use leptos::prelude::*;
use leptos_meta::{Meta, Title};

use auth_errors::translate_auth_error;

use crate::components::prose;
const CHAMP: &str = "rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] \
     px-3 py-2 text-sm text-[var(--color-ink)] outline-none focus:border-[var(--color-accent)]";
const BOUTON: &str = "rounded-md bg-[var(--color-accent)] px-4 py-2.5 text-sm font-medium \
     text-white transition-opacity hover:opacity-90";

#[component]
pub fn LoginPage() -> impl IntoView {
    let query = leptos_router::hooks::use_query_map();
    view! {
        <Title text=crate::i18n::use_ui().account.login_title />
        <Meta name="robots" content="noindex" />
        {move || {
            let sent = query.with(|q| q.get("sent"));
            let mode = query.with(|q| q.get("mode").unwrap_or_default());
            let error = query.with(|q| q.get("error"));
            let next = query.with(|q| q.get("next").unwrap_or_default());
            match sent.as_deref() {
                Some(genre) => view! { <MailEnvoye genre=genre.to_string() /> }.into_any(),
                None if mode == "reset" => view! { <DemandeReset next error /> }.into_any(),
                None if mode == "link" => view! { <DemandeLien next error /> }.into_any(),
                None => view! { <Identifiants signup=mode == "signup" next error /> }.into_any(),
            }
        }}
    }
}

/// Formulaire de connexion ou d'inscription, selon `signup`. Le POST natif porte
/// `next` en champ caché ; l'erreur du rebond s'affiche traduite, et
/// `email_not_confirmed` déplie le formulaire de renvoi du mail.
#[component]
fn Identifiants(signup: bool, next: String, error: Option<String>) -> impl IntoView {
    let ui = &crate::i18n::use_ui().account;
    let avec_next = |base: &str| {
        if next.is_empty() {
            base.to_string()
        } else {
            let sep = if base.contains('?') { '&' } else { '?' };
            format!(
                "{base}{sep}next={}",
                leptos_router::location::Url::escape(&next)
            )
        }
    };
    let lien_reset = avec_next("/login?mode=reset");
    let lien_magique = avec_next("/login?mode=link");
    let lien_bascule = if signup {
        avec_next("/login")
    } else {
        avec_next("/login?mode=signup")
    };
    let renvoi = error.as_deref() == Some("email_not_confirmed");
    let message = error.map(|code| translate_auth_error(ui, Some(&code)));

    view! {
        <div class="mx-auto flex max-w-sm flex-col gap-8 px-4 py-16">
            <div class="flex flex-col gap-2">
                <h1 class="font-sans text-2xl text-[var(--color-ink)]">
                    {if signup { ui.create_account } else { ui.login }}
                </h1>
                <p class="text-sm text-[var(--color-ink-muted)]">{ui.login_intro}</p>
            </div>

            <div class="flex flex-col gap-2.5">
                <GoogleButton />
                <MicrosoftButton />
            </div>

            <div class="flex items-center gap-3">
                <div class="h-px flex-1 bg-[var(--color-rule)]"></div>
                <span class="text-xs text-[var(--color-ink-subtle)]">{ui.or}</span>
                <div class="h-px flex-1 bg-[var(--color-rule)]"></div>
            </div>

            <form
                method="post"
                action=if signup { "/internal/auth/signup" } else { "/internal/auth/login" }
                class="flex flex-col gap-4"
            >
                <input type="hidden" name="next" value=next.clone() />
                <div class="flex flex-col gap-1.5">
                    <label for="email" class="text-sm text-[var(--color-ink-muted)]">{ui.email}</label>
                    <input
                        id="email"
                        name="email"
                        type="email"
                        autocomplete="email"
                        required
                        class=CHAMP
                    />
                </div>

                <div class="flex flex-col gap-1.5">
                    <label for="password" class="text-sm text-[var(--color-ink-muted)]">
                        {ui.password}
                    </label>
                    <input
                        id="password"
                        name="password"
                        type="password"
                        autocomplete=if signup { "new-password" } else { "current-password" }
                        required
                        class=CHAMP
                    />
                    {signup
                        .then(|| {
                            view! {
                                <p class="text-xs text-[var(--color-ink-subtle)]">{ui.password_hint}</p>
                            }
                        })}
                    {(!signup)
                        .then(|| {
                            view! {
                                <a
                                    href=lien_reset
                                    class="self-end text-xs text-[var(--color-ink-muted)] hover:text-[var(--color-ink)]"
                                >
                                    {ui.forgot_password}
                                </a>
                            }
                        })}
                </div>

                {message
                    .map(|m| {
                        view! { <p class="text-sm text-red-600">{m}</p> }
                    })}

                <button type="submit" class=BOUTON>
                    {if signup { ui.create } else { ui.sign_in }}
                </button>
            </form>
            <TermsNotice />

            <div class="text-center text-sm text-[var(--color-ink-muted)]">
                <a href=lien_magique class="hover:text-[var(--color-ink)]">
                    {ui.email_link}
                </a>
            </div>

            {renvoi
                .then(|| {
                    view! {
                        <form method="post" action="/internal/auth/resend" class="flex flex-col gap-2">
                            <input type="hidden" name="next" value=next.clone() />
                            <label
                                for="resend-email"
                                class="text-sm text-[var(--color-ink-muted)]"
                            >
                                {ui.resend_label}
                            </label>
                            <input
                                id="resend-email"
                                name="email"
                                type="email"
                                autocomplete="email"
                                required
                                class=CHAMP
                            />
                            <button
                                type="submit"
                                class="self-start text-sm text-[var(--color-accent)] underline hover:opacity-80"
                            >
                                {ui.resend}
                            </button>
                        </form>
                    }
                })}

            <div class="text-center text-sm text-[var(--color-ink-muted)]">
                <a href=lien_bascule class="hover:text-[var(--color-ink)]">
                    {if signup { ui.to_sign_in } else { ui.to_sign_up }}
                </a>
            </div>
            <SignalerDepuisEcranBloquant de="/login" />
        </div>
    }
}

/// Every screen that can open an account, Google and Microsoft included, carries it.
#[component]
fn TermsNotice() -> impl IntoView {
    view! {
        <p class="text-center text-xs text-[var(--color-ink-subtle)]">
            {prose(
                crate::i18n::use_ui().account.terms_notice,
                "underline underline-offset-2 hover:text-[var(--color-ink)]",
            )}
        </p>
    }
}

/// Sortie de secours d'un écran où l'on peut rester coincé sans recours : la
/// connexion et le consentement MCP sont les deux culs-de-sac où un utilisateur
/// bloqué n'a rien d'autre à cliquer.
#[component]
pub fn SignalerDepuisEcranBloquant(de: &'static str) -> impl IntoView {
    view! {
        <div class="text-center text-xs text-[var(--color-ink-subtle)]">
            <a href=format!("/feedback?from={de}") class="hover:text-[var(--color-ink)]">
                {crate::i18n::use_ui().account.report_problem}
            </a>
        </div>
    }
}

/// Demande de lien de réinitialisation (`?mode=reset`) : l'adresse seule, la
/// réponse est identique que le compte existe ou non.
#[component]
fn DemandeReset(next: String, error: Option<String>) -> impl IntoView {
    let ui = &crate::i18n::use_ui().account;
    let retour = if next.is_empty() {
        "/login".to_string()
    } else {
        format!(
            "/login?next={}",
            leptos_router::location::Url::escape(&next)
        )
    };
    let message = error.map(|code| translate_auth_error(ui, Some(&code)));
    view! {
        <div class="mx-auto flex max-w-sm flex-col gap-8 px-4 py-16">
            <div class="flex flex-col gap-2">
                <h1 class="font-sans text-2xl text-[var(--color-ink)]">{ui.forgot_heading}</h1>
                <p class="text-sm text-[var(--color-ink-muted)]">{ui.forgot_intro}</p>
            </div>
            <form method="post" action="/internal/auth/reset-request" class="flex flex-col gap-4">
                <input type="hidden" name="next" value=next.clone() />
                <div class="flex flex-col gap-1.5">
                    <label for="email" class="text-sm text-[var(--color-ink-muted)]">{ui.email}</label>
                    <input
                        id="email"
                        name="email"
                        type="email"
                        autocomplete="email"
                        required
                        class=CHAMP
                    />
                </div>
                {message
                    .map(|m| {
                        view! { <p class="text-sm text-red-600">{m}</p> }
                    })}
                <button type="submit" class=BOUTON>{ui.send_link}</button>
            </form>
            <div class="text-center text-sm text-[var(--color-ink-muted)]">
                <a href=retour class="hover:text-[var(--color-ink)]">{ui.back_to_login}</a>
            </div>
        </div>
    }
}

/// Sign-in link request (`?mode=link`), the address alone. The link is worth a
/// password, so the answer never says whether the account existed.
#[component]
fn DemandeLien(next: String, error: Option<String>) -> impl IntoView {
    let ui = &crate::i18n::use_ui().account;
    let retour = if next.is_empty() {
        "/login".to_string()
    } else {
        format!(
            "/login?next={}",
            leptos_router::location::Url::escape(&next)
        )
    };
    let message = error.map(|code| translate_auth_error(ui, Some(&code)));
    view! {
        <div class="mx-auto flex max-w-sm flex-col gap-8 px-4 py-16">
            <div class="flex flex-col gap-2">
                <h1 class="font-sans text-2xl text-[var(--color-ink)]">{ui.link_heading}</h1>
                <p class="text-sm text-[var(--color-ink-muted)]">{ui.link_intro}</p>
            </div>
            <form method="post" action="/internal/auth/link-request" class="flex flex-col gap-4">
                <input type="hidden" name="next" value=next.clone() />
                <div class="flex flex-col gap-1.5">
                    <label for="email" class="text-sm text-[var(--color-ink-muted)]">{ui.email}</label>
                    <input
                        id="email"
                        name="email"
                        type="email"
                        autocomplete="email"
                        required
                        class=CHAMP
                    />
                </div>
                {message
                    .map(|m| {
                        view! { <p class="text-sm text-red-600">{m}</p> }
                    })}
                <button type="submit" class=BOUTON>{ui.send_link}</button>
            </form>
            <TermsNotice />
            <div class="text-center text-sm text-[var(--color-ink-muted)]">
                <a href=retour class="hover:text-[var(--color-ink)]">{ui.back_to_login}</a>
            </div>
        </div>
    }
}

/// Écran « Email envoyé » (`?sent=confirmation|recovery|connexion`).
#[component]
fn MailEnvoye(genre: String) -> impl IntoView {
    let ui = &crate::i18n::use_ui().account;
    view! {
        <div class="mx-auto flex max-w-sm flex-col gap-4 px-4 py-16">
            <h1 class="font-sans text-2xl text-[var(--color-ink)]">{ui.sent_heading}</h1>
            <p class="text-[var(--color-ink-muted)]">
                {match genre.as_str() {
                    "recovery" => ui.sent_recovery,
                    "connexion" => ui.sent_link,
                    _ => ui.sent_confirmation,
                }}
            </p>
        </div>
    }
}

const BOUTON_FOURNISSEUR: &str = "flex items-center justify-center gap-2.5 rounded-md border \
     border-[var(--color-rule)] bg-[var(--color-parchment)] px-4 py-2.5 text-sm font-medium \
     text-[var(--color-ink)] transition-colors hover:bg-[var(--color-vellum)]";

fn lien_start(fournisseur: &'static str) -> impl Fn() -> String {
    let query = leptos_router::hooks::use_query_map();
    move || {
        let next = query.with(|q| q.get("next").unwrap_or_default());
        format!(
            "/internal/auth/{fournisseur}/start?next={}",
            leptos_router::location::Url::escape(&next)
        )
    }
}

#[component]
fn GoogleButton() -> impl IntoView {
    view! {
        <a href=lien_start("google") rel="external nofollow" class=BOUTON_FOURNISSEUR>
            <svg width="18" height="18" viewBox="0 0 48 48" aria-hidden="true">
                <path
                    fill="#4285F4"
                    d="M45.1 24.5c0-1.6-.1-3.1-.4-4.5H24v8.5h11.8c-.5 2.7-2.1 5-4.4 6.5v5.4h7.1c4.2-3.8 6.6-9.5 6.6-15.9z"
                />
                <path
                    fill="#34A853"
                    d="M24 46c5.7 0 10.5-1.9 14-5.1l-7.1-5.4c-1.9 1.3-4.4 2.1-6.9 2.1-5.5 0-10.2-3.7-11.8-8.7H5v5.6C8.6 41.7 15.7 46 24 46z"
                />
                <path
                    fill="#FBBC05"
                    d="M12.2 28.9c-.4-1.2-.7-2.5-.7-3.9s.2-2.7.7-3.9v-5.6H5C3.6 18.3 2.8 21 2.8 25s.8 6.7 2.2 9.5l7.2-5.6z"
                />
                <path
                    fill="#EA4335"
                    d="M24 11.4c3.1 0 5.9 1.1 8.1 3.2l6.2-6.2C34.5 4.8 29.7 3 24 3 15.7 3 8.6 7.3 5 15.5l7.2 5.6c1.6-5 6.3-8.7 11.8-8.7z"
                />
            </svg>
            {crate::i18n::use_ui().account.continue_google}
        </a>
    }
}

#[component]
fn MicrosoftButton() -> impl IntoView {
    view! {
        <a href=lien_start("microsoft") rel="external nofollow" class=BOUTON_FOURNISSEUR>
            <svg width="18" height="18" viewBox="0 0 23 23" aria-hidden="true">
                <path fill="#F25022" d="M1 1h10v10H1z" />
                <path fill="#7FBA00" d="M12 1h10v10H12z" />
                <path fill="#00A4EF" d="M1 12h10v10H1z" />
                <path fill="#FFB900" d="M12 12h10v10H12z" />
            </svg>
            {crate::i18n::use_ui().account.continue_microsoft}
        </a>
    }
}
