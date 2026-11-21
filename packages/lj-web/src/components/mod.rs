//! Composants partages (shell, kit UI) + sous-modules de tranche (decision,
//! search, profile).

pub mod app_shell;
pub mod client_only;
pub mod decision;
pub mod decision_bar;
pub mod footer;
pub mod hover_preview;
pub mod jurisdiction_chip;
pub mod language;
pub mod layout;
pub mod profile;
mod prose;
pub mod search;
pub mod theme;
pub mod top_bar;
pub mod ui;

pub use app_shell::{own_page_description, AppShell, FillViewport};
pub use footer::Footer;
pub use prose::{prose, LINK};
pub use top_bar::{TopBar, Wordmark};
