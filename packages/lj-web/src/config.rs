//! Config centralisee cote serveur (regle repo #5 : pas de `std::env::var`
//! disperse — tout passe par `Settings`). C'est l'UNIQUE endroit ou lj-web
//! touche l'environnement. Le port front + les assets vivent dans la config
//! cargo-leptos (`[[workspace.metadata.leptos]]`) + `LeptosOptions`, pas ici.
#![cfg(feature = "ssr")]

/// Reglages serveur du front. Prefix d'environnement obligatoire : `LIBREJUSTICE_`.
pub struct Settings {
    /// Projet Supabase (auth client-side). Injecte dans la page (`window`) au
    /// SSR pour le shim JS `js/auth.js`, qui ne peut pas lire `import.meta.env`.
    pub supabase_url: String,
    pub supabase_anon_key: String,
}

impl Settings {
    /// Lit la config depuis l'environnement. Les cles Supabase defautent a vide
    /// (auth desactivee si non configuree, cf. shim JS).
    pub fn from_env() -> Self {
        Self {
            supabase_url: std::env::var("LIBREJUSTICE_VITE_SUPABASE_URL").unwrap_or_default(),
            supabase_anon_key: std::env::var("LIBREJUSTICE_VITE_SUPABASE_PUBLISHABLE_KEY")
                .unwrap_or_default(),
        }
    }
}
