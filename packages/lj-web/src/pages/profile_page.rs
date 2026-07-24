//! Page `/profil` — port de `profile-page.tsx`.
//!
//! Sous `AuthGuard`. Profil charge en pur client (token Supabase en localStorage,
//! absent au SSR). Edition du nom affiche, reglages de theme, suppression de
//! compte (mot de confirmation « SUPPRIMER »).

use leptos::prelude::*;
use leptos_meta::{Meta, Title};

use lj_dtos::UserProfile;

#[cfg(feature = "hydrate")]
use crate::api::ApiClient;
use crate::auth::AuthGuard;
use crate::components::profile::ThemeSettings;
use crate::components::ui::Skeleton;

/// Mot a saisir pour confirmer la suppression (port de `DELETE_CONFIRM_WORD`).
const DELETE_CONFIRM_WORD: &str = "SUPPRIMER";

#[component]
pub fn ProfilePage() -> impl IntoView {
    view! {
        <Title text="Profil - LibreJustice" />
        <Meta name="robots" content="noindex" />
        <AuthGuard fallback=|| view! { <ProfileSkeleton /> }>
            <ProfileView />
        </AuthGuard>
    }
}

#[cfg(feature = "hydrate")]
fn client() -> ApiClient {
    ApiClient::from_context()
}

#[component]
fn ProfileView() -> impl IntoView {
    // Profil charge cote client dans un signal mutable : il sert de cache de page
    // (les mutations y poussent le profil renvoye, comme `setQueryData`). `loaded`
    // bascule a la fin du fetch (succes ou echec) : tant qu'il est faux on rend le
    // skeleton, jamais une zone vide.
    let profile = RwSignal::new(None::<UserProfile>);
    let loaded = RwSignal::new(false);
    Effect::new(move |_| {
        #[cfg(feature = "hydrate")]
        leptos::task::spawn_local(async move {
            if let Ok(p) = client().fetch_me().await {
                profile.set(Some(p));
            }
            loaded.set(true);
        });
    });

    let display_name = RwSignal::new(String::new());
    // Etats de la mutation « update me » (port de `updateMe.is*`).
    let update_pending = RwSignal::new(false);
    let update_error = RwSignal::new(None::<String>);
    let update_success = RwSignal::new(false);

    let delete_confirm = RwSignal::new(String::new());
    let delete_error = RwSignal::new(None::<String>);
    let delete_pending = RwSignal::new(false);
    let delete_api_error = RwSignal::new(None::<String>);

    // Seed du champ a l'arrivee du profil (sur la valeur serveur uniquement,
    // pour ne pas ecraser une saisie en cours).
    Effect::new(move |_| {
        if let Some(p) = profile.get() {
            display_name.set(p.display_name.unwrap_or_default());
        }
    });

    // `Callback` (Copy) : le formulaire vit dans la branche `<Show>` (ChildrenFn
    // rappelable), un handler `move |_|` capturant `navigate` (non-Copy) y serait
    // moved-out. Idem `on_profile_submit` pour l'homogeneite.
    let on_profile_submit = Callback::new(move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        #[cfg(feature = "hydrate")]
        {
            let trimmed = display_name.get_untracked().trim().to_string();
            let update = lj_dtos::UserProfileUpdate {
                display_name: if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                },
            };
            update_pending.set(true);
            update_error.set(None);
            update_success.set(false);
            leptos::task::spawn_local(async move {
                match client().update_me(&update).await {
                    Ok(updated) => {
                        profile.set(Some(updated));
                        update_success.set(true);
                    }
                    Err(e) => update_error.set(Some(e.message)),
                }
                update_pending.set(false);
            });
        }
    });

    #[cfg(feature = "hydrate")]
    let navigate = leptos_router::hooks::use_navigate();

    let on_delete_submit = Callback::new(move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if delete_confirm.get_untracked() != DELETE_CONFIRM_WORD {
            delete_error.set(Some(format!(
                "Tapez « {DELETE_CONFIRM_WORD} » pour confirmer."
            )));
            return;
        }
        delete_error.set(None);
        #[cfg(feature = "hydrate")]
        {
            let navigate = navigate.clone();
            delete_pending.set(true);
            delete_api_error.set(None);
            leptos::task::spawn_local(async move {
                match client().delete_account().await {
                    Ok(()) => {
                        crate::auth::sign_out().await;
                        navigate(
                            "/",
                            leptos_router::NavigateOptions {
                                replace: true,
                                ..Default::default()
                            },
                        );
                    }
                    Err(e) => {
                        delete_api_error.set(Some(e.message));
                        delete_pending.set(false);
                    }
                }
            });
        }
    });

    let email = move || {
        profile
            .get()
            .and_then(|p: UserProfile| p.email)
            .unwrap_or_else(|| "—".to_string())
    };

    view! {
        <Show when=move || loaded.get() fallback=|| view! { <ProfileSkeleton /> }>
            <div class="mx-auto flex max-w-lg flex-col gap-10 px-4 py-12">
                <h1
                    class="font-sans text-2xl text-[var(--color-ink)]"
                    style="font-variation-settings: 'wght' 300"
                >
                    "Profil"
                </h1>

                <section class="flex flex-col gap-4">
                    <h2 class="text-sm font-medium uppercase tracking-widest text-[var(--color-ink-subtle)]">
                        "Identité"
                    </h2>
                    <div class="rounded-md border border-[var(--color-rule)] px-4 py-3">
                        <p class="text-xs text-[var(--color-ink-subtle)]">"Adresse email"</p>
                        <p class="mt-0.5 text-sm text-[var(--color-ink)]">{email}</p>
                    </div>

                    <form on:submit=move |ev| on_profile_submit.run(ev) class="flex flex-col gap-3">
                        <label for="displayName" class="text-xs text-[var(--color-ink-subtle)]">
                            "Nom affiché"
                        </label>
                        <input
                            id="displayName"
                            type="text"
                            maxlength="80"
                            prop:value=move || display_name.get()
                            on:input=move |ev| display_name.set(event_target_value(&ev))
                            placeholder="Anonyme"
                            class="rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 py-2 text-sm text-[var(--color-ink)] outline-none focus:border-[var(--color-accent)]"
                        />
                        <Show when=move || update_error.get().is_some()>
                            <p class="text-sm text-red-600">
                                {move || update_error.get().unwrap_or_default()}
                            </p>
                        </Show>
                        <Show when=move || update_success.get()>
                            <p class="text-sm text-[var(--color-ink-muted)]">"Profil mis à jour."</p>
                        </Show>
                        <button
                            type="submit"
                            disabled=move || update_pending.get()
                            class="self-start rounded-md bg-[var(--color-accent)] px-4 py-2 text-sm font-medium text-white transition-opacity hover:opacity-90 disabled:opacity-50"
                        >
                            {move || if update_pending.get() { "…" } else { "Enregistrer" }}
                        </button>
                    </form>
                </section>

                <section class="flex flex-col gap-4">
                    <h2 class="text-sm font-medium uppercase tracking-widest text-[var(--color-ink-subtle)]">
                        "Apparence"
                    </h2>
                    <ThemeSettings />
                </section>

                <section class="flex flex-col gap-4 rounded-md border border-red-300 bg-red-50/50 p-4 dark:border-red-900/50 dark:bg-red-950/20">
                    <h2 class="text-sm font-medium uppercase tracking-widest text-red-700 dark:text-red-400">
                        "Supprimer mon compte"
                    </h2>
                    <p class="text-sm text-[var(--color-ink-muted)]">
                        "Cette action supprime " <strong>"définitivement"</strong>
                        " votre compte, vos signets et votre historique de recherche. Aucune restauration n'est possible."
                    </p>
                    <form on:submit=move |ev| on_delete_submit.run(ev) class="flex flex-col gap-3">
                        <label for="deleteConfirm" class="text-xs text-[var(--color-ink-subtle)]">
                            "Tapez " <strong>{DELETE_CONFIRM_WORD}</strong> " pour confirmer"
                        </label>
                        <input
                            id="deleteConfirm"
                            type="text"
                            autocomplete="off"
                            prop:value=move || delete_confirm.get()
                            on:input=move |ev| delete_confirm.set(event_target_value(&ev))
                            class="rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 py-2 text-sm text-[var(--color-ink)] outline-none focus:border-red-500"
                        />
                        <Show when=move || delete_error.get().is_some()>
                            <p class="text-sm text-red-600">
                                {move || delete_error.get().unwrap_or_default()}
                            </p>
                        </Show>
                        <Show when=move || delete_api_error.get().is_some()>
                            <p class="text-sm text-red-600">
                                {move || delete_api_error.get().unwrap_or_default()}
                            </p>
                        </Show>
                        <button
                            type="submit"
                            disabled=move || {
                                delete_pending.get() || delete_confirm.get() != DELETE_CONFIRM_WORD
                            }
                            class="self-start rounded-md bg-red-600 px-4 py-2 text-sm font-medium text-white transition-opacity hover:opacity-90 disabled:opacity-40"
                        >
                            {move || {
                                if delete_pending.get() {
                                    "Suppression…"
                                } else {
                                    "Supprimer définitivement"
                                }
                            }}
                        </button>
                    </form>
                </section>
            </div>
        </Show>
    }
}

/// Skeleton du profil : calque le conteneur, le titre et les trois sections
/// (identité / apparence / zone danger). Affiché pendant la vérification de
/// session (AuthGuard) puis le chargement du profil — la page ne blanchit jamais.
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
            <section class="flex flex-col gap-3 rounded-md border border-[var(--color-rule)] p-4">
                <Skeleton class="h-4 w-40" />
                <Skeleton class="h-4 w-full" />
                <Skeleton class="h-9 w-full" />
            </section>
        </div>
    }
}
