//! Composants de profil (STUBS). A remplir par l'agent de la tranche
//! Profil/Activite : port de `apps/web/src/components/profile/theme-settings.tsx`.

pub mod theme_settings;

#[cfg(feature = "hydrate")]
pub use theme_settings::sync_auth_theme;
pub use theme_settings::ThemeSettings;
