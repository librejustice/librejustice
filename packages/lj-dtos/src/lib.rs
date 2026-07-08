//! `lj-dtos` — contrats api ↔ web (port de `apps/api/.../schemas.py`).
//!
//! Tous les structs sont `#[serde(rename_all = "camelCase")]` (= `_CamelModel`
//! avec `alias_generator=to_camel` côté Python). DTOs = source de vérité (règle
//! #3) : `lj-api` et `lj-web` importent ces types, jamais de redéfinition côté
//! consommateur. Les enums de schéma (`JurisdictionLevel`, `MainOutcome`…)
//! vivent dans `schema` (ce crate).
//!
//! Fidélité Python : `_CamelModel` pose `extra="forbid"` ⇒ on annote les corps
//! de requête avec `#[serde(deny_unknown_fields)]`. Les `datetime.datetime`
//! Pydantic (timestamps profil/activité) sont portés en `String` (RFC 3339) :
//! `lj-dtos` n'a pas `chrono` en dépendance et reste un pur crate de contrats
//! sérialisés — la conversion `NaiveDate`/`DateTime` vit dans `lj-store`/`lj-api`.
//!
//! Noyau léger serde-pur partagé backend ↔ front wasm (ADR 0060) : aucune
//! dépendance lourde (pas de `lj-core`, qui tire `tokenizers`/`quick-xml`
//! incompatibles `wasm32`), pour garder le wasm minimal. La taxonomie (`schema`)
//! vit ici, source unique ; `lj-core` la réexporte.

use serde::{Deserialize, Serialize};

pub mod schema;

pub use schema::{Domaine, JuridictionType, JurisdictionLevel, Office, Portee, Solution, Voie};

// ── Enums propres à l'API (canal, mode de recherche, tri) ────────────────────

/// Canal d'origine d'une activité utilisateur (`web` = UI ; `mcp` = endpoint IA).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActivitySource {
    Web,
    Mcp,
}

/// Mode de recherche demandé. `semantic` force le hybride (cf. search.py).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    Auto,
    Lexical,
    Semantic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    Relevance,
    DateDesc,
    DateAsc,
}

/// Mode de recherche effectivement résolu et renvoyé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueryMode {
    Lexical,
    Hybrid,
}

// ── Health ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HealthResponse {
    /// Toujours `"ok"` (Python : `Literal["ok"]`).
    pub status: String,
    pub version: String,
}

// ── Stats corpus ───────────────────────────────────────────────────────────

/// Compteurs globaux du corpus pour la page d'accueil. Comptes exacts servis
/// depuis un cache process-local (TTL long) : le corpus ne bouge qu'à l'ingest
/// quotidien, donc un recalcul 2×/jour suffit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CorpusStatsResponse {
    /// Nombre de décisions actives indexées (non soft-deleted).
    pub decisions_count: i64,
    /// Nombre de codes/textes navigables (parité catalogue `/codes`).
    pub codes_count: i64,
    /// Nombre d'articles de loi en vigueur (somme des articles du catalogue).
    pub articles_count: i64,
}

// ── Search ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub juridiction_type: Option<Vec<JuridictionType>>,
    /// Solutions du dispositif (référentiel `solution:*`, ADR 0146).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<Vec<Solution>>,
    /// Voies procédurales (référentiel `voie:*`, ADR 0146).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voie: Option<Vec<Voie>>,
    /// Juges/offices spécialisés (référentiel `office:*`, ADR 0146).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub office: Option<Vec<Office>>,
    /// Domaines de référence (référentiel `domaine:*`, ADR 0146) — une racine
    /// sélectionnée matche elle-même + toutes ses feuilles (expansion API).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_domain: Option<Vec<Domaine>>,
    /// Codes du référentiel `jurisdiction` (`tj76351`, `ca_paris`, `cass_soc`…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_code: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_instrument: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_article: Option<Vec<String>>,
    /// Niveaux de publication (suffixes d'uid `publication:*`, ADR 0146).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<Vec<String>>,
    /// Portées jurisprudentielles (référentiel `portee:*`, ADR 0167) — groupes
    /// de `publication_codes` au rang le plus fort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portee: Option<Vec<Portee>>,
    /// Date ISO `YYYY-MM-DD`. Cap Python : `1678-01-01` ↔ `2262-01-01`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_to: Option<String>,
    #[serde(default = "default_search_mode")]
    pub mode: SearchMode,
    #[serde(default = "default_sort")]
    pub sort: SortOrder,
    #[serde(default = "default_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
    /// Mode IA : active le reranker LLM (ADR 0041). No-op en lexical.
    #[serde(default)]
    pub ai_mode: bool,
}

fn default_search_mode() -> SearchMode {
    SearchMode::Auto
}
fn default_sort() -> SortOrder {
    SortOrder::Relevance
}
fn default_limit() -> u32 {
    20
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BestChunk {
    pub chunk_index: i32,
    pub snippet: String,
}

/// Un choix de facette, servi avec son libellé depuis le référentiel DB
/// (ADR 0146 §4). `parent` porte la hiérarchie (arbres juridiction/domaine,
/// 2 niveaux) : `None` = racine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FacetChoice {
    pub value: String,
    pub label: String,
    pub count: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

/// Paire clé (code référentiel) + libellé FR résolue par l'API depuis
/// `facet_value` (ADR 0146) — le front rend le label sans table compilée.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FacetTag {
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegalInstrumentFacet {
    /// Token de filtre = `ref_text_uid` (référence catalogue, ADR 0145 M4).
    pub value: String,
    /// Libellé d'affichage = `legal_text.title` du uid.
    pub label: String,
    pub count: i64,
    #[serde(default)]
    pub articles: Vec<FacetChoice>,
}

/// Facettes de recherche (ADR 0146 §3, office séparé par l'ADR 0163) :
/// Juridiction (arbre) · Office · Domaine (arbre) · Solution · Publication ·
/// Date · Textes cités.
///
/// `juridiction` : niveau 1 = racines à valeur **uid complet** (`juridiction:TJ`,
/// types 0102) ; niveau 2 = codes `jurisdiction` (`tj76351`, `parent` = uid
/// racine). Les autres facettes portent le **suffixe** d'uid (`REJET`, `JEX`,
/// `CIVIL_DROIT_LOCATIF`…).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchFacets {
    #[serde(default)]
    pub juridiction: Vec<FacetChoice>,
    #[serde(default)]
    pub office: Vec<FacetChoice>,
    #[serde(default)]
    pub legal_domain: Vec<FacetChoice>,
    #[serde(default)]
    pub solution: Vec<FacetChoice>,
    #[serde(default)]
    pub portee: Vec<FacetChoice>,
    #[serde(default)]
    pub publication: Vec<FacetChoice>,
    #[serde(default)]
    pub date_lecture_year: Vec<FacetChoice>,
    #[serde(default)]
    pub legal_instrument: Vec<LegalInstrumentFacet>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchHit {
    pub id: String,
    pub juridiction_type: JuridictionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_name: Option<String>,
    pub title_html: String,
    pub score: f64,
    pub best_chunk: BestChunk,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    /// Tags référentiels résolus (clé + libellé, ADR 0146).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voie: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub office: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_domain: Option<FacetTag>,
    #[serde(default)]
    pub publication_codes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chars: Option<i64>,
    /// Résumé v4 embarqué en mode IA uniquement (ADR 0051).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchResponse {
    pub query: String,
    pub total: i64,
    pub hits: Vec<SearchHit>,
    pub query_mode: QueryMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facets: Option<SearchFacets>,
    #[serde(default)]
    pub all_hit_ids: Vec<String>,
}

// ── Decision detail ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegalReference {
    pub instrument: String,
    /// Slug du `legal_text` résolu (FK de citation, ADR 0123 §2) — présent quand
    /// la citation est ancrée au catalogue. Le front bâtit `/loi/{slug}/{numKey}`
    /// directement, sans re-slugifier. `None` ⇒ rendu brut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    pub articles: Vec<LegalRefArticle>,
}

/// Un article cité (ADR 0123 §2) : `num` = libellé affiché (brut source) ;
/// `numKey` = clé canonique résolue (`legal_citation.ref_num_key`) pour le lien
/// `/loi/{slug}/{numKey}` — vide si l'article n'a pas été ancré au catalogue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegalRefArticle {
    pub num: String,
    #[serde(default)]
    pub num_key: String,
}

/// Cible d'une mention de citation : un article (ou un texte) résolu. `href`
/// pointe l'article (`/loi/{slug}/{numKey}`) ; `None` ⇒ citation non résolue.
/// `label` = libellé de la cible (titre/clé canonique).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CitationTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    pub label: String,
}

/// Une mention cliquable d'une citation dans le corps d'un paragraphe (ADR 0125 /
/// 0134). `start`/`end` sont des offsets CODEPOINTS **locaux au paragraphe**
/// rendu, demi-ouverts `[start, end)`. `targets` porte une ou **plusieurs** cibles :
/// une citation multi-articles (« articles 1382, 1383 et 1384 du code civil »)
/// partage un seul span de texte mais vise N articles → le front rend un lien
/// simple pour 1 cible, un menu déroulant pour ≥2. Sur chevauchement de spans, on
/// fusionne en l'enveloppe et on unit les cibles (aucune n'est droppée).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CitationSpan {
    pub start: usize,
    pub end: usize,
    pub targets: Vec<CitationTarget>,
}

/// Bloc de corps structuré d'une décision (renderer pur). `kind` peut se répéter ;
/// `anchor` est l'ancre DOM **unique** ; `label` = titre affiché. ADR 0046 / 0048.
///
/// `paragraphSpans` (ADR 0134) : aligné index-à-index sur `paragraphs`, porte par
/// paragraphe ses mentions de citation cliquables (vide = aucune ; champ absent
/// pour l'ancien fonds dont les offsets ne sont pas peuplés).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionSection {
    pub kind: String,
    pub anchor: String,
    pub label: String,
    pub paragraphs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paragraph_spans: Vec<Vec<CitationSpan>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionSourceXml {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nom_juridiction: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub numero_dossier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formation_jugement: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_recours: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionDetail {
    pub id: String,
    pub juridiction_type: JuridictionType,
    /// Titre lisible machine/stable « <juridiction>, <date ISO>, <n° rôle> ».
    pub title: String,
    pub paragraphs: Vec<String>,
    /// Mentions de citation cliquables alignées sur `paragraphs` (ADR 0134), pour
    /// le rendu de repli sans sections autoritatives. Vide / absent si aucune.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paragraph_spans: Vec<Vec<CitationSpan>>,
    /// Sommaire/corps structuré autoritatif (None ⇒ heuristique front). ADR 0046.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sections: Option<Vec<DecisionSection>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    /// Tags référentiels résolus (clé + libellé, ADR 0146).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voie: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub office: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_domain: Option<FacetTag>,
    #[serde(default)]
    pub publication_codes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_audience: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formation_or_chamber: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_references: Option<Vec<LegalReference>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_xml: Option<DecisionSourceXml>,
    /// Mots-clés de matière (themes Judilibre), du général au spécifique. ADR 0090.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub themes: Vec<String>,
    /// Libellé brut de la nomenclature des affaires civiles (nac Judilibre). ADR 0090.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nac: Option<String>,
    /// ECLI (identifiant européen stable et citable) si présent. Provenance/audit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ecli: Option<String>,
    /// Source de provenance autoritaire (`decision_sources.source` : `judilibre`,
    /// `opendata`, `dila-jade`, `cnda`…). Provenance/audit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Chronologie de l'affaire (ADR 0161/0169) : décisions chaînées par les
    /// liens appel/pourvoi/renvoi résolus, décision courante incluse, de la
    /// plus récente à la plus ancienne. Vide si la décision n'est pas chaînée.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chronology: Vec<ChronologyEntry>,
}

/// Étape de la chronologie d'une affaire (ADR 0169) : une décision de la
/// chaîne appel/pourvoi/renvoi, identifiée par sa juridiction et sa date.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChronologyEntry {
    pub id: String,
    /// Nom de la juridiction (repli : libellé du type).
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// Vrai pour la décision affichée (rendue non cliquable).
    #[serde(default)]
    pub current: bool,
    /// Nature du lien vers l'étape suivante (la décision attaquée, juste en
    /// dessous) : `APPEL_DE` | `POURVOI_CONTRE` | `RENVOI_APRES_CASSATION`.
    /// Absent quand la paire adjacente n'est pas directement liée (chaîne
    /// lacunaire, affaires sérielles) et sur la dernière étape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

/// Libellé FR d'une source de provenance (`decision_sources.source`). Un code
/// inconnu (ne devrait pas arriver : ensemble fermé côté ingest) est renvoyé tel
/// quel, par transparence d'audit. Pas un vocabulaire de facette (ADR 0146) :
/// c'est de la provenance, hors référentiel `facet_value`.
pub fn source_label(code: &str) -> &str {
    match code {
        "judilibre" => "Judilibre (Cour de cassation)",
        "opendata" => "Open Data des juridictions administratives",
        "dila-jade" => "DILA — JADE (juridictions administratives)",
        "dila-constit" => "DILA — Conseil constitutionnel",
        "dila-cass" => "DILA — Cour de cassation",
        "dila-capp" => "DILA — cours d'appel",
        "cedh" => "CEDH — HUDOC",
        "cjue" => "CJUE — EUR-Lex",
        "cnda" => "Cour nationale du droit d'asile",
        other => other,
    }
}

/// Origine canonique du site (permaliens d'audit, URLs absolues). Source unique
/// partagée backend ↔ front : `lj-web` aligne sa `CANONICAL_BASE` dessus.
pub const SITE_BASE: &str = "https://librejustice.fr";

/// Permalien canonique LibreJustice d'une décision (`/decision/{id}`).
pub fn decision_permalink(id: &str) -> String {
    format!("{SITE_BASE}/decision/{id}")
}

/// Lignes de provenance/audit d'une décision — `(libellé, valeur)` dans l'ordre
/// d'affichage : source d'origine, ECLI, permalien LibreJustice. Builder pur
/// **partagé** par le bloc « Source » du web, l'export PDF et l'export DOCX —
/// contenu unique, rendu propre à chacun.
pub fn provenance_rows(detail: &DecisionDetail) -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    if let Some(src) = detail.source.as_deref().filter(|s| !s.is_empty()) {
        rows.push(("Source", source_label(src).to_string()));
    }
    if let Some(ecli) = detail.ecli.as_deref().filter(|s| !s.is_empty()) {
        rows.push(("ECLI", ecli.to_string()));
    }
    rows.push(("Permalien", decision_permalink(&detail.id)));
    rows
}

/// Décision proche (KNN embeddings). Port fidèle de `schemas.SimilarDecisionHit`
/// (champs canoniques + résumé), pas le stub à 3 champs du scaffold.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SimilarDecisionHit {
    pub id: String,
    pub juridiction_type: JuridictionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_name: Option<String>,
    pub score: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    /// Tags référentiels résolus (clé + libellé, ADR 0146).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voie: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub office: Option<FacetTag>,
    #[serde(default)]
    pub publication_codes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SimilarDecisionsResponse {
    pub decision_id: String,
    pub hits: Vec<SimilarDecisionHit>,
}

/// Prévisualisation légère d'une décision (hover card des liens de
/// jurisprudence, ADR 0168) : identité + solution + codes de publication +
/// résumé — jamais le corps.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionPreview {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voie: Option<FacetTag>,
    #[serde(default)]
    pub publication_codes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

// ── Référentiel LEGI versionné (ADR 0092) ────────────────────────────────────

/// Une version d'article dans la timeline LEGI (ADR 0092). `dateDebut`/`dateFin`
/// ISO `YYYY-MM-DD` ; `dateFin` absente ⇒ pas de fin (sentinelle normalisée NULL
/// à la frontière de parsing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawArticleVersion {
    pub legiarti: String,
    pub etat: String,
    pub date_debut: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_fin: Option<String>,
}

/// Défaut de `LawArticleResponse::translation` (droit FR LEGI = texte officiel).
fn default_officiel() -> String {
    "officiel".to_string()
}

/// Article LEGI servi à une date (law-at-date, ADR 0092). `code` = `code_court`
/// (slug). `dateDebut` ISO requise ; `dateFin` absente ⇒ pas de fin.
/// `legifranceUrl` versionnée `/codes/article_lc/{LEGIARTI}/{date}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawArticleResponse {
    pub legiarti: String,
    pub legitext: String,
    pub code: String,
    /// Titre humain du texte (`legal_text.title`, ex. « Code de la famille
    /// sénégalais ») pour l'affichage « Article N du … ». `code` reste le slug
    /// d'URL ; sans ce champ le front retombe sur le slug humanisé (laid).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_title: Option<String>,
    pub num: String,
    /// Clé canonique de l'article (`num_key`) pour les liens internes — l'URL
    /// `/loi/{code}/{numKey}` que le serve résout en lookup exact (ADR 0123 §2).
    /// `num` reste le libellé affiché.
    pub num_key: String,
    pub etat: String,
    pub date_debut: String,
    /// Provenance de la version servie (`legifrance` / `jorf` / `treaty`, ADR
    /// 0112) — pour lier vers la section descriptive de `/sources` (ADR 0114).
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub titre_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_fin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texte: Option<String>,
    /// Corps dans la langue d'origine (ADR 0116), affiché en regard du FR si présent
    /// (couche vérification/vérité ; souvent absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texte_original: Option<String>,
    /// Langue de `texteOriginal` (ISO-639-1, `ar`…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang_original: Option<String>,
    /// Provenance du `texte` (ADR 0116) : `officiel` / `non_officiel` / `automatique`
    /// → badge UI. Défaut `officiel` (droit FR LEGI).
    #[serde(default = "default_officiel")]
    pub translation: String,
    /// Fraîcheur « as-of » effective (ADR 0129) : dernière base crédible que la copie
    /// reflète le droit en vigueur (ISO `YYYY-MM-DD`). Jamais inconnue (au pire la date
    /// de get). `legifrance`/`kali` : date du dernier sync ; sinon date de get/curation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_asof: Option<String>,
    /// Autorité du **diffuseur** (ADR 0129), libellé FR (« source gouvernementale »,
    /// « agrégateur tiers », « traduction automatique »…). Axe DISTINCT de `translation`
    /// (officialité du texte). Calculé via mapping pur `lj_core::source_authority`.
    pub source_authority: String,
    /// Lien « source » unique de l'article (ADR 0129/0131) : page/PDF du diffuseur pour
    /// le curé (étranger, traités), ou page Légifrance versionnée pour un article natif
    /// LEGI/KALI (calculée côté API depuis l'identité). Absente seulement si aucune des
    /// deux n'est dérivable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// Source secondaire **amont** (ADR 0129) : site qu'un agrégateur (jafbase) pointe. Rare.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_upstream_url: Option<String>,
    /// Apparat éditorial affiché (ADR 0134) : la « Nota » officielle Légifrance (entrée
    /// en vigueur, QPC, applicabilité) ou la jurisprudence/doctrine + renvois « voir
    /// aussi » des éditions annotées curées. Rendu sous le corps, distinct du texte
    /// normatif. Absent si l'article n'en porte pas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nota: Option<String>,
    pub versions: Vec<LawArticleVersion>,
    /// Articles voisins pour la lecture en contexte (ADR 0114) : division
    /// enclosante ou fenêtre, l'article courant marqué `current`. Vide si pas de
    /// contexte exploitable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<ArticleNeighbor>,
}

/// Article voisin pour le contexte de lecture (ADR 0114) : numéro + état, sans
/// corps (clic = navigation `/loi/{code}/{numKey}`). `current` = l'article de la
/// page. `numKey` = clé canonique pour le lien (ADR 0123 §2) ; `num` = affichage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArticleNeighbor {
    pub num: String,
    pub num_key: String,
    pub etat: String,
    pub current: bool,
}

/// Sommaire d'un code LEGI (ADR 0092). `code` = `code_court` (slug).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawCodeSummary {
    pub legitext: String,
    pub code: String,
    pub titre: String,
    pub nature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derniere_modification: Option<String>,
    pub article_count: i64,
}

/// Décision citant un article LEGI (ADR 0092), via `legal_citation` (ADR 0145).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CitingDecisionHit {
    pub id: String,
    pub title: String,
    pub juridiction_type: JuridictionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
}

/// Un résultat de recherche d'article (ADR 0114, `/recherche-textes`). `code` =
/// slug du texte parent (lien `/loi/{code}/{numKey}`) ; `codeTitle` = titre
/// lisible du code ; `titrePath` = fil d'Ariane ; `snippet` = extrait surligné du
/// corps. `numKey` = clé canonique pour le lien (ADR 0123 §2), `num` = affichage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArticleSearchHit {
    pub code: String,
    pub code_title: String,
    pub num: String,
    pub num_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub titre_path: Option<String>,
    pub snippet: String,
    pub source: String,
}

/// Réponse de `/api/search-textes` (ADR 0114) : hits paginés, total exact sous le
/// prédicat BM25 + filtres, et comptes par facette.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArticleSearchResponse {
    pub hits: Vec<ArticleSearchHit>,
    pub total: i64,
    pub facets: ArticleSearchFacets,
}

/// Comptes par facette de la recherche d'articles, sous le même prédicat que les hits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArticleSearchFacets {
    pub jurisdiction: Vec<FacetChoice>,
    pub nature: Vec<FacetChoice>,
    pub source: Vec<FacetChoice>,
}

/// Entrée du catalogue des codes (`/api/codes`). `code` = slug (lien `/loi/{code}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodeCatalogueEntry {
    pub code: String,
    pub title: String,
    pub nature: String,
    pub jurisdiction: String,
    pub article_count: i64,
}

/// Réponse de `/api/codes` : liste des codes du corpus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodeCatalogueResponse {
    pub entries: Vec<CodeCatalogueEntry>,
}

/// Entrée de la table des matières d'un code. `numKey` = clé canonique, `num` =
/// affichage, `titlePath` = fil d'Ariane, `status` = état de l'article.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TocEntry {
    pub num: String,
    pub num_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_path: Option<String>,
    pub status: String,
}

/// Réponse de `/api/loi/{code}/sommaire` : table des matières d'un code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodeTocResponse {
    pub entries: Vec<TocEntry>,
}

// ── Compte utilisateur ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserProfile {
    pub sub: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Timestamp RFC 3339 (Python `datetime.datetime`).
    pub created_at: String,
    pub last_seen_at: String,
    pub track_activity: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserProfileUpdate {
    /// Pydantic : `min_length=1, max_length=80`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// Bascule du mode d'enregistrement d'activité (ADR 0056). `false` purge aussi
/// les données existantes (recherches, lectures, signets).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivityTrackingUpdate {
    pub enabled: bool,
}

// ── Signets ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BookmarkItem {
    pub id: String,
    pub title: String,
    pub juridiction_type: JuridictionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    /// Solution résolue (clé + libellé référentiel, ADR 0146).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub bookmarked_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BookmarksResponse {
    pub items: Vec<BookmarkItem>,
    pub total: i64,
}

// ── Historique de recherche ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchHistoryEntry {
    pub id: i64,
    pub query: String,
    /// `dict[str, Any]` côté Python : payload de filtres opaque.
    pub filters: serde_json::Value,
    pub source: ActivitySource,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchHistoryResponse {
    pub items: Vec<SearchHistoryEntry>,
    pub total: i64,
}

// ── Décisions consultées ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionViewItem {
    pub id: String,
    pub title: String,
    pub juridiction_type: JuridictionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    /// Solution résolue (clé + libellé référentiel, ADR 0146).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub view_count: i64,
    pub last_source: ActivitySource,
    pub last_viewed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionViewsResponse {
    pub items: Vec<DecisionViewItem>,
    pub total: i64,
}

// ── Filtrage articles de procédure (denylist, ADR 0058) ──────────────────────

/// Port fidèle de `schemas._PROCEDURAL_ARTICLE_DENYLIST` : articles de pure
/// procédure masqués de la sortie API (facet + détail) sans toucher la base.
/// `(instrument normalisé, &[articles])`.
mod procedural {
    pub(super) const DENYLIST: &[(&str, &[&str])] = &[
        (
            "Code de procédure civile",
            &[
                // frais et dépens
                "695", "696", "699", "700", // forme et prononcé du jugement
                "450", "451", "452", "453", "454", "455", "456", "457", "458", "459", "462", "463",
                "464", "465", "466", // mise en état
                "446-1", "446-2", "446-3", "446-4", "763", "776", "778", "779", "780", "785",
                "786", "787", "788", "789", "790", "799", "800", "802", "803", "804", "805", "807",
                "808", // exécution provisoire
                "514", "515", "517", "521", "524",
                // circuits d'appel et forme des conclusions
                "905", "905-1", "905-2", "906", "907", "908", "909", "910", "911", "912", "913",
                "914", "916", "954", "960", "961", "963", // désistement / péremption
                "384", "385", "394", "395", "399", // procédure de cassation
                "627", "974", "978", "979", "982", "1009-1", "1010", "1011", "1014", "1015",
                "1018", "1022", "1026", "1031-1",
            ],
        ),
        (
            "Code de procédure pénale",
            &[
                // forme de l'arrêt et procédure du pourvoi
                "567", "567-1-1", "568", "584", "585", "585-1", "586", "590", "591", "592", "593",
                "594", "598", "609-1", "612", "614", "615", "802",
            ],
        ),
        (
            "Code de l'organisation judiciaire",
            &[
                "L. 131-6",
                "L. 131-6-1",
                "L. 431-3",
                "L. 431-4",
                "L. 432-1",
                "R. 431-5",
            ],
        ),
        // frais (équivalent administratif de l'article 700 CPC)
        ("Code de justice administrative", &["L. 761-1"]),
        // aide juridictionnelle
        ("Loi du 10 juillet 1991", &["20", "24", "37", "75"]),
    ];
}

/// `true` si `(instrument, article)` est de la pure procédure (denylist).
///
/// Port de `schemas.is_procedural_article`. `article = None` ⇒ `false` (un
/// instrument cité sans article précis n'est jamais procédural). Sert à masquer
/// ces articles de la sortie API sans toucher la base. ADR 0058.
pub fn is_procedural_article(instrument: &str, article: Option<&str>) -> bool {
    let Some(article) = article else {
        return false;
    };
    procedural::DENYLIST
        .iter()
        .find(|(name, _)| *name == instrument)
        .is_some_and(|(_, arts)| arts.contains(&article))
}

/// Construit les `LegalReference` exposées, articles procéduraux masqués.
///
/// Port de `schemas.parse_legal_refs`. Un instrument réduit à de la pure
/// procédure après filtrage disparaît ; un instrument cité sans article précis
/// est conservé tel quel. `raw` est la valeur JSON brute des `legal_references`
/// extraites. Renvoie `None` si vide / tout filtré.
pub fn parse_legal_refs(raw: &serde_json::Value) -> Option<Vec<LegalReference>> {
    let items = raw.as_array()?;
    if items.is_empty() {
        return None;
    }
    let mut refs: Vec<LegalReference> = Vec::new();
    for r in items {
        let instrument = r.get("instrument")?.as_str()?.to_string();
        let original: Vec<String> = r
            .get("articles")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let articles: Vec<LegalRefArticle> = original
            .iter()
            .filter(|a| !is_procedural_article(&instrument, Some(a.as_str())))
            // Chemin brut (JSON stocké, sans résolution catalogue) : pas de `numKey`
            // résolu → vide, pas de lien (la résolution vit côté `lj-api`, ADR 0123 §2).
            .map(|a| LegalRefArticle {
                num: a.clone(),
                num_key: String::new(),
            })
            .collect();
        // Un instrument cité AVEC articles, tous procéduraux ⇒ on le retire.
        if !original.is_empty() && articles.is_empty() {
            continue;
        }
        refs.push(LegalReference {
            instrument,
            slug: None,
            articles,
        });
    }
    if refs.is_empty() {
        None
    } else {
        Some(refs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn search_mode_camel_and_lowercase_roundtrip() {
        assert_eq!(
            serde_json::to_string(&SearchMode::Auto).unwrap(),
            "\"auto\""
        );
        assert_eq!(
            serde_json::to_string(&SortOrder::DateDesc).unwrap(),
            "\"date_desc\""
        );
        assert_eq!(
            serde_json::to_string(&QueryMode::Hybrid).unwrap(),
            "\"hybrid\""
        );
        assert_eq!(
            serde_json::to_string(&ActivitySource::Mcp).unwrap(),
            "\"mcp\""
        );
    }

    #[test]
    fn search_request_camel_aliases_and_defaults() {
        // Minimal body : seul `query` est requis ; les défauts Pydantic doivent tenir.
        let req: SearchRequest = serde_json::from_str(r#"{"query":"bail commercial"}"#).unwrap();
        assert_eq!(req.query, "bail commercial");
        assert_eq!(req.mode, SearchMode::Auto);
        assert_eq!(req.sort, SortOrder::Relevance);
        assert_eq!(req.limit, 20);
        assert_eq!(req.offset, 0);
        assert!(!req.ai_mode);
        assert!(req.juridiction_type.is_none());

        // Les clés JSON sont en camelCase (= to_camel côté Pydantic).
        let body = json!({
            "query": "x",
            "juridictionType": ["CA", "TJ"],
            "dateFrom": "2020-01-01",
            "aiMode": true
        });
        let req: SearchRequest = serde_json::from_value(body).unwrap();
        assert_eq!(
            req.juridiction_type,
            Some(vec![JuridictionType::Ca, JuridictionType::Tj])
        );
        assert_eq!(req.date_from.as_deref(), Some("2020-01-01"));
        assert!(req.ai_mode);
    }

    #[test]
    fn search_request_forbids_unknown_fields() {
        // extra="forbid" côté Pydantic ⇒ deny_unknown_fields.
        let err = serde_json::from_str::<SearchRequest>(r#"{"query":"x","bogus":1}"#);
        assert!(err.is_err());
    }

    #[test]
    fn search_hit_serializes_camel_case() {
        let hit = SearchHit {
            id: "ce-1".into(),
            juridiction_type: JuridictionType::Ce,
            jurisdiction_name: Some("Conseil d'État".into()),
            title_html: "<b>x</b>".into(),
            score: 1.5,
            best_chunk: BestChunk {
                chunk_index: 0,
                snippet: "…".into(),
            },
            date_lecture: Some("2024-08-06".into()),
            docket_numbers: Some(vec!["12345".into()]),
            solution: Some(FacetTag {
                key: "REJET".into(),
                label: "Rejet".into(),
            }),
            voie: None,
            office: None,
            legal_domain: None,
            publication_codes: vec!["B".into()],
            chars: Some(4200),
            summary: None,
        };
        let v = serde_json::to_value(&hit).unwrap();
        assert_eq!(v["juridictionType"], "CE");
        assert_eq!(v["titleHtml"], "<b>x</b>");
        assert_eq!(v["bestChunk"]["chunkIndex"], 0);
        // Tags référentiels : paire clé + libellé résolue par l'API (ADR 0146).
        assert_eq!(v["solution"]["key"], "REJET");
        assert_eq!(v["solution"]["label"], "Rejet");
        // None ⇒ champ omis (skip_serializing_if).
        assert!(v.get("summary").is_none());
        assert!(v.get("voie").is_none());
    }

    #[test]
    fn facet_choice_parent_optional() {
        // `parent` : omis en sérialisation quand None, défaut à la désérialisation.
        let flat: FacetChoice =
            serde_json::from_str(r#"{"value":"REJET","label":"Rejet","count":3}"#).unwrap();
        assert_eq!(flat.parent, None);
        let child = FacetChoice {
            value: "tj76351".into(),
            label: "Le Havre".into(),
            count: 2,
            parent: Some("juridiction:TJ".into()),
        };
        let v = serde_json::to_value(&child).unwrap();
        assert_eq!(v["parent"], "juridiction:TJ");
        assert!(serde_json::to_value(&flat).unwrap().get("parent").is_none());
    }

    #[test]
    fn legi_article_response_serializes_camel_case() {
        let art = LawArticleResponse {
            legiarti: "LEGIARTI000006832947".into(),
            legitext: "LEGITEXT000006070721".into(),
            code: "code-civil".into(),
            code_title: Some("Code civil".into()),
            num: "L131-4".into(),
            num_key: "L131-4".into(),
            etat: "VIGUEUR".into(),
            date_debut: "1992-05-15".into(),
            source: "legifrance".into(),
            titre_text: Some("Livre Ier > Titre III".into()),
            date_fin: None,
            texte: Some("Le contrat…".into()),
            texte_original: None,
            lang_original: None,
            translation: "officiel".into(),
            source_asof: Some("2026-06-28".into()),
            source_authority: "source gouvernementale".into(),
            source_url: Some(
                "https://www.legifrance.gouv.fr/codes/article_lc/LEGIARTI000006832947/1992-05-15"
                    .into(),
            ),
            source_upstream_url: None,
            nota: Some("Conformément à la loi n° 2019-222…".into()),
            versions: vec![LawArticleVersion {
                legiarti: "LEGIARTI000006832947".into(),
                etat: "MODIFIE".into(),
                date_debut: "1804-03-15".into(),
                date_fin: Some("1992-05-14".into()),
            }],
            context: Vec::new(),
        };
        let v = serde_json::to_value(&art).unwrap();
        assert_eq!(v["legiarti"], "LEGIARTI000006832947");
        assert_eq!(v["dateDebut"], "1992-05-15");
        assert_eq!(v["titreText"], "Livre Ier > Titre III");
        assert_eq!(
            v["sourceUrl"],
            "https://www.legifrance.gouv.fr/codes/article_lc/LEGIARTI000006832947/1992-05-15"
        );
        assert_eq!(v["versions"][0]["dateFin"], "1992-05-14");
        // None ⇒ champs omis (skip_serializing_if).
        assert!(v.get("dateFin").is_none());
        // `nota` = apparat éditorial AFFICHÉ (ADR 0134) : exposé quand présent, omis si None.
        assert_eq!(v["nota"], "Conformément à la loi n° 2019-222…");
    }

    #[test]
    fn citing_decision_hit_serializes_camel_case() {
        let hit = CitingDecisionHit {
            id: "cass-1".into(),
            title: "Cour de cassation, 2024-01-10, 22-12.345".into(),
            juridiction_type: JuridictionType::Cc,
            date_lecture: Some("2024-01-10".into()),
        };
        let v = serde_json::to_value(&hit).unwrap();
        assert_eq!(v["juridictionType"], "CC");
        assert_eq!(v["dateLecture"], "2024-01-10");
    }

    #[test]
    fn procedural_denylist_matches_spec() {
        // CPC 700 (frais) + 905 (circuit d'appel) sont procéduraux.
        assert!(is_procedural_article(
            "Code de procédure civile",
            Some("700")
        ));
        assert!(is_procedural_article(
            "Code de procédure civile",
            Some("905-1")
        ));
        // Principes directeurs (CPC 16) NON inclus dans la denylist.
        assert!(!is_procedural_article(
            "Code de procédure civile",
            Some("16")
        ));
        // Article inconnu d'un code non répertorié.
        assert!(!is_procedural_article("Code civil", Some("1240")));
        // article=None ⇒ jamais procédural.
        assert!(!is_procedural_article("Code de procédure civile", None));
        // L. 761-1 CJA (frais administratif).
        assert!(is_procedural_article(
            "Code de justice administrative",
            Some("L. 761-1")
        ));
    }

    #[test]
    fn parse_legal_refs_filters_procedural() {
        // Instrument réduit à de la pure procédure après filtrage ⇒ disparaît.
        let raw = json!([
            {"instrument": "Code de procédure civile", "articles": ["700"]},
            {"instrument": "Code civil", "articles": ["1240", "1241"]},
            {"instrument": "Code de l'environnement", "articles": []}
        ]);
        let refs = parse_legal_refs(&raw).unwrap();
        // CPC (que 700) retiré ; Code civil conservé ; instrument sans article conservé.
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].instrument, "Code civil");
        let nums: Vec<&str> = refs[0].articles.iter().map(|a| a.num.as_str()).collect();
        assert_eq!(nums, vec!["1240", "1241"]);
        // Chemin brut : `numKey` non résolu (vide), `slug` absent.
        assert!(refs[0].articles.iter().all(|a| a.num_key.is_empty()));
        assert!(refs[0].slug.is_none());
        assert_eq!(refs[1].instrument, "Code de l'environnement");
        assert!(refs[1].articles.is_empty());
    }

    #[test]
    fn parse_legal_refs_mixed_articles_keeps_substantive() {
        // CPC avec un article de fond + un procédural ⇒ conservé, procédural retiré.
        let raw = json!([
            {"instrument": "Code de procédure civile", "articles": ["9", "700"]}
        ]);
        let refs = parse_legal_refs(&raw).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].articles.len(), 1);
        assert_eq!(refs[0].articles[0].num, "9");
    }

    #[test]
    fn parse_legal_refs_empty_is_none() {
        assert!(parse_legal_refs(&json!([])).is_none());
        assert!(parse_legal_refs(&json!(null)).is_none());
        // Tout filtré ⇒ None.
        let raw = json!([{"instrument": "Code de procédure civile", "articles": ["700", "696"]}]);
        assert!(parse_legal_refs(&raw).is_none());
    }
}
