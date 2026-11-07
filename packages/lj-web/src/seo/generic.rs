//! Meta generiques du site + page 404. Constantes canoniques partagees.

/// Origine canonique (URLs absolues, og:url, JSON-LD). Alignée sur la source
/// unique partagée backend ↔ front ([`lj_dtos::SITE_BASE`]).
pub const CANONICAL_BASE: &str = lj_dtos::SITE_BASE;
/// Carte Open Graph (1200x630).
pub const OG_IMAGE: &str = "https://librejustice.fr/og-card-v3.png";

/// Metadonnees d'une page, rendues via `leptos_meta`.
pub struct PageMeta {
    pub title: String,
    pub description: String,
    pub robots: Option<&'static str>,
}

/// Meta generiques du site (heritees par les pages sans `<Title>` propre).
pub fn site_default() -> PageMeta {
    PageMeta {
        title: "LibreJustice".to_string(),
        description: "Moteur de recherche libre sur le droit français : jurisprudence (Conseil \
                      d'État, Cour de cassation, cours d'appel, tribunaux) et textes (codes, \
                      lois, traités), mis à jour quotidiennement."
            .to_string(),
        robots: None,
    }
}

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

/// Meta de la page 404 (noindex).
pub fn not_found_meta() -> PageMeta {
    PageMeta {
        title: "Page introuvable - LibreJustice".to_string(),
        description: site_default().description,
        robots: Some("noindex"),
    }
}

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
