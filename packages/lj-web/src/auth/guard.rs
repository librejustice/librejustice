//! `AuthGuard` : protege les routes authentifiees (port de `auth-guard.tsx`).
//!
//! La session vit en `localStorage`, invisible au SSR : le gate se joue cote
//! client (Effect), redirige vers `/connexion?next=…` si absente, sinon affiche
//! les enfants. Pendant la verification, rend `fallback` (skeleton calque sur la
//! page protegee) au lieu d'une zone vide — l'ecran reste « plein » au reload.

use leptos::prelude::*;

/// Garde d'authentification. Enveloppe une page reservee aux connectes.
#[component]
pub fn AuthGuard(
    children: ChildrenFn,
    /// Rendu tant que la session n'est pas verifiee (SSR + 1er rendu client +
    /// duree du check). Skeleton calque sur la page protegee. Defaut : vide.
    #[prop(optional, into)]
    fallback: ViewFn,
) -> impl IntoView {
    // `ready` : la session a ete verifiee et est valide. Faux par defaut (SSR +
    // 1er rendu client) => on ne flashe jamais le contenu protege avant le check.
    let ready = RwSignal::new(false);

    #[cfg(feature = "hydrate")]
    {
        use leptos_router::hooks::{use_location, use_navigate};
        use leptos_router::NavigateOptions;
        let location = use_location();
        let navigate = use_navigate();
        Effect::new(move |_| {
            let pathname = location.pathname.get();
            let navigate = navigate.clone();
            leptos::task::spawn_local(async move {
                if super::has_session().await {
                    ready.set(true);
                } else {
                    let next = String::from(js_sys::encode_uri_component(&pathname));
                    navigate(
                        &format!("/connexion?next={next}"),
                        NavigateOptions {
                            replace: true,
                            ..Default::default()
                        },
                    );
                }
            });
        });
    }

    let children = StoredValue::new(children);
    move || {
        if ready.get() {
            children.with_value(|c| c()).into_any()
        } else {
            fallback.run().into_any()
        }
    }
}
