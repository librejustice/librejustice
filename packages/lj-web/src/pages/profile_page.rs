//! Page `/profile`.
//!
//! Rendue serveur derrière la garde de session (`page_gates`) : le résumé du
//! compte se lit au rendu (resource bloquante, sérialisée pour l'hydratation),
//! chaque action est un formulaire natif → 303 avec l'issue en query (`done=`,
//! `sent=`, `error=`). Les moyens d'accès — identités liées, mot de passe — se
//! gèrent entre l'apparence et la zone danger. L'apparence reste un
//! îlot client : le thème vit dans `localStorage`.

use leptos::prelude::*;
use leptos_meta::{Meta, Title};
use lj_dtos::{AccountSummary, LinkedIdentity};

use super::login_page::auth_errors::translate_auth_error;
use crate::components::client_only::ClientOnly;
use crate::components::profile::{ApiKeysSettings, ThemeSettings};
use crate::components::ui::Skeleton;
use crate::pages::decision_page::data::sendable;

const CHAMP: &str = "rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] \
     px-3 py-2 text-sm text-[var(--color-ink)] outline-none focus:border-[var(--color-accent)]";
const BOUTON: &str = "self-start rounded-md bg-[var(--color-accent)] px-4 py-2 text-sm \
     font-medium text-white transition-opacity hover:opacity-90";
const ETIQUETTE: &str = "text-xs text-[var(--color-ink-subtle)]";
const TITRE_SECTION: &str =
    "text-sm font-medium uppercase tracking-widest text-[var(--color-ink-subtle)]";
const CARTE: &str =
    "flex items-center justify-between rounded-md border border-[var(--color-rule)] px-4 py-3";
const ACTION: &str = "text-sm text-[var(--color-accent)] underline hover:opacity-80";
const ACTION_RETRAIT: &str =
    "text-sm text-[var(--color-ink-muted)] underline hover:text-[var(--color-ink)]";
const FOURNISSEURS: [(&str, &str); 2] = [("google", "Google"), ("microsoft", "Microsoft")];

#[component]
pub fn ProfilePage() -> impl IntoView {
    let compte = Resource::new_blocking(|| (), move |_| sendable(charger()));

    view! {
        <Title text="Profil - LibreJustice" />
        <Meta name="robots" content="noindex" />
        <Suspense fallback=|| {
            view! { <ProfileSkeleton /> }
        }>
            {move || {
                compte
                    .get()
                    .map(|compte| match compte {
                        Some(compte) => view! { <ProfileView compte /> }.into_any(),
                        None => view! { <SessionPerdue /> }.into_any(),
                    })
            }}
        </Suspense>
    }
}

/// Résumé du compte. Au SSR : la session du porteur, lue en process — la garde
/// serveur a déjà exigé le cookie, `None` ne reste possible que si la session
/// tombe entre les deux. En navigation SPA : `GET /internal/account`.
async fn charger() -> Option<lj_dtos::AccountSummary> {
    #[cfg(feature = "ssr")]
    {
        let parts = use_context::<axum::http::request::Parts>()?;
        crate::api::ApiClient::from_context()
            .account_summary(&parts)
            .await
    }
    #[cfg(not(feature = "ssr"))]
    {
        crate::api::ApiClient::from_context()
            .fetch_account()
            .await
            .ok()
    }
}

/// Issue d'une action, lue dans la query au retour du 303.
fn message_done(code: &str) -> &'static str {
    match code {
        "nom" => "Profil mis à jour.",
        "google" => "Compte Google lié.",
        "microsoft" => "Compte Microsoft lié.",
        "deliaison" => "Connexion retirée.",
        "email" => "Adresse e-mail mise à jour.",
        _ => "C'est fait.",
    }
}

fn message_sent(code: &str) -> &'static str {
    match code {
        "password" => "E-mail envoyé : ouvrez le lien qu'il contient pour choisir le mot de passe.",
        "email_change" => {
            "E-mail envoyé à votre adresse actuelle : confirmez-y la demande de changement."
        }
        _ => "E-mail envoyé.",
    }
}

#[component]
fn ProfileView(compte: AccountSummary) -> impl IntoView {
    let query = leptos_router::hooks::use_query_map();
    let notice = move || {
        query.with(|q| {
            q.get("done")
                .map(|c| message_done(&c))
                .or_else(|| q.get("sent").map(|c| message_sent(&c)))
        })
    };
    let error = move || query.with(|q| q.get("error").map(|c| translate_auth_error(Some(&c))));

    let email = compte.email.clone().unwrap_or_else(|| "—".to_string());
    let nom = compte.display_name.clone().unwrap_or_default();
    let identites = compte.identities.clone();
    let a_mot_de_passe = compte.has_password;

    view! {
        <div class="mx-auto flex max-w-lg flex-col gap-10 px-4 py-12">
            <h1
                class="font-sans text-2xl text-[var(--color-ink)]"
                style="font-variation-settings: 'wght' 300"
            >
                "Profil"
            </h1>

            {move || {
                notice()
                    .map(|m| {
                        view! { <p class="text-sm text-[var(--color-ink-muted)]">{m}</p> }
                    })
            }}
            {move || {
                error()
                    .map(|m| {
                        view! { <p class="text-sm text-red-600">{m}</p> }
                    })
            }}

            <section class="flex flex-col gap-4">
                <h2 class=TITRE_SECTION>"Identité"</h2>
                <div class="rounded-md border border-[var(--color-rule)] px-4 py-3">
                    <p class=ETIQUETTE>"Adresse e-mail"</p>
                    <p class="mt-0.5 text-sm text-[var(--color-ink)]">{email}</p>
                </div>

                <form method="post" action="/internal/account/display-name" class="flex flex-col gap-3">
                    <label for="displayName" class=ETIQUETTE>
                        "Nom affiché"
                    </label>
                    <input
                        id="displayName"
                        name="display_name"
                        type="text"
                        maxlength="80"
                        value=nom
                        placeholder="Anonyme"
                        class=CHAMP
                    />
                    <button type="submit" class=BOUTON>
                        "Enregistrer"
                    </button>
                </form>

                <form method="post" action="/internal/account/email-change" class="flex flex-col gap-3">
                    <label for="newEmail" class=ETIQUETTE>
                        "Changer d'adresse e-mail : un lien de confirmation part vers l'adresse actuelle, puis vers la nouvelle"
                    </label>
                    <input
                        id="newEmail"
                        name="email"
                        type="email"
                        autocomplete="email"
                        required
                        placeholder="nouvelle@adresse.fr"
                        class=CHAMP
                    />
                    <button type="submit" class=BOUTON>
                        "Demander le changement"
                    </button>
                </form>
            </section>

            <section class="flex flex-col gap-4">
                <h2 class=TITRE_SECTION>"Apparence"</h2>
                <ClientOnly fallback=|| {
                    view! { <Skeleton class="h-16 w-full" /> }
                }>
                    <ThemeSettings />
                </ClientOnly>
            </section>

            <section class="flex flex-col gap-4">
                <h2 class=TITRE_SECTION>"Connexions"</h2>
                {FOURNISSEURS
                    .iter()
                    .map(|&(cle, nom)| {
                        let liee = identites.iter().find(|id| id.provider == cle).cloned();
                        view! { <CarteFournisseur cle nom liee /> }
                    })
                    .collect_view()}

                <div class=CARTE>
                    <p class="text-sm text-[var(--color-ink)]">"Mot de passe"</p>
                    <form method="post" action="/internal/account/password-link">
                        <button type="submit" class=ACTION>
                            {if a_mot_de_passe { "Changer" } else { "Créer" }}
                        </button>
                    </form>
                </div>
            </section>

            <section class="flex flex-col gap-4">
                <h2 class=TITRE_SECTION>"Clés d'API"</h2>
                <ClientOnly fallback=|| {
                    view! { <Skeleton class="h-24 w-full" /> }
                }>
                    <ApiKeysSettings />
                </ClientOnly>
            </section>

            <section class="flex flex-col gap-4 rounded-md border border-red-300 bg-red-50/50 p-4 dark:border-red-900/50 dark:bg-red-950/20">
                <h2 class="text-sm font-medium uppercase tracking-widest text-red-700 dark:text-red-400">
                    "Supprimer mon compte"
                </h2>
                <p class="text-sm text-[var(--color-ink-muted)]">
                    "Cette action supprime " <strong>"définitivement"</strong>
                    " votre compte et vos signets. Aucune restauration n'est possible."
                </p>
                <form method="post" action="/internal/account/delete" class="flex flex-col gap-3">
                    <label for="deleteConfirm" class=ETIQUETTE>
                        "Tapez " <strong>"SUPPRIMER"</strong> " pour confirmer"
                    </label>
                    <input
                        id="deleteConfirm"
                        name="confirm"
                        type="text"
                        autocomplete="off"
                        required
                        pattern="SUPPRIMER"
                        class="rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 py-2 text-sm text-[var(--color-ink)] outline-none focus:border-red-500"
                    />
                    <button
                        type="submit"
                        class="self-start rounded-md bg-red-600 px-4 py-2 text-sm font-medium text-white transition-opacity hover:opacity-90"
                    >
                        "Supprimer définitivement"
                    </button>
                </form>
            </section>
        </div>
    }
}

/// « Délier » est offert sans condition : l'API refuse le retrait du dernier
/// moyen d'accès et rend `error=dernier_acces`.
#[component]
fn CarteFournisseur(
    cle: &'static str,
    nom: &'static str,
    liee: Option<LinkedIdentity>,
) -> impl IntoView {
    view! {
        <div class=CARTE>
            <p class="text-sm text-[var(--color-ink)]">{nom}</p>
            {match liee {
                Some(id) => {
                    view! {
                        <form method="post" action="/internal/account/unlink">
                            <input type="hidden" name="provider" value=id.provider />
                            <input type="hidden" name="provider_sub" value=id.provider_sub />
                            <button type="submit" class=ACTION_RETRAIT>
                                "Délier"
                            </button>
                        </form>
                    }
                        .into_any()
                }
                None => {
                    view! {
                        <a
                            href=format!("/internal/auth/{cle}/start?link=1")
                            rel="external nofollow"
                            class=ACTION
                        >
                            "Lier"
                        </a>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

/// La garde serveur a laissé passer mais la session est tombée entre-temps
/// (navigation SPA après déconnexion dans un autre onglet).
#[component]
fn SessionPerdue() -> impl IntoView {
    view! {
        <div class="mx-auto flex max-w-lg flex-col gap-4 px-4 py-12">
            <p class="text-sm text-[var(--color-ink-muted)]">"Votre session a expiré."</p>
            <a
                href="/login?next=%2Fprofile"
                class="self-start text-sm text-[var(--color-accent)] underline hover:opacity-80"
            >
                "Se reconnecter"
            </a>
        </div>
    }
}

/// Skeleton du profil : calque le conteneur, le titre et les quatre sections.
/// Affiché le temps de la lecture du compte — la page ne blanchit jamais.
#[component]
fn ProfileSkeleton() -> impl IntoView {
    view! {
        <div aria-hidden="true" class="mx-auto flex max-w-lg flex-col gap-10 px-4 py-12">
            <Skeleton class="h-8 w-24" />
            <section class="flex flex-col gap-4">
                <Skeleton class="h-4 w-20" />
                <div class="flex flex-col gap-2 rounded-md border border-[var(--color-rule)] px-4 py-3">
                    <Skeleton class="h-3 w-24" />
                    <Skeleton class="h-4 w-48" />
                </div>
                <Skeleton class="h-3 w-20" />
                <Skeleton class="h-9 w-full" />
                <Skeleton class="h-8 w-28" />
            </section>
            <section class="flex flex-col gap-4">
                <Skeleton class="h-4 w-24" />
                <Skeleton class="h-16 w-full" />
            </section>
            <section class="flex flex-col gap-4">
                <Skeleton class="h-4 w-28" />
                <Skeleton class="h-12 w-full" />
                <Skeleton class="h-12 w-full" />
                <Skeleton class="h-12 w-full" />
            </section>
            <section class="flex flex-col gap-3 rounded-md border border-[var(--color-rule)] p-4">
                <Skeleton class="h-4 w-40" />
                <Skeleton class="h-4 w-full" />
                <Skeleton class="h-9 w-full" />
            </section>
        </div>
    }
}
