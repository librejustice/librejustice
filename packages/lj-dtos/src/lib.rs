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

pub use schema::{
    abbreviate_jurisdiction_name, Domain, JurisdictionLevel, JurisdictionType, Office, Procedure,
    Significance, Solution,
};

// ── Enums propres à l'API (canal, mode de recherche, tri) ────────────────────

/// Canal d'origine d'une activité utilisateur (`web` = UI ; `mcp` = endpoint IA).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActivitySource {
    Web,
    Mcp,
}

/// Moteur interrogé par une recherche de l'historique (ADR 0251) — axe
/// orthogonal au canal `ActivitySource` : `decisions` (jurisprudence) ou
/// `textes` (référentiel de normes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchEngine {
    Decisions,
    Textes,
}

/// Contexte d'un appel de recherche (param transport `context`, pas un
/// filtre) : `user` = recherche posée par l'utilisateur ; `teaser` = fetch
/// machine des ponts croisés décisions ↔ textes — jamais enregistré en
/// historique, marqué sur le span pour l'exclure des comptages d'usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchContext {
    User,
    Teaser,
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
    /// Nombre de textes normatifs du corpus (tout `legal_text`, toutes natures :
    /// codes, lois, traités, décrets, arrêtés, circulaires… — pas seulement le
    /// catalogue navigable `/codes`).
    pub texts_count: i64,
    /// Nombre d'articles en vigueur du corpus (identités `(text_uid, num_key)`
    /// distinctes, statut `VIGUEUR`).
    pub articles_count: i64,
}

// ── Search ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_type: Option<Vec<JurisdictionType>>,
    /// Solutions du dispositif (référentiel `solution:*`, ADR 0146).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<Vec<Solution>>,
    /// Voies procédurales (référentiel `procedure:*`, ADR 0146).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub procedure: Option<Vec<Procedure>>,
    /// Juges/offices spécialisés (référentiel `office:*`, ADR 0146).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub office: Option<Vec<Office>>,
    /// Domaines de référence (référentiel `legal_domain:*`, ADR 0146) — une racine
    /// sélectionnée matche elle-même + toutes ses feuilles (expansion API).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_domain: Option<Vec<Domain>>,
    /// Codes du référentiel `jurisdiction` (`tj76351`, `ca_paris`, `cc`…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_code: Option<Vec<String>>,
    /// Chambres (catégorie contrôlée `chamber:*`, ADR 0172) — axe uniforme
    /// tous ordres (suffixes d'uid : `CIVILE`, `SOCIALE`, `ETRANGERS`…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chamber: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_instrument: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_article: Option<Vec<String>>,
    /// Niveaux de publication (suffixes d'uid `publication:*`, ADR 0146).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<Vec<String>>,
    /// Portées jurisprudentielles (référentiel `significance:*`, ADR 0167) — groupes
    /// de `publication_codes` au rang le plus fort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub significance: Option<Vec<Significance>>,
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
    /// Slug URL du texte (`legal_text.slug`) — token lisible côté MCP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    pub count: i64,
    #[serde(default)]
    pub articles: Vec<FacetChoice>,
}

/// Facettes de recherche (ADR 0146 §3, office séparé par l'ADR 0163) :
/// Juridiction (arbre) · Office · Domaine (arbre) · Solution · Publication ·
/// Date · Textes cités.
///
/// `juridiction` : niveau 1 = racines à valeur **uid complet** (`jurisdiction_type:TJ`,
/// types 0102) ; niveau 2 = codes `jurisdiction` (`tj76351`, `parent` = uid
/// racine). Les autres facettes portent le **suffixe** d'uid (`REJET`, `JEX`,
/// `CIVIL_DROIT_LOCATIF`…).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchFacets {
    #[serde(default)]
    pub jurisdiction: Vec<FacetChoice>,
    /// Chambre (catégorie contrôlée `chamber:*`, ADR 0172) — axe uniforme
    /// tous ordres, remplace le grain-chambre Cassation de la facette juridiction.
    #[serde(default)]
    pub chamber: Vec<FacetChoice>,
    #[serde(default)]
    pub office: Vec<FacetChoice>,
    #[serde(default)]
    pub legal_domain: Vec<FacetChoice>,
    #[serde(default)]
    pub solution: Vec<FacetChoice>,
    #[serde(default)]
    pub significance: Vec<FacetChoice>,
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
    pub jurisdiction_type: JurisdictionType,
    /// Code de cour précis (`jurisdiction:*`, ex. `tj_paris`) — token du
    /// filtre `jurisdiction_code`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_name: Option<String>,
    pub title_html: String,
    /// Siège (chambre/formation/office) recomposé, rendu en 2ᵉ ligne discrète
    /// sous le titre (ADR 0170) — hors du `title_html` qui reste sur une ligne.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<String>,
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
    pub procedure: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub office: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_domain: Option<FacetTag>,
    /// Spécialisation de chambre (`chamber:*`), tag référentiel résolu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chamber: Option<FacetTag>,
    /// Catégorie de publication (`publication:*`, de référence-6), tag résolu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<FacetTag>,
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
    /// la citation est ancrée au catalogue. Le front bâtit `/texte/{slug}/{numKey}`
    /// directement, sans re-slugifier. `None` ⇒ rendu brut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    pub articles: Vec<LegalRefArticle>,
}

/// Un article cité (ADR 0123 §2) : `num` = libellé affiché (brut source) ;
/// `numKey` = clé canonique résolue (`legal_citation.ref_num_key`) pour le lien
/// `/texte/{slug}/{numKey}` — vide si l'article n'a pas été ancré au catalogue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegalRefArticle {
    pub num: String,
    #[serde(default)]
    pub num_key: String,
}

/// Cible d'une mention de citation : un article (ou un texte) résolu. `href`
/// pointe l'article (`/texte/{slug}/{numKey}`) ; `None` ⇒ citation non résolue.
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
    pub nom_jurisdiction_type: Option<String>,
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
    pub jurisdiction_type: JurisdictionType,
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
    /// Code de cour précis (`jurisdiction:*`, ex. `tj_paris`) — token du
    /// filtre `jurisdiction_code`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    /// Tags référentiels résolus (clé + libellé, ADR 0146).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub procedure: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub office: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_domain: Option<FacetTag>,
    /// Catégorie de publication (`publication:*`, de référence-6), tag résolu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<FacetTag>,
    #[serde(default)]
    pub publication_codes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_audience: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    /// Siège composé depuis les axes structurés (ADR 0170) : position de
    /// chambre qualifiée par le type de formation ou l'office — « pôle 5 —
    /// 3e chambre (formation à trois) ». Jamais une chaîne source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<String>,
    /// Spécialisation de chambre (`chamber:*`) et type de formation
    /// (`formation:*`), tags référentiels résolus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chamber: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formation: Option<FacetTag>,
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
    /// Commentaires institutionnels (ADR 0204) : analyses officielles dépliées,
    /// lien vers les conclusions du rapporteur public, documents liés (rapports,
    /// avis, communiqués Cass). Non cherchables — enrichissement de la fiche,
    /// rendu en accordéon fermé en fin de page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commentaires: Vec<Commentaire>,
}

/// Commentaire institutionnel d'une décision (ADR 0204) : contenu porté
/// localement déplié sur place (`body`) ou ligne-lien externe (`url`). Trois
/// formes : `analyse` (body local, AJCE), `conclusions` (existence seule, lien
/// composé vers ArianeWeb), `note` (document lié — rapports/avis/communiqués
/// Cass, notes de doctrine — rendu comme lien titré vers l'éditeur).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Commentaire {
    /// `analyse` | `conclusions` | `note`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Date ISO du document commenté (date de lecture).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Libellé du lien (`note`) : type de document Cass, titre de la note…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Éditeur/source du lien (`Cour de cassation`, `GISTI`…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    /// Accès : `libre` | `abonnes`. Absent = libre.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<String>,
    /// Rubriques du plan de classement (`code : libellé hiérarchique`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rubriques: Vec<String>,
    /// Renvois doctrinaux (`(1) Cf. CE, …`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renvois: Vec<String>,
    /// Lien externe — présent = la ligne se rend comme un lien sortant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
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
    /// Clé de solution de l'étape (`INFIRMATION`, `CONFIRMATION`, `REJET`…) :
    /// le sort d'une décision se lit dans la solution de celle qui la révise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<String>,
    /// Numéros RG de l'étape — identifient sans ambiguïté quelle décision une
    /// infirmation/confirmation vise (les titres seuls prêtent à confusion).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
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
        "dila-cnil" => "DILA — CNIL (délibérations)",
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
    pub jurisdiction_type: JurisdictionType,
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
    pub procedure: Option<FacetTag>,
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
    pub procedure: Option<FacetTag>,
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
    /// Version future (`date_debut` > aujourd'hui, calculé côté API — l'horloge
    /// vit côté serveur, ADR 0178) : badge « à venir » sur la frise.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub upcoming: bool,
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
    /// `/texte/{code}/{numKey}` que le serve résout en lookup exact (ADR 0123 §2).
    /// `num` reste le libellé affiché.
    pub num_key: String,
    pub etat: String,
    pub date_debut: String,
    /// Provenance de la version servie (`legifrance` / `jorf` / `treaty`, ADR
    /// 0112) — pour lier vers la section descriptive de `/sources` (ADR 0114).
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub titre_text: Option<String>,
    /// Fil d'Ariane TOC cliquable : les divisions enclosantes de l'article,
    /// de la racine à la section directe, `href` = vue-lecture de la section
    /// (`/texte/{code}/section/{cid}`, ADR 0207). Vide quand la structure n'est
    /// pas ingérée (texte étranger, JORF) — le front retombe sur `titreText`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub breadcrumb: Vec<LinkedTextRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_fin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texte: Option<String>,
    /// Renvois cliquables du corps (ADR 0217) : spans codepoints demi-ouverts
    /// sur `texte` (convention 0143), même forme que les décisions (ADR 0134).
    /// Hrefs datés quand l'article est servi à date explicite — le renvoi
    /// navigue dans le même temps que la lecture.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub texte_spans: Vec<CitationSpan>,
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
    /// Première version future de l'article (`date_debut` > aujourd'hui, ISO,
    /// calculé côté API — ADR 0178) : bandeau « sera modifié le … ». La
    /// sentinelle `2222-02-22` signifie « à une date à déterminer ».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upcoming_version_date: Option<String>,
    pub versions: Vec<LawArticleVersion>,
    /// Articles voisins pour la lecture en contexte (ADR 0114) : division
    /// enclosante ou fenêtre, l'article courant marqué `current`. Vide si pas de
    /// contexte exploitable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<ArticleNeighbor>,
    /// Corps « dispositions modifiées » d'un article modificatif (ordonnance/loi
    /// de réforme), dérivé du graphe `legal_link` (bloc `<LIENS>` DILA, ADR 0174) :
    /// cibles exactes par ID, liens garantis. Non vide ⇒ le front rend cette liste
    /// au lieu de `texte` (qui reste le résumé brut, illisible). Vide pour un
    /// article normal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modifications: Vec<ArticleModification>,
    /// Textes qui ont modifié/créé/abrogé CET article (liens entrants du graphe,
    /// ADR 0174) : l'historique « Modifié par », dans l'ordre du fichier source.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modified_by: Vec<LinkedTextRef>,
    /// Dispositions que cet article cite (liens sortants CITATION, ADR 0174).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cites: Vec<LinkedTextRef>,
    /// Dispositions qui citent cet article (liens entrants CITATION, ADR 0174).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cited_by: Vec<LinkedTextRef>,
    /// Commentaires de norme (ADR 0212) : doctrine ancrée sur cet article ou
    /// sur le texte entier, même forme que côté décision (ADR 0204). Non
    /// cherchables — accordéon fermé en fin de page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commentaires: Vec<Commentaire>,
    /// Travaux parlementaires (ADR 0215, zéro ingest) : une ligne par **loi**
    /// modificatrice de l'article, `href` composé
    /// `https://www.legifrance.gouv.fr/jorf/id/{JORFTEXT}` (bloc « Travaux
    /// préparatoires » et dossiers législatifs servis par Légifrance).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub travaux_parlementaires: Vec<LinkedTextRef>,
}

/// Opération d'un segment du comparateur de versions (ADR 0193).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LawCompareOp {
    Equal,
    Insert,
    Delete,
}

/// Tronçon contigu du diff entre deux rédactions (ADR 0193). Le texte d'un côté
/// se reconstruit en concaténant `equal`+`delete` (ancien) ou `equal`+`insert`
/// (nouveau) ; les sauts de ligne restent dans `text`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawCompareSegment {
    pub op: LawCompareOp,
    pub text: String,
}

/// Réponse du comparateur de versions d'un article (ADR 0193) : identité,
/// les deux versions comparées (métadonnées de frise), le diff en segments,
/// et la timeline complète pour les sélecteurs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawCompareResponse {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_title: Option<String>,
    pub num: String,
    pub num_key: String,
    pub from: LawArticleVersion,
    pub to: LawArticleVersion,
    pub segments: Vec<LawCompareSegment>,
    pub versions: Vec<LawArticleVersion>,
}

/// Une référence de texte/article liée par le graphe `legal_link` (ADR 0174) :
/// libellé verbatim DILA (« LOI n°2018-287 du 20 avril 2018 - art. 16 »), lien
/// interne quand la cible est en base, date de signature du texte si connue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkedTextRef {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

/// Article voisin pour le contexte de lecture (ADR 0114) : numéro + état, sans
/// corps (clic = navigation `/texte/{code}/{numKey}`). `current` = l'article de la
/// page. `numKey` = clé canonique pour le lien (ADR 0123 §2) ; `num` = affichage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArticleNeighbor {
    pub num: String,
    pub num_key: String,
    pub etat: String,
    pub current: bool,
}

/// Une cible d'une disposition d'article modificatif (graphe `legal_link`,
/// ADR 0174) : un article (numéro linkable, `href` garanti quand la cible est
/// en base), une division de code (`section`, non linkable — ancres en
/// Phase B), ou un texte entier (`texte`, lié à son sommaire).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModificationItem {
    /// `article` (numéro dans `label`), `section` (titre de division) ou
    /// `texte` (libellé complet).
    pub kind: String,
    /// Numéro d'article (« 1302 », « L611-7 »), titre de section, ou libellé.
    pub label: String,
    /// Lien interne (`/texte/{code}/{numKey}` pour un article résolu,
    /// `/texte/{code}` pour un texte) ; absent si la cible n'est pas en base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

/// Un bloc du corps « dispositions modifiées » d'un article modificatif (graphe
/// `legal_link`, ADR 0174). Une action portant sur un texte cible, avec ses
/// dispositions. Rendu en liste par le front — remplace le rendu brut illisible
/// du résumé de liens (aligné sur  / de référence).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArticleModification {
    /// `modifie` / `cree` / `abroge`.
    pub action: String,
    /// Nom affichable du code cible (« Code civil »).
    pub code: String,
    /// Lien interne vers le sommaire du code cible (`/texte/{code}`) quand résolu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_href: Option<String>,
    pub items: Vec<ModificationItem>,
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
    /// Dates programmées de versions futures du texte (`VERSIONS_A_VENIR`
    /// DILA, ADR 0178), ISO, triées — bandeau « sera modifié le … ». La
    /// sentinelle `2222-02-22` signifie « à une date à déterminer ».
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upcoming_versions: Vec<String>,
    /// Corps monolithique d'un texte sans articles (circulaires…, ADR 0196).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// État de diffusion du texte (`VIGUEUR`/`ABROGE`, ADR 0196).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// NOR (familles qui en portent : circulaires, actes JO).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nor: Option<String>,
    /// Date de signature ISO (`date_texte`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_texte: Option<String>,
    /// Libellé de portée quand le texte relève de la doctrine administrative
    /// (ADR 0196) ; absent pour les normes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Fond du catalogue des normes (ADR 0255) — maillage retour vers
    /// `/normes/{fond}` ; absent pour un acte individuel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fond: Option<String>,
    /// Libellé du fond (« Lois et ordonnances »).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fond_label: Option<String>,
    /// Année de parcours du texte dans son fond (`/normes/{fond}/{annee}`) ;
    /// absente = bucket `sans-date`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fond_year: Option<i32>,
}

/// Décision citant un article LEGI (ADR 0092), via `legal_citation` (ADR 0145).
/// `portee` = portée jurisprudentielle (groupes de `publication_codes`,
/// ADR 0167) — ordre de la liste et badge « Portée majeure/importante ».
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CitingDecisionHit {
    pub id: String,
    pub title: String,
    pub jurisdiction_type: JurisdictionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    pub significance: Significance,
    /// Résumé de la décision (première phrase affichée en carte, comme les
    /// décisions similaires). Absent tant que le cron résumé n'est pas passé.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Article « souvent cité avec » l'article de la page (plan graphe Phase D) :
/// co-citation dans `legal_citation` sur un échantillon de décisions citantes,
/// boilerplate procédural exclu. `numKey` = numéro affichable et clé de lien ;
/// `count` = décisions de l'échantillon citant les deux articles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoCitedArticle {
    pub num_key: String,
    pub text_title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    pub count: i64,
}

/// Un résultat de recherche d'article (ADR 0114, `/recherche-textes`). `code` =
/// slug du texte parent (lien `/texte/{code}/{numKey}`) ; `codeTitle` = titre
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
    /// Textes porteurs de hits : `value` = slug (`/texte/{slug}`), token du filtre `code`.
    #[serde(default)]
    pub code: Vec<FacetChoice>,
    pub jurisdiction: Vec<FacetChoice>,
    pub nature: Vec<FacetChoice>,
    pub source: Vec<FacetChoice>,
    /// Sur-facette « portée » (ADR 0196) : `norme` | `doctrine_administrative`,
    /// agrégée côté API depuis les buckets `nature` (mapping code, pas de colonne).
    #[serde(default)]
    pub scope: Vec<FacetChoice>,
}

/// Entrée du catalogue des codes (`/api/codes`). `code` = slug (lien `/texte/{code}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodeCatalogueEntry {
    pub code: String,
    pub title: String,
    pub nature: String,
    pub jurisdiction: String,
    pub article_count: i64,
}

impl CodeCatalogueEntry {
    /// Familles de TÊTE du catalogue `/codes` (rendues SSR) : codes et
    /// constitutions. La longue traîne (lois, ordonnances, règlements UE —
    /// ~6 500 entrées) se charge à la demande (`scope=head` côté API) : la
    /// rendre dans le document initial pesait 6,2 Mo de DOM.
    pub fn is_head(&self) -> bool {
        let n = self.nature.to_ascii_uppercase();
        n.starts_with("CODE") || n == "ETAT_CIVIL" || n == "CONSTITUTION" || n == "LOI_CONSTIT"
    }
}

/// Réponse de `/api/codes` : liste des codes du corpus. `total` = nombre
/// d'entrées toutes natures — supérieur à `entries.len()` sur `scope=head`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodeCatalogueResponse {
    pub entries: Vec<CodeCatalogueEntry>,
    pub total: u64,
}

/// Une juridiction du catalogue hub (`/api/juridictions`, ADR 0253).
/// `code` = clé d'URL (`/juridiction/{code}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JurisdictionHubEntry {
    pub code: String,
    pub label: String,
    pub decision_count: i64,
}

/// Une famille de juridictions du catalogue (`CA` → « Cours d'appel »),
/// dans l'ordre éditorial des hubs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JurisdictionTypeGroup {
    pub jurisdiction_type: String,
    pub label: String,
    pub jurisdictions: Vec<JurisdictionHubEntry>,
}

/// Réponse de `/api/juridictions` : catalogue groupé par famille.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JurisdictionCatalogueResponse {
    pub groups: Vec<JurisdictionTypeGroup>,
}

/// Compteur d'une année d'un hub juridiction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JurisdictionYearCount {
    pub year: i32,
    pub count: i64,
}

/// Réponse de `/api/juridictions/{code}` : hub d'une juridiction — années
/// couvertes (plus récente en tête) et volume total.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JurisdictionHubResponse {
    pub code: String,
    pub label: String,
    pub jurisdiction_type: String,
    pub type_label: String,
    pub decision_count: i64,
    pub years: Vec<JurisdictionYearCount>,
}

/// Une décision d'une page hub juridiction×année : le lien crawlable
/// (`title` = ancre, `publicId` → `/decision/{publicId}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JurisdictionHubDecision {
    pub public_id: String,
    pub title: String,
    pub date_lecture: String,
}

/// Réponse de `/api/juridictions/{code}/{annee}` : page paginée des décisions
/// d'une année (`total` = décisions de l'année, pagination par `page`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JurisdictionYearResponse {
    pub code: String,
    pub label: String,
    pub year: i32,
    pub total: i64,
    pub page: u32,
    pub page_size: u32,
    pub decisions: Vec<JurisdictionHubDecision>,
}

/// Un fond du catalogue des normes (`/api/normes`, ADR 0255). `fond` = clé
/// d'URL (`/normes/{fond}`) ; le fond `codes` renvoie vers le catalogue
/// `/codes` existant (pas de hub année).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormFondEntry {
    pub fond: String,
    pub label: String,
    pub text_count: i64,
}

/// Réponse de `/api/normes` : catalogue des fonds, dans l'ordre éditorial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormCatalogueResponse {
    pub fonds: Vec<NormFondEntry>,
}

/// Compteur d'une année d'un hub fond. `year = None` = bucket « sans date »
/// (URL `/normes/{fond}/sans-date`), toujours en dernier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormYearCount {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    pub count: i64,
}

/// Réponse de `/api/normes/{fond}` : hub d'un fond — années couvertes (plus
/// récente en tête, bucket sans date en queue) et volume total.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormFondResponse {
    pub fond: String,
    pub label: String,
    pub text_count: i64,
    pub years: Vec<NormYearCount>,
}

/// Un texte d'une page hub fond×année : le lien crawlable (`title` = ancre,
/// `slug` → `/texte/{slug}`). `date` absente sur le bucket sans date.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormHubText {
    pub slug: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

/// Réponse de `/api/normes/{fond}/{annee}` : page paginée des textes d'une
/// année (`year = None` = bucket « sans date », `total` = textes du bucket).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormYearResponse {
    pub fond: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    pub total: i64,
    pub page: u32,
    pub page_size: u32,
    pub texts: Vec<NormHubText>,
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

/// Nœud du sommaire arborescent réel d'un texte (ADR 0207), aplati en ordre
/// de lecture — le front reconstruit l'imbrication par `depth` (1 = premier
/// niveau). `kind` = `section` (avec `cid`, l'ancre stable `#{cid}` et la clé
/// de la vue-lecture `/texte/{code}/section/{cid}`) ou `article` (avec `numKey`,
/// le lien `/texte/{code}/{numKey}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TocNode {
    pub kind: String,
    pub depth: i32,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_key: Option<String>,
    pub etat: String,
}

/// Taille maximale (en articles) d'une vue-lecture servie/matérialisée en
/// place (ADR 0214) : seuil de la vue-lecture intégrale des textes courts
/// côté API (`reading` du sommaire) et borne d'éligibilité d'une division à
/// l'accordéon de lecture côté front. Une seule règle, récursive — le texte
/// entier n'est que la division racine.
pub const INLINE_READING_MAX: usize = 150;

/// Réponse de `/api/texte/{code}/sommaire` : table des matières d'un code.
/// `tree` = l'arbre structurel réel daté (ADR 0207) quand le texte en a un ;
/// il prime sur `entries` (sommaire à plat par `titlePath`, servi seulement
/// quand `tree` est vide — textes sans structure ingérée). `reading` = vue-
/// lecture intégrale (corps joints) servie à la place des `entries` pour les
/// textes courts — un BOFiP de 22 § ou un décret se lit sur sa page, pas en
/// chips ; `tree` reste servi pour le rail « Plan du texte ».
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodeTocResponse {
    pub entries: Vec<TocEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tree: Vec<TocNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reading: Vec<LawSectionItem>,
}

/// Item de la vue-lecture d'une section (ADR 0207) : sous-arbre en ordre de
/// lecture. `kind` = `section` (intertitre, `cid` pour l'ancre) ou `article`
/// (corps `texte`/`nota` joints ; `numKey` → lien vers la page article).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawSectionItem {
    pub kind: String,
    pub depth: i32,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_key: Option<String>,
    pub etat: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texte: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nota: Option<String>,
}

/// Référence d'une division du texte : fil d'Ariane et navigation bloc
/// précédent / suivant de la vue-lecture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawSectionRef {
    pub cid: String,
    pub label: String,
}

/// Réponse de `/api/texte/{code}/section/{cid}` : vue-lecture d'une section
/// (ADR 0207) — les articles de la division rendus à la suite, intertitres
/// des sous-sections inclus. `code` = slug du texte, `title` = titre de la
/// section (porté par l'arête parente). `ancestors` = divisions englobantes
/// (de la racine au parent) ; `prev`/`next` = bloc précédent / suivant en
/// ordre de lecture (hors sous-arbre de la section).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawSectionResponse {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_title: Option<String>,
    pub cid: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ancestors: Vec<LawSectionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev: Option<LawSectionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<LawSectionRef>,
    pub items: Vec<LawSectionItem>,
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
    pub jurisdiction_type: JurisdictionType,
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
    /// Moteur interrogé (ADR 0251) : route le relancement de l'entrée
    /// (`/decisions?q=` vs `/textes?q=`).
    pub engine: SearchEngine,
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
    pub jurisdiction_type: JurisdictionType,
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

/// Préfixe un titre d'instrument par « du / de la / de l' » selon son premier
/// mot : élision devant voyelle (« de l'Ordonnance n° 2016-131 », « de
/// l'Arrêté »), féminins usuels des natures de textes (« de la Loi »), sinon
/// « du » (« du Code civil », « du Décret »). Sert les titres « Article N … »
/// (page /texte, hover card, MCP).
pub fn instrument_with_de(name: &str) -> String {
    let first = name.split_whitespace().next().unwrap_or("").to_lowercase();
    if first
        .chars()
        .next()
        .is_some_and(|c| "aeiouéèêë".contains(c))
    {
        return format!("de l'{name}");
    }
    const FEMININS: &[&str] = &[
        "loi",
        "constitution",
        "convention",
        "décision",
        "directive",
        "déclaration",
        "délibération",
        "charte",
    ];
    if FEMININS.contains(&first.as_str()) {
        format!("de la {name}")
    } else {
        format!("du {name}")
    }
}

/// Libellé FR d'un état DILA de version d'article (`legal_article.status`,
/// ADR 0178) : les états différés (`*_DIFF` = décidé mais pas encore effectif)
/// et les cas de chronique (mort-né, disjoint) deviennent lisibles à l'écran.
/// État inconnu ou vide → l'appelant retombe sur la valeur brute. Vit ici
/// (pas dans `lj-core`) : consommé par le front WASM comme par l'API — même
/// statut que [`instrument_with_de`].
pub fn article_status_label(status: &str) -> Option<&'static str> {
    Some(match status {
        "VIGUEUR" => "En vigueur",
        "MODIFIE" => "Modifié",
        "ABROGE" => "Abrogé",
        "PERIME" => "Périmé",
        "REMPLACE" => "Remplacé",
        "TRANSFERE" => "Transféré",
        "DEPLACE" => "Déplacé",
        "ANNULE" => "Annulé",
        "DENONCE" => "Dénoncé",
        "DISJOINT" => "Disjoint",
        "VIGUEUR_DIFF" => "Entrée en vigueur différée",
        "ABROGE_DIFF" => "Abrogation différée",
        "MODIFIE_MORT_NE" => "Modifié, jamais entré en vigueur",
        _ => return None,
    })
}

// ── Fiche entité (ADR 0189) ──────────────────────────────────────────────────

/// Fiche d'une entité du référentiel (`entity`, ADR 0179) : identité registre
/// + agrégats contentieux dérivés de `decision_party`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityPageResponse {
    pub header: EntityHeaderDto,
    pub stats: EntityStatsDto,
}

/// Identité registre. `namespace` = préfixe de l'uid (`siren` | `rna` |
/// `cnb` | `oacc`) ; `nature` = `physique` | `morale_privee` |
/// `morale_publique`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityHeaderDto {
    /// Uid complet namespacé (ex. `siren:552043002`).
    pub uid: String,
    pub namespace: String,
    pub nature: String,
    /// Dénomination courante.
    pub denomination: String,
    pub sigle: Option<String>,
    pub forme: Option<String>,
    pub active: bool,
    /// Dénominations datées (la courante incluse), ordre chronologique.
    pub denominations: Vec<EntityDenominationDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityDenominationDto {
    pub denomination: String,
    /// Dates ISO `YYYY-MM-DD` ; `None` = borne ouverte.
    pub date_debut: Option<String>,
    pub date_fin: Option<String>,
}

/// Agrégats contentieux d'une entité sur le corpus de décisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityStatsDto {
    pub decision_count: i64,
    pub as_applicant: i64,
    pub as_defendant: i64,
    /// Décisions par année, ordre chronologique.
    pub by_year: Vec<EntityYearCountDto>,
    /// Décisions par juridiction, décroissant.
    pub by_jurisdiction: Vec<EntityKeyCountDto>,
    /// Conseils (avocats/cabinets) observés aux côtés de l'entité, décroissant.
    pub top_counsel: Vec<EntityCounselDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityYearCountDto {
    pub year: i32,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityKeyCountDto {
    /// Clé machine (ex. code juridiction).
    pub key: String,
    /// Libellé d'affichage.
    pub label: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityCounselDto {
    /// Uid registre du conseil si lui-même résolu (`cnb:…`, `oacc:…`).
    pub uid: Option<String>,
    pub name: String,
    pub count: i64,
}

/// Décisions citant l'entité, paginées, plus récentes d'abord. Les items
/// portent les mêmes hits que la recherche (rendu unifié `ResultCard`,
/// précédent : jurisprudence des pages article) plus le rôle de l'entité.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityDecisionsResponse {
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
    pub items: Vec<EntityDecisionHitDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityDecisionHitDto {
    pub hit: SearchHit,
    /// Côté de l'entité dans cette décision (`applicant` | `defendant`).
    pub side: Option<String>,
    /// Qualité de l'entité (`party` | `law_firm` | `counsel_name`).
    pub quality: String,
}

// ── Autocomplétion (ADR 0216) ─────────────────────────────────────────────────

/// Suggestions d'autocomplétion (`GET /suggest`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SuggestResponse {
    /// Nombre de mots de fin de query que chaque suggestion remplace (contexte
    /// re-suggéré inclus — le mot en cours de frappe compte pour un).
    pub matched_tokens: u32,
    pub suggestions: Vec<String>,
}

// ── Annuaire des entités (ADR 0192) ──────────────────────────────────────────

/// Résultat de recherche d'entités (annuaire — registre complet, ADR 0239).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntitySearchResponse {
    pub items: Vec<EntityDirectoryItemDto>,
}

/// Listing annuaire paginé d'une catégorie — registre complet, trié par
/// contentieux décroissant (ADR 0239).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityDirectoryResponse {
    /// Lignes paginables du filtre courant (registre entier de la catégorie,
    /// ou sous-ensemble barreau).
    pub total: i64,
    /// Dont entités avec ≥ 1 décision liée (même filtre).
    pub contentieux: i64,
    pub page: i64,
    pub page_size: i64,
    pub items: Vec<EntityDirectoryItemDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityDirectoryItemDto {
    /// Uid complet namespacé (ex. `siren:552043002`) → fiche `/entite/{ns}/{id}`.
    pub uid: String,
    pub namespace: String,
    pub denomination: String,
    pub nature: String,
    pub forme: Option<String>,
    pub active: bool,
    /// Slug barreau (avocats `cnb:` uniquement, extrait de l'uid).
    pub barreau: Option<String>,
    /// Nombre de décisions liées (source du tri).
    pub decision_count: i64,
}

/// Compteurs d'une catégorie de l'annuaire : total du registre chargé et
/// entités avec au moins une décision liée (ADR 0233).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnuaireCategorieStatsDto {
    pub registre: i64,
    pub contentieux: i64,
}

/// Compteurs de l'annuaire par catégorie (page d'accueil annuaire).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnuaireStatsResponse {
    pub entreprises: AnnuaireCategorieStatsDto,
    pub personnes_publiques: AnnuaireCategorieStatsDto,
    pub associations: AnnuaireCategorieStatsDto,
    pub avocats: AnnuaireCategorieStatsDto,
    pub cabinets: AnnuaireCategorieStatsDto,
}

/// Encart « Parties » d'une décision : acteurs extraits, liés au registre
/// quand résolus.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionPartiesResponse {
    pub parties: Vec<DecisionPartyDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionPartyDto {
    pub quality: String,
    pub side: Option<String>,
    /// Verbatim extrait de la décision.
    pub value: String,
    pub nature: Option<String>,
    /// Slug officiel CNB du barreau (counsel uniquement).
    pub barreau: Option<String>,
    /// Uid registre si résolu → fiche `/entite/{ns}/{id}`.
    pub entity_uid: Option<String>,
}

// ── Fiche entité — volet registre servi par APIs externes (ADR 0199) ─────────

/// Volet registre d'une fiche entité, servi à l'affichage par les APIs
/// publiques (recherche-entreprises, BODACC/JOAFE Opendatasoft) — aucun stock
/// local. Chaque section absente (API indisponible, entité hors périmètre)
/// est simplement `None`/vide : le rendu dégrade sans erreur.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityRegistreResponse {
    /// Identité enrichie + dirigeants + finances (entreprises `siren:` seulement).
    pub entreprise: Option<RegistreEntrepriseDto>,
    /// Annonces officielles, plus récentes d'abord (BODACC pour `siren:`,
    /// JOAFE pour `rna:`).
    pub annonces: Vec<RegistreAnnonceDto>,
    /// Nombre total d'annonces au registre (la liste est tronquée).
    pub annonces_total: i64,
    /// Liens sortants (annuaire-entreprises, documents INPI…).
    pub liens: Vec<RegistreLienDto>,
}

/// Identité registre d'une entreprise (recherche-entreprises.api.gouv.fr —
/// données SIRENE + RNE).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistreEntrepriseDto {
    pub siege_adresse: Option<String>,
    /// Code NAF/APE de l'activité principale (ex. `62.01Z`).
    pub activite_naf: Option<String>,
    pub date_creation: Option<String>,
    /// Libellé de la tranche d'effectif salarié (ex. « 20 à 49 salariés »).
    pub effectif: Option<String>,
    pub dirigeants: Vec<RegistreDirigeantDto>,
    /// Années comptables publiées, plus récente d'abord.
    pub finances: Vec<RegistreFinanceDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistreDirigeantDto {
    /// Nom affichable (personne physique « Prénom NOM », morale : dénomination).
    pub nom: String,
    pub qualite: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistreFinanceDto {
    pub annee: String,
    pub chiffre_affaires: Option<i64>,
    pub resultat_net: Option<i64>,
}

/// Une annonce officielle (BODACC ou JOAFE).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistreAnnonceDto {
    /// Date de parution ISO.
    pub date: Option<String>,
    /// Famille lisible (« Dépôts des comptes », « Création »…).
    pub famille: String,
    /// PDF officiel hébergé par la DILA (JOAFE uniquement) — lien direct,
    /// jamais proxifié.
    pub url_pdf: Option<String>,
}

/// Lien sortant vers la source officielle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistreLienDto {
    pub label: String,
    pub url: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn instrument_with_de_contracte_selon_le_premier_mot() {
        // Spec utilisateur : « Article 2 du Ordonnance » est fautif.
        assert_eq!(
            instrument_with_de("Ordonnance n° 2016-131 du 10 février 2016"),
            "de l'Ordonnance n° 2016-131 du 10 février 2016"
        );
        assert_eq!(instrument_with_de("Code civil"), "du Code civil");
        assert_eq!(
            instrument_with_de("Loi n° 2018-287 du 20 avril 2018"),
            "de la Loi n° 2018-287 du 20 avril 2018"
        );
        assert_eq!(instrument_with_de("Décret n° 94-46"), "du Décret n° 94-46");
        assert_eq!(
            instrument_with_de("Arrêté du 12 mai"),
            "de l'Arrêté du 12 mai"
        );
        assert_eq!(instrument_with_de("Constitution"), "de la Constitution");
    }

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
        assert!(req.jurisdiction_type.is_none());

        // Les clés JSON sont en camelCase (= to_camel côté Pydantic).
        let body = json!({
            "query": "x",
            "jurisdictionType": ["CA", "TJ"],
            "dateFrom": "2020-01-01",
            "aiMode": true
        });
        let req: SearchRequest = serde_json::from_value(body).unwrap();
        assert_eq!(
            req.jurisdiction_type,
            Some(vec![JurisdictionType::Ca, JurisdictionType::Tj])
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
            jurisdiction_type: JurisdictionType::Ce,
            jurisdiction_code: Some("ce".into()),
            jurisdiction_name: Some("Conseil d'État".into()),
            title_html: "<b>x</b>".into(),
            seat: None,
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
            procedure: None,
            office: None,
            legal_domain: None,
            chamber: None,
            publication: None,
            publication_codes: vec!["B".into()],
            chars: Some(4200),
            summary: None,
        };
        let v = serde_json::to_value(&hit).unwrap();
        assert_eq!(v["jurisdictionType"], "CE");
        assert_eq!(v["titleHtml"], "<b>x</b>");
        assert_eq!(v["bestChunk"]["chunkIndex"], 0);
        // Tags référentiels : paire clé + libellé résolue par l'API (ADR 0146).
        assert_eq!(v["solution"]["key"], "REJET");
        assert_eq!(v["solution"]["label"], "Rejet");
        // None ⇒ champ omis (skip_serializing_if).
        assert!(v.get("summary").is_none());
        assert!(v.get("procedure").is_none());
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
            parent: Some("jurisdiction_type:TJ".into()),
        };
        let v = serde_json::to_value(&child).unwrap();
        assert_eq!(v["parent"], "jurisdiction_type:TJ");
        assert!(serde_json::to_value(&flat).unwrap().get("parent").is_none());
    }

    #[test]
    fn legi_article_response_serializes_camel_case() {
        let art = LawArticleResponse {
            upcoming_version_date: None,
            breadcrumb: Vec::new(),
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
            texte_spans: vec![],
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
                upcoming: false,
                legiarti: "LEGIARTI000006832947".into(),
                etat: "MODIFIE".into(),
                date_debut: "1804-03-15".into(),
                date_fin: Some("1992-05-14".into()),
            }],
            context: Vec::new(),
            modifications: Vec::new(),
            modified_by: Vec::new(),
            cites: Vec::new(),
            cited_by: Vec::new(),
            commentaires: Vec::new(),
            travaux_parlementaires: Vec::new(),
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
            jurisdiction_type: JurisdictionType::Cc,
            date_lecture: Some("2024-01-10".into()),
            significance: Significance::Majeure,
            summary: None,
        };
        let v = serde_json::to_value(&hit).unwrap();
        assert_eq!(v["jurisdictionType"], "CC");
        assert_eq!(v["dateLecture"], "2024-01-10");
        assert_eq!(v["significance"], "MAJEURE");
    }
}
