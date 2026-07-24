//! Page `/reinitialiser-mot-de-passe` — port de `reset-password-page.tsx`.
//!
//! Atterrissage du lien de reinitialisation envoye par email. Securite : le
//! formulaire n'apparait QUE sur l'evenement `PASSWORD_RECOVERY`, emis par
//! Supabase une fois le token du lien echange (le lien prouve le controle de la
//! boite mail). Une simple session active n'y donne pas acces.

use leptos::prelude::*;
use leptos_meta::{Meta, Title};
use leptos_router::components::A;

const PASSWORD_HINT: &str =
    "Au moins 8 caractères, dont une minuscule, une majuscule et un chiffre.";

/// Phase du flux (port du `useState<"verifying" | "ready" | "done">`).
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Verifying,
    Ready,
    Done,
}

#[component]
pub fn ResetPasswordPage() -> impl IntoView {
    let phase = RwSignal::new(Phase::Verifying);
    let password = RwSignal::new(String::new());
    let confirm = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let saving = RwSignal::new(false);

    #[cfg(feature = "hydrate")]
    {
        use super::login_page::browser;
        // Hash d'erreur eventuel (lien invalide / expire).
        if let Some(hash_err) = browser::read_auth_hash_error() {
            error.set(Some(hash_err.message));
            browser::clear_location_hash();
        }
        // Le formulaire ne s'ouvre que sur PASSWORD_RECOVERY. L'abonnement tient
        // un `Closure` JS (pas `Send`), incompatible avec le cleanup de l'Owner
        // reactif (`Send + Sync`) : on `forget` comme le fait la substrate pour
        // l'abonnement permanent d'`app.rs` (wasm mono-thread, page courte).
        let subscription = browser::on_auth_event(move |event| {
            if event == "PASSWORD_RECOVERY" {
                error.set(None);
                phase.set(Phase::Ready);
            }
        });
        std::mem::forget(subscription);
    }

    let on_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        error.set(None);
        let pw = password.get_untracked();
        let cf = confirm.get_untracked();
        if pw != cf {
            error.set(Some("Les mots de passe ne correspondent pas.".to_string()));
            return;
        }
        if pw.chars().count() < 8 {
            error.set(Some(
                "Le mot de passe doit contenir au moins 8 caractères.".to_string(),
            ));
        }
        #[cfg(feature = "hydrate")]
        {
            use super::login_page::browser;
            saving.set(true);
            leptos::task::spawn_local(async move {
                match browser::update_password(&pw).await {
                    Ok(()) => phase.set(Phase::Done),
                    Err(msg) => error.set(Some(msg)),
                }
                saving.set(false);
            });
        }
    };

    #[cfg(feature = "hydrate")]
    let navigate = leptos_router::hooks::use_navigate();
    // `Callback` (Copy) : le bouton « Continuer » vit dans la branche principale
    // du `<Show>` (closure `Fn` rappelable) ; un handler capturant `navigate`
    // (non-`Copy`) y serait « moved-out » (probleme `FnOnce`).
    let on_continue = Callback::new(move |_: leptos::ev::MouseEvent| {
        #[cfg(feature = "hydrate")]
        navigate("/decisions", Default::default());
    });

    view! {
        <Title text="Réinitialiser le mot de passe - LibreJustice" />
        <Meta name="robots" content="noindex" />
        <Show
            when=move || phase.get() == Phase::Done
            fallback=move || {
                view! {
                    <ResetForm
                        phase=phase
                        password=password
                        confirm=confirm
                        error=error
                        saving=saving
                        on_submit=on_submit
                    />
                }
            }
        >
            <div class="mx-auto flex max-w-sm flex-col gap-4 px-4 py-16">
                <h1 class="font-sans text-2xl text-[var(--color-ink)]">"Mot de passe mis à jour"</h1>
                <p class="text-[var(--color-ink-muted)]">
                    "Votre mot de passe a été modifié. Vous êtes connecté."
                </p>
                <button
                    type="button"
                    on:click=move |ev| on_continue.run(ev)
                    class="self-start rounded-md bg-[var(--color-accent)] px-4 py-2.5 text-sm font-medium text-white transition-opacity hover:opacity-90"
                >
                    "Continuer"
                </button>
            </div>
        </Show>
    }
}

#[component]
fn ResetForm(
    phase: RwSignal<Phase>,
    password: RwSignal<String>,
    confirm: RwSignal<String>,
    error: RwSignal<Option<String>>,
    saving: RwSignal<bool>,
    on_submit: impl Fn(leptos::ev::SubmitEvent) + Clone + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        <div class="mx-auto flex max-w-sm flex-col gap-8 px-4 py-16">
            <h1 class="font-sans text-2xl text-[var(--color-ink)]">
                "Réinitialiser le mot de passe"
            </h1>

            <Show
                when=move || phase.get() == Phase::Ready
                fallback=move || {
                    view! {
                        <div class="flex flex-col gap-3">
                            {move || {
                                match error.get() {
                                    Some(msg) => {
                                        view! { <p class="text-sm text-red-600">{msg}</p> }
                                            .into_any()
                                    }
                                    None => {
                                        view! {
                                            <p class="text-sm text-[var(--color-ink-muted)]">
                                                "Vérification du lien…"
                                            </p>
                                        }
                                            .into_any()
                                    }
                                }
                            }}
                            <p class="text-sm text-[var(--color-ink-muted)]">
                                "Ce lien doit être ouvert sur le navigateur depuis lequel vous l'avez demandé. "
                                <A
                                    href="/connexion"
                                    attr:class="text-[var(--color-accent)] underline hover:opacity-80"
                                >
                                    "Demander un nouveau lien"
                                </A>
                            </p>
                        </div>
                    }
                }
            >
                <form on:submit=on_submit.clone() class="flex flex-col gap-4">
                    <div class="flex flex-col gap-1.5">
                        <label for="password" class="text-sm text-[var(--color-ink-muted)]">
                            "Nouveau mot de passe"
                        </label>
                        <input
                            id="password"
                            type="password"
                            autocomplete="new-password"
                            required
                            prop:value=move || password.get()
                            on:input=move |ev| password.set(event_target_value(&ev))
                            class="rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 py-2 text-sm text-[var(--color-ink)] outline-none focus:border-[var(--color-accent)]"
                        />
                        <p class="text-xs text-[var(--color-ink-subtle)]">{PASSWORD_HINT}</p>
                    </div>
                    <div class="flex flex-col gap-1.5">
                        <label for="confirm" class="text-sm text-[var(--color-ink-muted)]">
                            "Confirmer le mot de passe"
                        </label>
                        <input
                            id="confirm"
                            type="password"
                            autocomplete="new-password"
                            required
                            prop:value=move || confirm.get()
                            on:input=move |ev| confirm.set(event_target_value(&ev))
                            class="rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 py-2 text-sm text-[var(--color-ink)] outline-none focus:border-[var(--color-accent)]"
                        />
                    </div>
                    <Show when=move || error.get().is_some()>
                        <p class="text-sm text-red-600">{move || error.get().unwrap_or_default()}</p>
                    </Show>
                    <button
                        type="submit"
                        disabled=move || saving.get()
                        class="rounded-md bg-[var(--color-accent)] px-4 py-2.5 text-sm font-medium text-white transition-opacity hover:opacity-90 disabled:opacity-50"
                    >
                        {move || if saving.get() { "…" } else { "Mettre à jour" }}
                    </button>
                </form>
            </Show>
        </div>
    }
}
