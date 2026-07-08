//! Shim d'authentification : bindings wasm-bindgen vers `js/auth.js` (port de
//! `apps/web/src/lib/supabase.ts`) + `AuthGuard` (port de `auth-guard.tsx`).
//!
//! Cote SSR (pas de `localStorage`/session), toutes les fns sont des no-op
//! anonymes : `get_access_token() -> None`, l'AuthGuard ne redirige pas (le
//! gate effectif se joue cote client apres hydratation, comme le React legacy
//! qui montait `null` puis verifiait la session dans un `useEffect`).

mod bindings;
mod guard;

use leptos::prelude::*;

pub use guard::AuthGuard;

#[cfg(feature = "hydrate")]
pub use bindings::{
    current_email, has_session, on_auth_state_change, reset_password, sign_in_oauth,
    sign_in_password, sign_out, sign_up,
};

/// Jeton d'acces de la session courante (header `Authorization: Bearer`).
///
/// Cote SSR : toujours `None` (anonyme — pas de session locale). Cote wasm :
/// delegue au shim JS (Supabase `getSession`).
#[cfg(feature = "ssr")]
pub async fn get_access_token() -> Option<String> {
    None
}

/// Jeton d'acces de la session courante (header `Authorization: Bearer`).
#[cfg(feature = "hydrate")]
pub async fn get_access_token() -> Option<String> {
    bindings::get_access_token().await
}

/// Etat d'authentification reactif partage : email de la session courante,
/// `None` si anonyme. Toujours `None` au SSR (pas de session locale) ; peuple et
/// tenu a jour cote client par le pilote d'auth (`app.rs::provide_auth_runtime`).
/// Lu par la top-bar (avatar + menu compte).
#[derive(Clone, Copy)]
pub struct AuthState {
    pub email: RwSignal<Option<String>>,
}

/// Pose un `AuthState` vide dans le contexte. Le pilotage (lecture de session,
/// abonnement aux changements) vit dans `app.rs::provide_auth_runtime`, racine de
/// composition qui orchestre email + cache + theme depuis une source unique.
pub fn provide_auth_state() {
    provide_context(AuthState {
        email: RwSignal::new(None::<String>),
    });
}

/// Accede a l'`AuthState` du contexte (pose par `provide_auth_state`).
pub fn use_auth() -> AuthState {
    expect_context::<AuthState>()
}
