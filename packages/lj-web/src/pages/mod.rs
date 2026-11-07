pub mod annonces_page;
pub mod annuaire;
pub mod api_guide;
pub mod authorize_mcp_page;
pub mod bookmarks_page;
pub mod code_catalogue_page;
pub mod confirm_page;
pub mod contribute_page;
pub mod corpus_page;
pub mod decision_page;
pub mod email_change_page;
pub mod entity_page;
pub mod feedback_page;
pub mod juridictions_page;
pub mod landing;
pub mod law_code_page;
pub mod law_compare_page;
pub mod law_page;
pub mod law_section_page;
pub mod legal_notice;
pub mod login_link_page;
pub mod login_page;
pub mod mcp_guide;
pub mod normes_page;
pub mod not_found;
pub mod open_source;
pub mod privacy;
pub mod profile_page;
pub mod redact;
pub mod redact_page;
pub mod reset_password_page;
pub mod search_page;
pub mod sources_page;
pub mod terms;
pub mod textes_page;
pub mod tools_page;

pub use annonces_page::{AnnoncesCataloguePage, AnnoncesDepartementPage, AnnoncesPage, MarchePage};
pub use annuaire::{AnnuaireDirectoryPage, AnnuairePage};
pub use api_guide::ApiGuide;
pub use authorize_mcp_page::AuthorizeMcpPage;
pub use bookmarks_page::BookmarksPage;
pub use code_catalogue_page::CodeCataloguePage;
pub use confirm_page::ConfirmPage;
pub use contribute_page::ContributePage;
pub use corpus_page::CorpusPage;
pub use decision_page::DecisionPage;
pub use email_change_page::EmailChangePage;
pub use entity_page::EntityPage;
pub use feedback_page::FeedbackPage;
pub use juridictions_page::{JuridictionHubPage, JuridictionYearPage, JuridictionsPage};
pub use landing::Landing;
pub use law_code_page::LawCodePage;
pub use law_page::NormNodePage;
pub use legal_notice::LegalNotice;
pub use login_link_page::LoginLinkPage;
pub use login_page::LoginPage;
pub use mcp_guide::McpGuide;
pub use normes_page::{NormFondPage, NormYearPage, NormesPage};
pub use not_found::NotFound;
pub use open_source::OpenSource;
pub use privacy::Privacy;
pub use profile_page::ProfilePage;
pub use redact_page::RedactPage;
pub use reset_password_page::ResetPasswordPage;
pub use search_page::SearchPage;
pub use sources_page::SourcesPage;
pub use terms::Terms;
pub use textes_page::TextesPage;
pub use tools_page::ToolsPage;

/// Déclare le document cachable par un cache partagé : 5 min de CDN,
/// revalidation navigateur à chaque navigation. Réservé aux pages dont le HTML
/// est identique pour tous les visiteurs — le défaut serveur étant `private,
/// no-store`, c'est la page qui dit le contraire. No-op côté hydrate.
#[cfg(feature = "ssr")]
pub(crate) fn cache_public_5_min() {
    use axum::http::{header::CACHE_CONTROL, HeaderValue};
    if let Some(resp) = leptos::prelude::use_context::<leptos_axum::ResponseOptions>() {
        resp.insert_header(
            CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=0, must-revalidate, s-maxage=300"),
        );
    }
}

#[cfg(not(feature = "ssr"))]
pub(crate) fn cache_public_5_min() {}
