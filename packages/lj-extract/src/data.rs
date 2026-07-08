//! Données statiques embarquées (PUR : pas de lecture fs au runtime).
//!
//! Le snapshot des titres de codes Légifrance est `include_str!` depuis
//! `data/legifrance_codes.json` (copie versionnée dans la crate).

use serde::Deserialize;

/// JSON brut embarqué. Désérialisé paresseusement via [`legifrance_codes`].
pub const LEGIFRANCE_CODES_JSON: &str = include_str!("../data/legifrance_codes.json");

#[derive(Debug, Clone, Deserialize)]
pub struct LegifranceCode {
    /// LEGITEXT du code (= legitext du pont LEGI, ADR 0092).
    pub cid: String,
    pub titre: String,
    #[serde(default)]
    pub etat: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LegifranceCodes {
    pub codes: Vec<LegifranceCode>,
}

/// Désérialise le snapshot embarqué des codes Légifrance.
pub fn legifrance_codes() -> LegifranceCodes {
    serde_json::from_str(LEGIFRANCE_CODES_JSON).expect("legifrance_codes.json embarqué valide")
}

/// Gazetteer CCN embarqué (ADR 0123) : conteneurs KALICONT titrés « Convention
/// collective » du fond KALI. Régénéré par la requête documentée dans le JSON
/// (`legal_text` source kali). Consommé par [`crate::gazetteer`].
pub const CCN_GAZETTEER_JSON: &str = include_str!("../data/ccn_gazetteer.json");

#[derive(Debug, Clone, Deserialize)]
pub struct CcnGazetteerEntry {
    pub kalicont: String,
    pub title: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CcnGazetteerRaw {
    pub conventions: Vec<CcnGazetteerEntry>,
}

/// Désérialise le gazetteer CCN embarqué (le champ `_doc` est ignoré).
pub fn ccn_gazetteer_raw() -> CcnGazetteerRaw {
    serde_json::from_str(CCN_GAZETTEER_JSON).expect("ccn_gazetteer.json embarqué valide")
}

/// Table d'unicité d'articles : `label article → titre de code`, restreinte aux
/// numéros **nationalement uniques** (présents dans un seul code en vigueur,
/// mesuré sur Légifrance). Sert au SALVAGE des articles rescapés d'un instrument
/// garble (cf. `extract::common::salvage_code_for_garble`). Régénérée par
/// `lj-ingest legifrance-unicity`.
pub const LEGIFRANCE_ARTICLE_UNICITY_JSON: &str =
    include_str!("../data/legifrance_article_unicity.json");

#[derive(Debug, Clone, Deserialize)]
pub struct ArticleUnicity {
    pub articles: std::collections::BTreeMap<String, String>,
}

/// Désérialise la table d'unicité d'articles embarquée.
pub fn legifrance_article_unicity() -> ArticleUnicity {
    serde_json::from_str(LEGIFRANCE_ARTICLE_UNICITY_JSON)
        .expect("legifrance_article_unicity.json embarqué valide")
}

/// Registre de résolution garble → code (Tier B, ADR 0076) : pour les articles
/// **non** nationalement uniques (donc hors table d'unicité), une résolution
/// CONTEXTUELLE `(instrument garble, article) → code` minée par LLM (Mistral)
/// sous double garde — choix borné à l'ensemble candidat Légifrance + audit Opus.
/// Sert au SALVAGE (cf. `extract::common::salvage_code_for_garble`). Régénéré par
/// `lj-ingest tier-b-mine` + assemblage audité.
pub const GARBLE_RESOLUTION_REGISTRY_JSON: &str =
    include_str!("../data/garble_resolution_registry.json");

#[derive(Debug, Clone, Deserialize)]
pub struct GarbleResolution {
    pub instrument: String,
    pub article: String,
    pub code: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GarbleResolutionRegistry {
    pub resolutions: Vec<GarbleResolution>,
}

/// Désérialise le registre de résolution garble embarqué.
pub fn garble_resolution_registry() -> GarbleResolutionRegistry {
    serde_json::from_str(GARBLE_RESOLUTION_REGISTRY_JSON)
        .expect("garble_resolution_registry.json embarqué valide")
}

/// Table d'alias d'instruments (ADR 0077) : `variante → titre de code canonique`,
/// pour les variantes orthographiques/tronquées/à prose-collée qu'aucune règle
/// générique de `normalize_instrument` ne recolle sûrement (CESEDA mutilé, « Cgi »,
/// titres ambigus tranchés au contexte, prose résiduelle). Minée par subagents
/// puis auditée Opus. Consommée par `extract::common::canonicalize_instrument`.
pub const INSTRUMENT_ALIASES_JSON: &str = include_str!("../data/instrument_aliases.json");

#[derive(Debug, Clone, Deserialize)]
pub struct InstrumentAliases {
    pub aliases: std::collections::BTreeMap<String, String>,
}

/// Désérialise la table d'alias d'instruments embarquée.
pub fn instrument_aliases() -> InstrumentAliases {
    serde_json::from_str(INSTRUMENT_ALIASES_JSON).expect("instrument_aliases.json embarqué valide")
}

/// Alias de liaison (ADR 0145) : `lower(text_key)` → `text_uid` catalogue —
/// la connaissance curée de l'ex-table d'overrides, devenue du code (une
/// correction de masse = un commit sur ce fichier). TSV 4 colonnes :
/// `text_key`, `article_key` (vide = niveau texte), `ref_text_uid`,
/// `ref_num_key` (vide = par existence). Consommé par [`crate::link`].
pub const LINK_ALIASES_TSV: &str = include_str!("../data/link_aliases.tsv");
