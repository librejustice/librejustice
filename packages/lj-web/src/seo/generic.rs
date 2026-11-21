//! Meta generiques du site + page 404. Constantes canoniques partagees.

/// Origine canonique (URLs absolues, og:url, JSON-LD). Alignée sur la source
/// unique partagée backend ↔ front ([`lj_dtos::SITE_BASE`]).
pub const CANONICAL_BASE: &str = lj_dtos::SITE_BASE;
/// Carte Open Graph (1200x630).
pub const OG_IMAGE: &str = "https://librejustice.fr/og-card-v3.png";

/// JSON-LD d'entité, posé sur l'accueil : une `Organization` et un `WebSite`
/// liés par `@id`. Les `sameAs` listent les endroits où le nom LibreJustice est
/// déjà porté ailleurs (dépôt, réutilisation data.gouv, annuaire MCP), ce qui
/// rattache ces profils au domaine pour un moteur.
pub const SITE_JSONLD: &str = r#"{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "Organization",
      "@id": "https://librejustice.fr/#organization",
      "name": "LibreJustice",
      "alternateName": "Libre Justice",
      "url": "https://librejustice.fr/",
      "logo": "https://librejustice.fr/icon-192.png",
      "sameAs": [
        "https://github.com/librejustice",
        "https://github.com/librejustice/librejustice",
        "https://www.data.gouv.fr/reuses/librejustice-moteur-de-recherche-libre-sur-la-jurisprudence-et-les-textes-de-loi-francais",
        "https://glama.ai/mcp/servers/fnxeb5mhsh"
      ]
    },
    {
      "@type": "WebSite",
      "@id": "https://librejustice.fr/#website",
      "name": "LibreJustice",
      "alternateName": "Libre Justice",
      "url": "https://librejustice.fr/",
      "inLanguage": "fr-FR",
      "publisher": { "@id": "https://librejustice.fr/#organization" }
    }
  ]
}"#;

/// URL canonique d'une page decision.
pub fn canonical_url(
    jurisdiction_type: lj_dtos::JurisdictionType,
    jurisdiction_code: Option<&str>,
    id: &str,
) -> String {
    format!(
        "{CANONICAL_BASE}{}",
        lj_dtos::decision_path(jurisdiction_type, jurisdiction_code, id)
    )
}

/// The page's canonical address in the reader's language: each version is its own canonical.
#[leptos::component]
pub fn Canonical(#[prop(into)] href: String) -> impl leptos::IntoView {
    let href = crate::i18n::in_lang(&href, crate::i18n::use_reader_lang().get());
    leptos::view! { <leptos_meta::Link rel="canonical" href=href /> }
}
