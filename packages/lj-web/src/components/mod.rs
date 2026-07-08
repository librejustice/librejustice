//! Composants partages (shell, kit UI) + sous-modules de tranche (decision,
//! search, activity, profile). Les sous-modules de tranche sont des STUBS a
//! remplir ; le kit `ui` et le shell (app_shell/top_bar/footer) sont partages
//! et figes (ne pas dupliquer cote tranches).

pub mod activity;
pub mod app_shell;
pub mod client_only;
pub mod decision;
pub mod decision_bar;
pub mod footer;
pub mod hover_preview;
pub mod profile;
pub mod search;
pub mod top_bar;
pub mod ui;

pub use app_shell::AppShell;
pub use footer::Footer;
pub use top_bar::{TopBar, Wordmark};
