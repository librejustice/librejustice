//! Pages (routes). Faites en T0 : confidentialite, landing, mcp_guide,
//! mentions_legales, not_found. Le reste est STUBBE (placeholder qui compile) —
//! a remplir par les agents de tranche selon le mapping de routes d'`app.rs`.

pub mod activity_page;
pub mod annuaire;
pub mod authorize_mcp_page;
pub mod code_catalogue_page;
pub mod confidentialite;
pub mod decision_page;
pub mod entity_page;
pub mod juridictions_page;
pub mod landing;
pub mod law_code_page;
pub mod law_compare_page;
pub mod law_page;
pub mod law_section_page;
pub mod login_page;
pub mod mcp_guide;
pub mod mentions_legales;
pub mod normes_page;
pub mod not_found;
pub mod profile_page;
pub mod reset_password_page;
pub mod search_page;
pub mod sources_page;
pub mod textes_page;

pub use activity_page::ActivityPage;
pub use annuaire::{AnnuaireDirectoryPage, AnnuairePage};
pub use authorize_mcp_page::AuthorizeMcpPage;
pub use code_catalogue_page::CodeCataloguePage;
pub use confidentialite::Confidentialite;
pub use decision_page::DecisionPage;
pub use entity_page::EntityPage;
pub use juridictions_page::{JuridictionHubPage, JuridictionYearPage, JuridictionsPage};
pub use landing::Landing;
pub use law_code_page::LawCodePage;
pub use law_compare_page::LawComparePage;
pub use law_page::LawArticlePage;
pub use law_section_page::LawSectionPage;
pub use login_page::LoginPage;
pub use mcp_guide::McpGuide;
pub use mentions_legales::MentionsLegales;
pub use normes_page::{NormFondPage, NormYearPage, NormesPage};
pub use not_found::NotFound;
pub use profile_page::ProfilePage;
pub use reset_password_page::ResetPasswordPage;
pub use search_page::SearchPage;
pub use sources_page::SourcesPage;
pub use textes_page::TextesPage;
