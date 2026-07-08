//! Meta generiques du site + page 404. Constantes canoniques partagees.

/// Origine canonique (URLs absolues, og:url, JSON-LD). Alignée sur la source
/// unique partagée backend ↔ front ([`lj_dtos::SITE_BASE`]).
pub const CANONICAL_BASE: &str = lj_dtos::SITE_BASE;
/// Carte Open Graph (1200x630).
pub const OG_IMAGE: &str = "https://librejustice.fr/og-card-v2.png";

/// Metadonnees d'une page, rendues via `leptos_meta`.
pub struct PageMeta {
    pub title: String,
    pub description: String,
    pub robots: Option<&'static str>,
}

/// Meta generiques du site (heritees par les pages sans `<Title>` propre).
pub fn site_default() -> PageMeta {
    PageMeta {
        title: "LibreJustice — recherche de jurisprudence française".to_string(),
        description: "Moteur de recherche libre sur la jurisprudence française : Conseil d'État, \
                      Cour de cassation, cours d'appel, tribunaux. Recherche hybride lexicale et \
                      sémantique."
            .to_string(),
        robots: None,
    }
}

/// Meta de la page 404 (noindex).
pub fn not_found_meta() -> PageMeta {
    PageMeta {
        title: "Page introuvable — LibreJustice".to_string(),
        description: site_default().description,
        robots: Some("noindex"),
    }
}

/// URL canonique d'une page decision.
pub fn canonical_url(id: &str) -> String {
    format!("{CANONICAL_BASE}/decision/{id}")
}
