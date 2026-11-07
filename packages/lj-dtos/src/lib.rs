//! `lj-dtos` — contrats api ↔ web.
//!
//! Tous les structs sont `#[serde(rename_all = "camelCase")]`. `lj-api` et
//! `lj-web` importent ces types, jamais de redéfinition côté consommateur. Les
//! enums de schéma (`JurisdictionLevel`…) vivent dans `schema` (ce crate).
//!
//! Les corps de requête sont annotés `#[serde(deny_unknown_fields)]`. Les
//! timestamps (profil/activité) sont portés en `String` (RFC 3339) : `lj-dtos`
//! n'a pas `chrono` en dépendance et reste un pur crate de contrats sérialisés
//! — la conversion `NaiveDate`/`DateTime` vit dans `lj-store`/`lj-api`.
//!
//! Shared by the backend and the wasm front, so its only dependencies are
//! serde and the codes of `lj-vocab`, never `lj-core`. The taxonomy
//! (`schema`) lives here, its single source.

use std::collections::BTreeMap;

use lj_vocab::{ANNONCE_FOND_BODACC, ANNONCE_FOND_JOAFE, ANNONCE_FOND_MARCHE};
use serde::{Deserialize, Serialize};

pub mod schema;

pub use schema::{
    abbreviate_jurisdiction_name, Domain, JurisdictionLevel, JurisdictionType, Solution,
};

// ── Enums propres à l'API (canal, mode de recherche, tri) ────────────────────

/// Canal d'origine d'une activité utilisateur : `web` = le front (`/internal/*`) ;
/// `api` = l'API publique ouverte par clé (`/api/*`) ; `mcp` = endpoint IA.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActivitySource {
    Web,
    Api,
    Mcp,
}

/// Engine a search queries, orthogonal to the `ActivitySource` channel:
/// `decisions` (case law), `textes` (legal texts), `annonces` (the running
/// events pot) or `entites` (the annuaire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchEngine {
    Decisions,
    Textes,
    Annonces,
    Entites,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchContext {
    User,
    Teaser,
}

pub const INTERNAL_ORIGIN_HEADER: &str = "x-lj-origin";
pub const INTERNAL_ORIGIN_COOKIE: &str = "__Host-lj_origin";

/// Mode de recherche demandé. `semantic` force le hybride.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    #[default]
    Auto,
    Lexical,
    Semantic,
    Reference,
}

/// A part of a decision a search runs in: the court's headnote (sommaire,
/// analyse), the judgment, its moyens annexés.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionPart {
    Headnote,
    Judgment,
    Annexes,
}

impl DecisionPart {
    pub const ALL: [DecisionPart; 3] = [
        DecisionPart::Headnote,
        DecisionPart::Judgment,
        DecisionPart::Annexes,
    ];
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
    /// The query designates a decision instead of describing one: exact key and
    /// title only, no embedding and no ANN.
    Reference,
    /// Requête sans texte : rien n'est classé, le fonds filtré est listé par
    /// date, et total comme facettes portent sur lui en entier.
    Browse,
}

// ── Health ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HealthResponse {
    /// Toujours `"ok"`.
    pub status: String,
    pub version: String,
}

// ── Stats corpus ───────────────────────────────────────────────────────────

/// Compteurs globaux du corpus pour la page d'accueil. Comptes exacts servis
/// depuis un cache process-local (TTL long).
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

/// Cap on `query`, in Unicode characters. The HTTP route, the MCP handlers, the
/// `maxLength` the MCP schema announces and the front must all agree on it.
pub const QUERY_MAX_CHARS: usize = 512;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_type: Option<Vec<JurisdictionType>>,
    /// Solutions du dispositif (référentiel `solution:*`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<Vec<Solution>>,
    /// Domaines de place (référentiel `legal_domain:*`) — une racine
    /// sélectionnée matche elle-même + toutes ses feuilles (expansion API).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_domain: Option<Vec<Domain>>,
    /// Codes du référentiel `jurisdiction` (`tj76351`, `ca_paris`, `cc`…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_code: Option<Vec<String>>,
    /// Seat paths (`FR.CA.PARIS.P5.C3`), from the legal order down to the room.
    /// A path matches itself and everything under it, and this key says on its
    /// own what `jurisdictionType` and `jurisdictionCode` say at its second and
    /// third depths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<Vec<String>>,
    /// Formation paths (`FR.TJ.JEX`, `FR.CC.RESTREINTE`): under the court type, the
    /// composition of the bench or the judge sitting alone by function. A path
    /// matches itself and everything under it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formation: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_instrument: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_article: Option<Vec<String>>,
    /// Valeurs de publication (suffixes d'uid `publication:*`). Appartenance
    /// multiple : une décision répond à chaque valeur dont elle porte un code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<Vec<String>>,
    /// Date ISO `YYYY-MM-DD`. Cap : `1678-01-01` ↔ `2262-01-01`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_to: Option<String>,
    /// The parts of a decision the search runs in, every part when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_in: Option<Vec<DecisionPart>>,
    #[serde(default)]
    pub mode: SearchMode,
    #[serde(default = "default_sort")]
    pub sort: SortOrder,
    #[serde(default = "default_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
    /// Mode IA : active le reranker LLM. No-op en lexical.
    #[serde(default)]
    pub ai_mode: bool,
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

/// Un choix de facette, servi avec son libellé depuis le référentiel DB.
/// `parent` porte la hiérarchie (arbres juridiction/domaine, 2 niveaux) :
/// `None` = racine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FacetChoice {
    pub value: String,
    pub label: String,
    pub count: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Ce que la valeur recouvre quand elle en regroupe plusieurs, rendu au
    /// survol : « Couvre les codes source C, D et Z. »
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Paire clé (code référentiel) + libellé FR résolue par l'API depuis
/// `facet_value` — le front rend le label sans table compilée.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FacetTag {
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegalInstrumentFacet {
    /// Token de filtre = `ref_text_uid` (référence catalogue).
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

/// Search facets, in the bar's order: « Juridiction », « Formation », « Domaine »,
/// « Solution », « Publication », « Date », « Textes cités ».
///
/// `juridiction` : niveau 1 = racines à valeur uid complet
/// (`jurisdiction_type:TJ`) ; niveau 2 = codes `jurisdiction` (`tj76351`,
/// `parent` = uid racine). Les autres facettes portent le suffixe d'uid
/// (`REJET`, `JEX`, `CIVIL_DROIT_LOCATIF`…).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchFacets {
    #[serde(default)]
    pub seat: Vec<FacetChoice>,
    /// Formation paths under their court type, `parent` carrying the hierarchy as
    /// in `seat`.
    #[serde(default)]
    pub formation: Vec<FacetChoice>,
    #[serde(default)]
    pub legal_domain: Vec<FacetChoice>,
    #[serde(default)]
    pub solution: Vec<FacetChoice>,
    #[serde(default)]
    pub publication: Vec<FacetChoice>,
    #[serde(default)]
    pub date_lecture_year: Vec<FacetChoice>,
    #[serde(default)]
    pub legal_instrument: Vec<LegalInstrumentFacet>,
    /// Most cited articles across texts, the pure-procedure bank dropped.
    /// `value` is the composite key `legalArticle` takes.
    #[serde(default)]
    pub legal_article_top: Vec<FacetChoice>,
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
    /// sous le titre — hors du `title_html` qui reste sur une ligne.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<String>,
    pub score: f64,
    pub best_chunk: BestChunk,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    /// Tags référentiels résolus (clé + libellé).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<FacetTag>,
    /// The « Formation » paths, with their labels: the composition first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formation: Vec<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_domain: Option<FacetTag>,
    /// The seat node the decision is filed under: the deepest path, and the
    /// label of its last segment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat_path: Option<FacetTag>,
    /// Codes de publication source (`b`, `A`…), dont le front tire le libellé et
    /// le badge. Appartenance multiple : plusieurs codes possibles.
    #[serde(default)]
    pub publication_codes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chars: Option<i64>,
    /// Résumé v4 embarqué en mode IA uniquement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Where the court's own summary sits inside `summary`, pasted between an
    /// opening and sometimes an outcome the model wrote: codepoint offsets, half
    /// open. Absent when the model wrote it all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_quote: Option<std::ops::Range<usize>>,
    /// Where the body of `summary` sits, after its opening, in codepoint offsets,
    /// half open: the court's words in a collage. Absent for an opening alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_body: Option<std::ops::Range<usize>>,
    /// Page de la décision chez son éditeur officiel, dérivée de la provenance
    /// autoritaire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub official_url: Option<String>,
    /// The rarest cited articles, under the slug of their text: the key is a
    /// `legalInstrument` value, the entries are `legalArticle` values, and the
    /// pair composes `/norm/{juris}/{slug}/{key}`. Procedural boilerplate dropped.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cited_articles: BTreeMap<String, Vec<String>>,
    /// The number the query bit on, when it is not the one the title names. A
    /// decision can carry many dockets and is titled by its head, so a
    /// search on any other answers with a card naming a different number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_docket: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchResponse {
    pub query: String,
    /// Absents de la recherche textuelle, comptée par `/internal/search/decompte`.
    /// Présents en parcours et pour un appelant qui ne reviendra pas les
    /// chercher (MCP).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    pub hits: Vec<SearchHit>,
    pub query_mode: QueryMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facets: Option<SearchFacets>,
    #[serde(default)]
    pub all_hit_paths: Vec<String>,
    /// Le tri IA a effectivement réordonné le haut de la liste. Faux dès qu'il
    /// a renoncé, et il renonce en silence : lexical, tri par date, budget épuisé.
    #[serde(default)]
    pub reranked: bool,
    /// Decisions the query designates by number or by name, served apart from
    /// the list in a content mode, three at most. Empty in `reference` mode,
    /// where the list is the designation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned: Vec<PinnedDecision>,
    /// More decisions carry the designation than `pinned` shows: the card is a
    /// sample, and `mode=reference` on the same query is the whole list.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned_more: bool,
}

/// A designated decision: its title is the identity card, its id the link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PinnedDecision {
    pub id: String,
    pub jurisdiction_type: JurisdictionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_code: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<FacetTag>,
    /// The « Formation » paths, with their labels: the composition first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formation: Vec<FacetTag>,
}

/// Réponse de `/internal/search/decompte` : le total exact et les facettes de la
/// récupération qui a servi la liste, pour les mêmes paramètres de requête.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchCountResponse {
    pub total: i64,
    pub facets: SearchFacets,
}

/// Answer of `/internal/search/instruments`: the catalogue texts and articles
/// the typed text names, counted on the current search. Same shape as the
/// « Textes cités » facet, which merges them without telling them apart; a zero
/// here is a catalogue value the search does not meet, not one fallen out of
/// the head of the facet.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegalInstrumentSuggestions {
    #[serde(default)]
    pub legal_instrument: Vec<LegalInstrumentFacet>,
}

// ── Decision detail ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegalReference {
    pub instrument: String,
    /// Slug du `legal_text` résolu (FK de citation) — présent quand la citation
    /// est ancrée au catalogue. Le front bâtit `/norm/{juris}/{slug}/{numKey}`
    /// directement, sans re-slugifier. `None` ⇒ rendu brut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    /// La juridiction qui édicte le texte cité : premier segment de son URL, et
    /// le signe qu'une décision applique du droit étranger.
    #[serde(default)]
    pub jurisdiction: String,
    pub articles: Vec<LegalRefArticle>,
}

/// Un article cité : `num` = libellé affiché (brut source) ; `numKey` = clé
/// canonique résolue (`legal_citation.ref_num_key`) pour le lien
/// `/norm/{juris}/{slug}/{numKey}`, vide si l'article n'a pas été ancré au catalogue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegalRefArticle {
    pub num: String,
    #[serde(default)]
    pub num_key: String,
}

/// Cible d'une mention de citation : un article (ou un texte) résolu. `href`
/// pointe l'article (`/norm/{juris}/{slug}/{numKey}`) ; `None` ⇒ citation non résolue.
/// `label` = libellé de la cible (titre/clé canonique).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CitationTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    pub label: String,
}

/// Une mention cliquable d'une citation dans le corps d'un paragraphe.
/// `start`/`end` sont des offsets CODEPOINTS locaux au paragraphe rendu,
/// demi-ouverts `[start, end)`. `targets` porte une ou plusieurs cibles : une
/// citation multi-articles (« articles 1382, 1383 et 1384 du code civil »)
/// partage un seul span de texte mais vise N articles → le front rend un lien
/// simple pour 1 cible, un menu déroulant pour ≥2. Sur chevauchement de spans,
/// on fusionne en l'enveloppe et on unit les cibles (aucune n'est droppée).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CitationSpan {
    pub start: usize,
    pub end: usize,
    pub targets: Vec<CitationTarget>,
}

/// Bloc de corps structuré d'une décision (renderer pur). `kind` peut se
/// répéter ; `anchor` est l'ancre DOM unique ; `label` = titre affiché.
///
/// `paragraphSpans` : aligné index-à-index sur `paragraphs`, porte par
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
    /// Nom sous lequel la décision est citée (« Danthony »), quand le
    /// référentiel activé le porte. Absent du corps de la décision, la
    /// pseudonymisation l'ayant retiré.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_name: Option<String>,
    pub paragraphs: Vec<String>,
    /// Mentions de citation cliquables alignées sur `paragraphs`, pour le rendu
    /// de repli sans sections autoritatives. Vide / absent si aucune.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paragraph_spans: Vec<Vec<CitationSpan>>,
    /// Sommaire/corps structuré autoritatif (None ⇒ heuristique front).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sections: Option<Vec<DecisionSection>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Where the court's own summary sits inside `summary`, pasted between an
    /// opening and sometimes an outcome the model wrote: codepoint offsets, half
    /// open. Absent when the model wrote it all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_quote: Option<std::ops::Range<usize>>,
    /// Where the body of `summary` sits, after its opening, in codepoint offsets,
    /// half open: the court's words in a collage. Absent for an opening alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_body: Option<std::ops::Range<usize>>,
    /// Code de cour précis (`jurisdiction:*`, ex. `tj_paris`) — token du
    /// filtre `jurisdiction_code`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_name: Option<String>,
    /// The place the court reads the law from, which its links carry; absent
    /// for a court without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub court_place: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    /// Tags référentiels résolus (clé + libellé).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<FacetTag>,
    /// The « Formation » paths, with their labels: the composition first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formation: Vec<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_domain: Option<FacetTag>,
    /// Codes de publication source (`b`, `A`…), dont le front tire le libellé et
    /// le badge. Appartenance multiple : plusieurs codes possibles.
    #[serde(default)]
    pub publication_codes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_audience: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    /// Siège composé depuis les axes structurés : position de chambre qualifiée
    /// par le type de formation ou l'office — « pôle 5 — 3e chambre (formation à
    /// trois) ». Jamais une chaîne source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<String>,
    /// The seat node the decision is filed under: the deepest path, and the
    /// label of its last segment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat_path: Option<FacetTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_references: Option<Vec<LegalReference>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_xml: Option<DecisionSourceXml>,
    /// Mots-clés de matière (themes Judilibre), du général au spécifique.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub themes: Vec<String>,
    /// Libellé brut de la nomenclature des affaires civiles (nac Judilibre).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nac: Option<String>,
    /// ECLI (identifiant européen stable et citable) si présent. Provenance/audit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ecli: Option<String>,
    /// Source de provenance autoritaire (`decision_sources.source` : `judilibre`,
    /// `opendata`, `dila-jade`, `cnda`…). Provenance/audit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Page de la décision chez son éditeur officiel, dérivée de la provenance
    /// autoritaire. Ce que l'on cite quand notre permalien ne suffit pas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub official_url: Option<String>,
    /// Chronologie de l'affaire : décisions chaînées par les liens
    /// appel/pourvoi/renvoi résolus, décision courante incluse, de la plus récente
    /// à la plus ancienne. Vide si la décision n'est pas chaînée.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chronology: Vec<ChronologyEntry>,
    /// Institutional commentary: links to the rapporteur public's conclusions and
    /// to the court's related documents (reports, opinions, press notes). Not
    /// searchable, an enrichment of the record, rendered as a collapsed accordion
    /// at the foot of the page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commentaires: Vec<Commentaire>,
    /// Decisions whose text cites this one or whose Conseil d'Etat analysis
    /// refers to it, authority first, less the steps of the chronology, which
    /// have their own block.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub citantes: Vec<CitingDecisionHit>,
    /// Total des citantes, servi quand il dépasse la liste rendue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citantes_total: Option<i64>,
}

/// Institutional commentary on a decision or an article. Three forms:
/// `summary`, the court's own summary of its ruling, carried whole in `body`;
/// `conclusions`, whose existence alone is stored and whose link is composed
/// towards ArianeWeb; and `note`, a linked document (report or opinion of the
/// Cour, doctrinal note) titled towards its publisher. The page renders only the
/// two link forms, the summary being already the head of the page.
// Read back from a stored blob whose writer ships apart from this reader, so a
// new field reaches the base before the binary that knows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commentaire {
    /// `summary` | `conclusions` | `note`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Date ISO du document commenté (date de lecture).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// Libellé du lien (`note`) : type de document Cass, titre de la note…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Éditeur/source du lien (`Cour de cassation`, `GISTI`…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    /// Edition reference, as the source carries it: `407-422` as readily as
    /// `act. 843`. Laid out by [`Commentaire::reference`], never rebuilt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<String>,
    /// External link: the line renders as an outgoing one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The text itself, on a `summary` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

impl Commentaire {
    /// The edition reference laid out for a reader, `vol. 134, n° 2, p. 407-422`,
    /// `None` when the source gave none of the three.
    ///
    /// A label the source already wrote is not written twice, and `p.` is only
    /// posed on a number or a range: HAL's other `page_s`, `act. 843`,
    /// `1 et suivantes` or `250 s.`, carry their own form.
    pub fn reference(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(v) = strip_label(&self.volume, &["volume", "vol.", "vol", "tome", "t."]) {
            parts.push(format!("vol. {v}"));
        }
        if let Some(i) = strip_label(&self.issue, &["numéro", "n°", "no", "n."]) {
            parts.push(format!("n° {i}"));
        }
        if let Some(p) = strip_label(&self.pages, &["pages", "page", "pp.", "p."]) {
            parts.push(if is_page_range(&p) {
                format!("p. {p}")
            } else {
                p
            });
        }
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

/// Drops a label the source already wrote, and only when a number follows it:
/// `n° 24` gives `24` where `nouvelle série` keeps its `no`.
fn strip_label(field: &Option<String>, labels: &[&str]) -> Option<String> {
    let s = field.as_ref()?.trim();
    for l in labels {
        let Some(head) = s.get(..l.len()) else {
            continue;
        };
        if !head.eq_ignore_ascii_case(l) {
            continue;
        }
        let rest = s[l.len()..].trim_start();
        if rest.starts_with(|c: char| c.is_ascii_digit()) {
            return Some(rest.to_string());
        }
    }
    (!s.is_empty()).then(|| s.to_string())
}

fn is_page_range(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_digit() || c == '-' || c == '\u{2013}' || c == ' ')
        && s.chars().any(|c| c.is_ascii_digit())
}

/// Étape de la chronologie d'une affaire : une décision de la chaîne
/// appel/pourvoi/renvoi, identifiée par sa juridiction et sa date.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChronologyEntry {
    /// Absent when the attacked decision is not in the corpus: the step is then
    /// known only from the text that attacks it, and renders unclickable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Set with `id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_type: Option<JurisdictionType>,
    /// Set with `id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_code: Option<String>,
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
/// inconnu est renvoyé tel quel, par transparence d'audit. Pas un vocabulaire
/// de facette : c'est de la provenance, hors référentiel `facet_value`.
pub fn source_label(code: &str) -> &str {
    match code {
        "judilibre" => "Judilibre (Cour de cassation)",
        "opendata" => "Open Data des juridictions administratives",
        "dila-jade" => "DILA — JADE (juridictions administratives)",
        "dila-constit" => "DILA — Conseil constitutionnel",
        "dila-cass" => "DILA — Cour de cassation",
        "dila-inca" => "DILA, inédits de la Cour de cassation",
        "dila-capp" => "DILA — cours d'appel",
        "cedh" => "CEDH — HUDOC",
        "cjue" => "CJUE — EUR-Lex",
        "cnda" => "Cour nationale du droit d'asile",
        "dila-cnil" => "DILA — CNIL (délibérations)",
        "ddd" => "Défenseur des droits",
        "adlc" => "Autorité de la concurrence",
        "comeu" => "Commission européenne",
        "contribution" => "Versée par un utilisateur",
        other => other,
    }
}

/// Noms de mois FR longs (rendu `toLocaleDateString("fr-FR", { month: "long" })`).
const MONTHS_FR: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

/// Date ISO `YYYY-MM-DD` en graphie française (« 3 mai 2018 ») ; la valeur brute
/// si ce n'est pas une date ISO. Formateur unique partagé par la page décision,
/// l'export PDF et le nom de fichier d'export.
pub fn format_date_fr(value: &str) -> String {
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() != 3 {
        return value.to_string();
    }
    match (
        parts[0].parse::<i32>(),
        parts[1].parse::<u32>(),
        parts[2].parse::<u32>(),
    ) {
        (Ok(y), Ok(m), Ok(d)) if (1..=12).contains(&m) && (1..=31).contains(&d) => {
            format!("{d} {} {y}", MONTHS_FR[(m - 1) as usize])
        }
        _ => value.to_string(),
    }
}

/// Origine canonique du site (permaliens d'audit, URLs absolues). Source unique
/// partagée backend ↔ front : `lj-web` aligne sa `CANONICAL_BASE` dessus.
pub const SITE_BASE: &str = "https://librejustice.fr";

/// The page of a decision, under the jurisdiction segment of its court.
pub fn decision_path(
    jurisdiction_type: JurisdictionType,
    jurisdiction_code: Option<&str>,
    id: &str,
) -> String {
    format!(
        "/decision/{}/{id}",
        jurisdiction_type.url_segment(jurisdiction_code)
    )
}

/// The public id a decision path ends with.
pub fn decision_id_of_path(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The court's words of a summary: the whole of one pasted, the `quote` alone of a collage.
pub fn court_words(summary: &str, quote: &std::ops::Range<usize>) -> String {
    if quote.start == 0 {
        return summary.to_owned();
    }
    summary
        .chars()
        .skip(quote.start)
        .take(quote.len())
        .collect()
}

/// Permalien canonique LibreJustice d'une décision.
pub fn decision_permalink(
    jurisdiction_type: JurisdictionType,
    jurisdiction_code: Option<&str>,
    id: &str,
) -> String {
    format!(
        "{SITE_BASE}{}",
        decision_path(jurisdiction_type, jurisdiction_code, id)
    )
}

/// Lignes de provenance/audit d'une décision — `(libellé, valeur)` dans l'ordre
/// d'affichage : source d'origine, ECLI, permalien LibreJustice. Builder pur
/// partagé par le bloc « Source » du web, l'export PDF et l'export DOCX —
/// contenu unique, rendu propre à chacun.
pub fn provenance_rows(detail: &DecisionDetail) -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    if let Some(src) = detail.source.as_deref().filter(|s| !s.is_empty()) {
        rows.push(("Source", source_label(src).to_string()));
    }
    if let Some(ecli) = detail.ecli.as_deref().filter(|s| !s.is_empty()) {
        rows.push(("ECLI", ecli.to_string()));
    }
    if let Some(url) = detail.official_url.as_deref().filter(|s| !s.is_empty()) {
        rows.push(("Page officielle", url.to_string()));
    }
    rows.push((
        "Permalien",
        decision_permalink(
            detail.jurisdiction_type,
            detail.jurisdiction_code.as_deref(),
            &detail.id,
        ),
    ));
    rows
}

/// Décision proche — carte des volets « similaires » de la page décision.
/// L'ordre dans chaque volet est le rang de fusion des signaux (citations
/// directes, articles partagés IDF, KNN) : les jambes n'ont pas d'échelle de
/// score commune, aucun score n'est porté.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SimilarDecisionHit {
    pub id: String,
    pub jurisdiction_type: JurisdictionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    /// Tags référentiels résolus (clé + libellé).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<FacetTag>,
    /// The « Formation » paths, with their labels: the composition first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formation: Vec<FacetTag>,
    #[serde(default)]
    pub publication_codes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Rail « similaires » en deux volets : décisions de principe (référence,
/// similaires sur le fond — portée notable, graphe de citations + KNN
/// pré-filtré) et décisions d'espèce (similaires en fait — KNN générique).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SimilarDecisionsResponse {
    pub decision_id: String,
    pub principe: Vec<SimilarDecisionHit>,
    pub espece: Vec<SimilarDecisionHit>,
    /// Vrai quand la source ne tranche aucune question de droit : les volets
    /// muets le sont par décision, pas faute de candidat assez proche.
    #[serde(default)]
    pub abstained: bool,
}

/// Prévisualisation légère d'une décision (hover card des liens de
/// jurisprudence) : identité + solution + codes de publication + résumé —
/// jamais le corps.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionPreview {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<FacetTag>,
    /// The « Formation » paths, with their labels: the composition first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formation: Vec<FacetTag>,
    #[serde(default)]
    pub publication_codes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

// ── Référentiel LEGI versionné ──────────────────────────────────────────────

/// Une version d'article dans la timeline LEGI. `dateDebut`/`dateFin` ISO
/// `YYYY-MM-DD` ; `dateFin` absente ⇒ pas de fin (sentinelle normalisée NULL
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
    /// vit côté serveur) : badge « à venir » sur la frise.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub upcoming: bool,
}

/// The place a reader resolves on when the URL names none.
pub const DEFAULT_READER_PLACE: &str = "FR-M";

/// The most places a text search reads from at once: each one the day has not
/// seen yet costs a read of the coverage.
pub const MAX_READER_PLACES: usize = 20;

/// A place of the vocabulary as the web and the connector write it (`PF`), with
/// its French label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlaceRef {
    pub uid: String,
    pub label: String,
}

/// How an article answers the reader's place and date: the compiled states
/// first, then the rule of reception of the reader's place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicabilityStatus {
    /// A state covers the reader: the served version is the one it carries.
    Applicable,
    /// The article does not apply where the reader stands: no rule brings its
    /// journal there, a rule excludes its matter, or a rule brings it and the
    /// source sets the article aside. `elsewhere` lists where it applies then.
    NotApplicable,
    /// Nothing establishes whether it applies there.
    Undetermined(UndeterminedReason),
    /// No state covers that date, at the reader's place nor anywhere else.
    OutsideWindow,
}

/// Why an article's applicability at a place is undetermined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UndeterminedReason {
    /// The place receives the journal on express mention, and none was read.
    NoMention,
    /// The matter passed to the place with no local instrument known: the
    /// served version is the one in force on the day of the transfer.
    Frozen,
}

/// The heading the reader picks a place under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceGroup {
    Metropole,
    OverseasIdentity,
    OverseasSpecialty,
    OtherStates,
}

/// A place an article can be read from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadablePlace {
    pub place: PlaceRef,
    pub group: PlaceGroup,
    /// `dateDebut` of the version the place reads at the reader's date, absent
    /// when no state covers it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// A state of the article in force at the reader's date in other places.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateElsewhere {
    pub places: Vec<PlaceRef>,
    pub starts_on: String,
    /// `dateDebut` of the source version the state carries, which is the date
    /// segment of its reading URL.
    pub version: String,
}

/// Where and when the served version applies, read from the compiled states.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Applicability {
    pub reader: PlaceRef,
    pub status: ApplicabilityStatus,
    /// Places of the state covering the reader; empty otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub places: Vec<PlaceRef>,
    /// Absent when the source does not date the start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_on: Option<String>,
    /// The clause of the source that gives the bound, verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_on_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends_on: Option<String>,
    /// `received` from the source, `computed` from the neighbouring state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends_on_origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends_on_source: Option<String>,
    /// Other states covering the same reader at the same date: a conflict the
    /// source left, shown rather than decided.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub competing: Vec<StateElsewhere>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elsewhere: Vec<StateElsewhere>,
    /// The source of the rule of reception that excludes the article, freezes
    /// it or asks for an express mention, verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_source: Option<String>,
    /// The day of the transfer whose wording a frozen matter keeps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_on: Option<String>,
    /// Where the article applies or keeps a frozen wording at the reader's
    /// date, and the reader's own place.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub readable: Vec<ReadablePlace>,
}

/// What applies in a place at a date (`GET /internal/corpus`), grouped by
/// journal then nature. Every group carries its whole count and a sample of
/// titles, the metropolitan fund not fitting in one page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlaceCorpusResponse {
    pub place: PlaceRef,
    pub date: String,
    pub total: i64,
    pub journals: Vec<CorpusJournal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CorpusJournal {
    pub code: String,
    pub label: String,
    pub total: i64,
    pub groups: Vec<CorpusGroup>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CorpusGroup {
    pub nature: String,
    pub label: String,
    /// Instruments the group holds, of which `texts` carries the first ones.
    pub total: i64,
    pub texts: Vec<CorpusText>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CorpusText {
    pub title: String,
    pub slug: String,
    #[serde(default)]
    pub jurisdiction: String,
}

/// The place vocabulary a reader chooses from, roots first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlacesResponse {
    pub places: Vec<PlaceNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlaceNode {
    pub uid: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub group: PlaceGroup,
}

/// Défaut de `LawArticleResponse::translation` (droit FR LEGI = texte officiel).
fn default_officiel() -> String {
    "officiel".to_string()
}

/// Article LEGI servi à une date (law-at-date). `code` = `code_court` (slug).
/// `dateDebut` ISO requise ; `dateFin` absente ⇒ pas de fin.
/// `legifranceUrl` versionnée `/codes/article_lc/{LEGIARTI}/{date}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawArticleResponse {
    pub legiarti: String,
    pub legitext: String,
    pub code: String,
    /// Jurisdiction that enacts the text, the first segment of its URL:
    /// `code` alone no longer identifies a text, two countries carrying the
    /// same name for their civil code.
    pub jurisdiction: String,
    /// The jurisdiction's name, « France » for `FR`.
    pub jurisdiction_label: String,
    /// Titre humain du texte (`legal_text.title`, ex. « Code de la famille
    /// sénégalais ») pour l'affichage « Article N du … ». `code` reste le slug
    /// d'URL ; sans ce champ le front retombe sur le slug humanisé (laid).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_title: Option<String>,
    pub num: String,
    /// Clé canonique de l'article (`num_key`) pour les liens internes — l'URL
    /// `/norm/{juris}/{code}/{numKey}` que le serve résout en lookup exact.
    /// `num` reste le libellé affiché.
    pub num_key: String,
    pub etat: String,
    pub date_debut: String,
    /// Provenance de la version servie (`legifrance` / `jorf` / `treaty`) — pour
    /// lier vers la section descriptive de `/sources`.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub titre_text: Option<String>,
    /// Clickable TOC breadcrumb: the article's enclosing divisions, from the root
    /// to the direct section, `href` = the section's reading view
    /// (`/norm/{juris}/{code}/{path}`). Empty when the structure is not ingested
    /// (foreign text, JORF): the front falls back on `titreText`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub breadcrumb: Vec<LinkedTextRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_fin: Option<String>,
    /// `dateFin` dépassée au jour de la lecture, calculé côté API (l'horloge vit
    /// côté serveur) : la version servie ne s'applique plus. `etat` ne le dit
    /// pas, KALI le laissant à `VIGUEUR` sur des versions dépassées.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub expired: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texte: Option<String>,
    /// Renvois cliquables du corps : spans codepoints demi-ouverts sur `texte`,
    /// même forme que côté décisions. Hrefs datés quand l'article est servi à date
    /// explicite — le renvoi navigue dans le même temps que la lecture.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub texte_spans: Vec<CitationSpan>,
    /// Corps dans la langue d'origine, affiché en regard du FR si présent
    /// (couche vérification/vérité ; souvent absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texte_original: Option<String>,
    /// Langue de `texteOriginal` (ISO-639-1, `ar`…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang_original: Option<String>,
    /// Provenance du `texte` : `officiel` / `non_officiel` / `automatique`
    /// → badge UI. Défaut `officiel` (droit FR LEGI).
    #[serde(default = "default_officiel")]
    pub translation: String,
    /// Fraîcheur « as-of » effective : dernière base crédible que la copie reflète
    /// le droit en vigueur (ISO `YYYY-MM-DD`). Jamais inconnue (au pire la date de
    /// get). `legifrance`/`kali` : date du dernier sync ; sinon date de
    /// get/curation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_asof: Option<String>,
    /// Autorité du diffuseur, libellé FR (« source gouvernementale »,
    /// « agrégateur tiers », « traduction automatique »…). Axe distinct de
    /// `translation` (officialité du texte). Calculé via mapping pur
    /// `lj_norm::source_authority`.
    pub source_authority: String,
    /// Le diffuseur est l'État ou l'institution dont le texte émane, et non un
    /// agrégateur qui le republie : ce que le front dit de `sourceUrl`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub source_official: bool,
    /// Lien « source » unique de l'article : page/PDF du diffuseur pour le curé
    /// (étranger, traités), ou page Légifrance versionnée pour un article natif
    /// LEGI/KALI (calculée côté API depuis l'identité). Absente seulement si aucune
    /// des deux n'est dérivable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// Source secondaire amont : site qu'un agrégateur (jafbase) pointe. Rare.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_upstream_url: Option<String>,
    /// Apparat éditorial affiché : la « Nota » officielle Légifrance (entrée en
    /// vigueur, QPC, applicabilité) ou la jurisprudence/doctrine + renvois « voir
    /// aussi » des éditions annotées curées. Rendu sous le corps, distinct du texte
    /// normatif. Absent si l'article n'en porte pas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nota: Option<String>,
    /// Première version future de l'article (`date_debut` > aujourd'hui, ISO,
    /// calculé côté API) : bandeau « sera modifié le … ». La sentinelle
    /// `2222-02-22` signifie « à une date à déterminer ».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upcoming_version_date: Option<String>,
    pub versions: Vec<LawArticleVersion>,
    /// Articles voisins pour la lecture en contexte : division enclosante ou
    /// fenêtre, l'article courant marqué `current`. Vide si pas de contexte
    /// exploitable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<ArticleNeighbor>,
    /// Corps « dispositions modifiées » d'un article modificatif (ordonnance/loi
    /// de réforme), dérivé du graphe `legal_link` (bloc `<LIENS>` DILA) : cibles
    /// exactes par ID, liens garantis. Non vide ⇒ le front rend cette liste au lieu
    /// de `texte` (qui reste le résumé brut, illisible). Vide pour un article
    /// normal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modifications: Vec<ArticleModification>,
    /// Textes qui ont modifié/créé/abrogé CET article (liens entrants du graphe) :
    /// l'historique « Modifié par », dans l'ordre du fichier source.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modified_by: Vec<LinkedTextRef>,
    /// Dispositions qui citent cet article (liens entrants CITATION).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cited_by: Vec<LinkedTextRef>,
    /// Commentaires de norme : doctrine ancrée sur cet article ou sur le texte
    /// entier, même forme que côté décision. Non cherchables — accordéon fermé en
    /// fin de page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commentaires: Vec<Commentaire>,
    /// Travaux parlementaires (zéro ingest) : une ligne par loi modificatrice de
    /// l'article, `href` composé
    /// `https://www.legifrance.gouv.fr/jorf/id/{JORFTEXT}` (bloc « Travaux
    /// préparatoires » et dossiers législatifs servis par Légifrance).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub travaux_parlementaires: Vec<LinkedTextRef>,
    /// Where and when the served version applies for the reader's place, read
    /// from the compiled states. Absent while the text has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applicability: Option<Applicability>,
}

/// Opération d'un segment du comparateur de versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LawCompareOp {
    Equal,
    Insert,
    Delete,
}

/// Tronçon contigu du diff ; les sauts de ligne restent dans `text`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawCompareSegment {
    pub op: LawCompareOp,
    pub text: String,
}

/// Nature d'une ligne du vis-à-vis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LawCompareRowKind {
    Equal,
    Changed,
    Added,
    Removed,
}

/// Une ligne alignée du comparateur. Concaténer `left` sur toutes les lignes
/// rend la rédaction de référence, `right` la comparée.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawCompareRow {
    pub kind: LawCompareRowKind,
    /// Vide sur un `added`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub left: Vec<LawCompareSegment>,
    /// Vide sur un `removed`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub right: Vec<LawCompareSegment>,
}

/// Réponse du comparateur de versions d'un article : identité, les deux
/// versions comparées, le vis-à-vis, et la timeline pour les sélecteurs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawCompareResponse {
    pub code: String,
    pub jurisdiction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_title: Option<String>,
    pub num: String,
    pub num_key: String,
    pub from: LawArticleVersion,
    pub to: LawArticleVersion,
    pub rows: Vec<LawCompareRow>,
    pub versions: Vec<LawArticleVersion>,
}

/// Une référence de texte/article liée par le graphe `legal_link` : libellé
/// verbatim DILA (« LOI n°2018-287 du 20 avril 2018 - art. 16 »), lien interne
/// quand la cible est en base, date de signature du texte si connue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkedTextRef {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

/// Article voisin pour le contexte de lecture : numéro + état, sans corps
/// (clic = navigation `/norm/{juris}/{code}/{numKey}`). `current` = l'article de la
/// page. `numKey` = clé canonique pour le lien ; `num` = affichage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArticleNeighbor {
    pub num: String,
    pub num_key: String,
    pub etat: String,
    pub current: bool,
}

/// Une cible d'une disposition d'article modificatif (graphe `legal_link`) :
/// un article (numéro linkable, `href` garanti quand la cible est en base),
/// une division de code (`section`, non linkable), ou un texte entier
/// (`texte`, lié à son sommaire).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModificationItem {
    /// `article` (numéro dans `label`), `section` (titre de division) ou
    /// `texte` (libellé complet).
    pub kind: String,
    /// Numéro d'article (« 1302 », « L611-7 »), titre de section, ou libellé.
    pub label: String,
    /// Internal link (`/norm/{juris}/{code}/{numKey}` for a resolved article,
    /// `/norm/{juris}/{code}` for a text); absent if the target is not in base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

/// Un bloc du corps « dispositions modifiées » d'un article modificatif (graphe
/// `legal_link`). Une action portant sur un texte cible, avec ses dispositions.
/// Rendu en liste par le front.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArticleModification {
    /// `modifie` / `cree` / `abroge`.
    pub action: String,
    /// Nom affichable du code cible (« Code civil »).
    pub code: String,
    /// Internal link to the target code's table of contents (`/norm/{juris}/{code}`) when resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_href: Option<String>,
    pub items: Vec<ModificationItem>,
}

/// Sommaire d'un code LEGI. `code` = `code_court` (slug).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawCodeSummary {
    pub legitext: String,
    pub code: String,
    pub jurisdiction: String,
    pub titre: String,
    pub nature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derniere_modification: Option<String>,
    pub article_count: i64,
    /// Dates programmées de versions futures du texte (`VERSIONS_A_VENIR`
    /// DILA), ISO, triées — bandeau « sera modifié le … ». La sentinelle
    /// `2222-02-22` signifie « à une date à déterminer ».
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upcoming_versions: Vec<String>,
    /// Corps monolithique d'un texte sans articles (circulaires…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// État de diffusion du texte (`VIGUEUR`/`ABROGE`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// NOR (familles qui en portent : circulaires, actes JO).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nor: Option<String>,
    /// Date de signature ISO (`date_texte`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_texte: Option<String>,
    /// The scope label when the text is not a norm, absent for a norm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Fond du catalogue des normes, maillage retour vers `/norm-index/{juris}/{fond}` ;
    /// absent pour un acte individuel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fond: Option<String>,
    /// Libellé du fond (« Lois et ordonnances »).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fond_label: Option<String>,
    /// Année de parcours du texte dans son fond (`/norm-index/{juris}/{fond}/{annee}`) ;
    /// absente = bucket `sans-date`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fond_year: Option<i32>,
    /// Les articles les plus cités du texte entier (rebond de la colonne droite) —
    /// même palmarès que celui d'une division, au périmètre du code.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top_cited: Vec<TopCitedArticle>,
    /// Commentaries anchored on the whole text (`num_key` NULL), in the same
    /// shape as on the decision and the article sides.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commentaires: Vec<Commentaire>,
    /// What the JO printed around a text's articles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<TextPublication>,
}

/// The blocks of a JO text outside its articles, and the issue that carried it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextPublication {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<LinkedTextRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visas: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signatories: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preparatory_work: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corrigendum: Option<String>,
}

/// Décision citant un article LEGI, via `legal_citation`. La liste est ordonnée
/// par autorité décroissante côté store ; `publication_codes` porte le badge.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CitingDecisionHit {
    pub id: String,
    pub title: String,
    pub jurisdiction_type: JurisdictionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    pub publication_codes: Vec<String>,
    /// Résumé de la décision (première phrase affichée en carte, comme les
    /// décisions similaires). Absent tant que le cron résumé n'est pas passé.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Clés canoniques des articles cités parmi ceux interrogés — servi par
    /// les citantes d'une division, où la ligne doit dire par quel article la
    /// décision entre. Vide pour les citantes d'un article : la réponse est dans
    /// l'URL. Le front y résout les numéros affichables depuis la vue-lecture qu'il
    /// rend déjà.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cited_articles: Vec<String>,
}

/// Article « souvent cité avec » l'article de la page : co-citation dans
/// `legal_citation` sur un échantillon de décisions citantes, boilerplate
/// procédural exclu. `numKey` = numéro affichable et clé de lien ; `count` =
/// décisions de l'échantillon citant les deux articles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoCitedArticle {
    pub num_key: String,
    pub text_title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    pub count: i64,
}

/// Un résultat de recherche d'article (`/recherche-textes`). `code` = slug du
/// texte parent (lien `/norm/{juris}/{code}/{numKey}`) ; `codeTitle` = titre lisible du
/// code ; `titrePath` = fil d'Ariane ; `snippet` = extrait surligné du corps.
/// `numKey` = clé canonique pour le lien, `num` = affichage. `status` +
/// `dateDebut` (ISO) portent le badge d'état de la carte : libellé
/// [`article_status_label`] pour une version, « JO du <date> » quand
/// `source = "jorf"` (une publication n'a pas d'état).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArticleSearchHit {
    pub code: String,
    pub jurisdiction: String,
    pub code_title: String,
    pub num: String,
    pub num_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub titre_path: Option<String>,
    pub snippet: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_debut: Option<String>,
    /// The entry into force of the version served, the text's date standing in
    /// for a start the source leaves unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<String>,
    /// The end of the version served, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<String>,
}

/// Réponse de `/internal/search-textes` : hits paginés, total exact sous le
/// prédicat BM25 + filtres, et comptes par facette.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArticleSearchResponse {
    pub hits: Vec<ArticleSearchHit>,
    pub total: i64,
    pub facets: ArticleSearchFacets,
    /// `Hybrid` never comes out of this engine, which has no vector leg.
    #[serde(default = "default_textes_query_mode")]
    pub query_mode: QueryMode,
    /// The articles the query designates, served apart from the list in a
    /// content mode, three at most. Empty in `reference` mode, where the list is
    /// the designation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned: Vec<PinnedArticle>,
    /// More articles carry the designation than `pinned` shows: the card is a
    /// sample, and `mode=reference` on the same query is the whole list.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned_more: bool,
    /// The corpora the answer covers, values of the `jurisdiction` filter: what
    /// the query resolved to, or the filter as stated. Empty when unrestricted.
    #[serde(default)]
    pub jurisdictions: Vec<String>,
    /// The place these results are read from, when there is one: the reader's
    /// own, else the country a single-country answer designates and that the
    /// place vocabulary knows. A page opened from here keeps it, and the rail
    /// names it, which is why the label travels with the uid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reader_place: Option<PlaceRef>,
}

fn default_textes_query_mode() -> QueryMode {
    QueryMode::Lexical
}

/// No `snippet`: served by identity rather than by a matched passage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PinnedArticle {
    pub code: String,
    pub jurisdiction: String,
    pub code_title: String,
    pub num: String,
    pub num_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub titre_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_debut: Option<String>,
}

/// Comptes par facette de la recherche d'articles, sous le même prédicat que les hits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArticleSearchFacets {
    /// Textes porteurs de hits : `value` = slug (`/norm/{juris}/{slug}`), token du
    /// filtre `code`. Top-20 par count — l'énumération complète pèserait
    /// plusieurs Mo de JSON par réponse. Les autres axes et `total` restent
    /// exacts.
    #[serde(default)]
    pub code: Vec<FacetChoice>,
    pub jurisdiction: Vec<FacetChoice>,
    pub source: Vec<FacetChoice>,
    /// The scope: `norme` | `doctrine_administrative` | `information_publique`,
    /// summed by the API from the `nature` buckets, with no column of its own.
    #[serde(default)]
    pub scope: Vec<FacetChoice>,
    /// Every jurisdiction path the hits sit on, an order and each level of a
    /// type under it (`FR`, `FR.LOI`, `FR.LOI.ORGANIQUE`), `parent` the level above.
    #[serde(default)]
    pub path: Vec<FacetChoice>,
}

/// Entrée du catalogue des codes (`/internal/codes`). `code` = slug (lien `/texte/{code}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodeCatalogueEntry {
    pub code: String,
    pub title: String,
    pub nature: String,
    pub jurisdiction: String,
    pub jurisdiction_label: String,
    pub article_count: i64,
}

/// Réponse de `/internal/codes` : liste des textes navigables du corpus. Le
/// périmètre `scope=head` (codes, constitutions, état civil — le fond `codes`
/// de l'arborescence des normes) est appliqué en SQL ; le reste du catalogue se
/// parcourt par année sous `/norm-index/{juris}/{fond}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodeCatalogueResponse {
    pub entries: Vec<CodeCatalogueEntry>,
}

/// Une juridiction du catalogue hub (`/internal/juridictions`).
/// `code` = clé d'URL (`/decision-index/{juris}/{code}`).
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

/// Réponse de `/internal/juridictions` : catalogue groupé par famille.
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

/// Réponse de `/internal/juridictions/{code}` : hub d'une juridiction, années
/// couvertes (plus récente en tête) et volume total.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JurisdictionHubResponse {
    pub code: String,
    pub label: String,
    pub jurisdiction_type: String,
    pub type_label: String,
    /// The court's node on the seat axis (`FR.TJ.EPINAL`), token of the `seat`
    /// filter. Derived from the code by the API, which alone holds the
    /// vocabulary renaming a court.
    pub seat_path: String,
    pub decision_count: i64,
    pub years: Vec<JurisdictionYearCount>,
}

/// Une décision d'une page hub juridiction×année : le lien crawlable
/// (`title` = ancre, `publicId` → la page de la décision).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JurisdictionHubDecision {
    pub public_id: String,
    pub title: String,
    pub date_lecture: String,
}

/// Réponse de `/internal/juridictions/{code}/{annee}` : page paginée des décisions
/// d'une année (`total` = décisions de l'année, pagination par `page`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JurisdictionYearResponse {
    pub code: String,
    pub label: String,
    pub jurisdiction_type: JurisdictionType,
    /// See [`JurisdictionHubResponse::seat_path`].
    pub seat_path: String,
    pub year: i32,
    pub total: i64,
    pub page: u32,
    pub page_size: u32,
    pub decisions: Vec<JurisdictionHubDecision>,
}

/// Un fond du catalogue des normes d'une juridiction. `fond` = clé d'URL
/// (`/norm-index/{juris}/{fond}`) ; le fond `codes` renvoie vers le catalogue
/// des codes (pas de hub année).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormFondEntry {
    pub fond: String,
    pub label: String,
    pub text_count: i64,
}

/// A jurisdiction's fonds, in editorial order. `jurisdiction` is the URL segment
/// (`fr`, `eu`, `intl`, `de`…).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormJurisdictionEntry {
    pub jurisdiction: String,
    pub text_count: i64,
    pub fonds: Vec<NormFondEntry>,
}

/// Réponse de `/internal/normes` : le catalogue de chaque juridiction, la plus
/// fournie en tête.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormCatalogueResponse {
    pub jurisdictions: Vec<NormJurisdictionEntry>,
}

/// Compteur d'une année d'un hub fond. `year = None` = bucket « sans date »
/// (URL `/norm-index/{juris}/{fond}/sans-date`), toujours en dernier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormYearCount {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    pub count: i64,
}

/// Réponse de `/internal/normes/{juris}/{fond}` : hub d'un fond, années
/// couvertes (plus récente en tête, bucket sans date en queue) et volume total.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormFondResponse {
    pub jurisdiction: String,
    pub fond: String,
    pub label: String,
    /// The norms search's `jurisdiction` values selecting the fond (`FR.DECRET`).
    pub search_paths: Vec<String>,
    pub text_count: i64,
    pub years: Vec<NormYearCount>,
}

/// Un texte d'une page hub fond×année : le lien crawlable (`title` = ancre,
/// `slug` et `jurisdiction` → `/norm/{juris}/{slug}`). `date` absente sur
/// le bucket sans date.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormHubText {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub jurisdiction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

/// Réponse de `/internal/normes/{juris}/{fond}/{annee}` : page paginée des
/// textes d'une année (`year = None` = bucket « sans date », `total` = textes
/// du bucket).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormYearResponse {
    pub jurisdiction: String,
    pub fond: String,
    pub label: String,
    /// See [`NormFondResponse::search_paths`].
    pub search_paths: Vec<String>,
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
    /// Fenêtre close au jour de la lecture : l'article ne s'applique plus. KALI
    /// laisse `status` à `VIGUEUR` sur des versions dépassées, d'où les deux.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub expired: bool,
}

/// Node of a text's real tree of contents, flattened in reading order; the front
/// rebuilds the nesting from `depth` (1 = first level). `kind` = `section` (with
/// `path`, the anchor `#{path}` and the reading view address
/// `/norm/{juris}/{code}/{path}`, absent when the section has no address) or
/// `article` (with `numKey`, the link `/norm/{juris}/{code}/{numKey}`). Sibling
/// sections with the same heading are a single node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TocNode {
    pub kind: String,
    pub depth: i32,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_key: Option<String>,
    pub etat: String,
}

/// Taille maximale (en articles) d'une vue-lecture servie/matérialisée en
/// place : seuil de la vue-lecture intégrale des textes courts côté API
/// (`reading` du sommaire) et borne d'éligibilité d'une division à l'accordéon
/// de lecture côté front. Une seule règle, récursive — le texte entier n'est
/// que la division racine.
pub const INLINE_READING_MAX: usize = 150;

/// Second bound of the same rule, in body bytes: a division of few articles can
/// still weigh far more than most. Past it, the division serves its index and
/// not its bodies.
pub const INLINE_READING_MAX_BYTES: usize = 50_000;

/// Réponse de `/internal/norm/{code}/sommaire` : table des matières d'un code.
/// `tree` = l'arbre structurel réel daté quand le texte en a un ; il prime sur
/// `entries` (sommaire à plat par `titlePath`, servi seulement quand `tree` est
/// vide — textes sans structure ingérée). `reading` = vue-lecture intégrale
/// (corps joints) servie à la place des `entries` pour les textes courts ;
/// `tree` reste servi pour le rail « Plan du texte ».
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodeTocResponse {
    pub entries: Vec<TocEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tree: Vec<TocNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reading: Vec<LawSectionItem>,
}

/// Item de la vue-lecture d'une section : sous-arbre en ordre de lecture.
/// `kind` = `section` (intertitre, `path` pour l'ancre) ou `article` (corps
/// `texte`/`nota` joints ; `numKey` → lien vers la page article).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawSectionItem {
    pub kind: String,
    pub depth: i32,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_key: Option<String>,
    pub etat: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texte: Option<String>,
    /// The cross-references of `texte`, as on the article's own page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub texte_spans: Vec<CitationSpan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nota: Option<String>,
}

/// Référence d'une division du texte : fil d'Ariane et navigation bloc
/// précédent / suivant de la vue-lecture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawSectionRef {
    pub path: String,
    pub label: String,
}

/// Article of a scope's "most cited" ranking: a division for
/// `/norm/{juris}/{code}/{path}`, the whole text for `/norm/{juris}/{code}`, ranked
/// by `citing_decision_counts`. `numKey` = the article's canonical key, `label` =
/// its displayable number (formatted by the front like the reading view), `href` =
/// its page. `count` = citing decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TopCitedArticle {
    pub num_key: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    pub count: i64,
}

/// Texte citant une division, agrégé au grain du texte citant et non de son
/// article : un arrêté qui vise trois articles de la division sort une fois,
/// `articleCount` à 3. C'est ce compte qui classe la liste, et le seuil de deux
/// articles qui la débruite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SectionCitingText {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    pub article_count: i64,
}

/// Réponse de `/internal/norm/{code}/section/{path}` : vue-lecture d'une section,
/// les articles de la division rendus à la suite, intertitres des sous-sections
/// inclus. `code` = slug du texte, `title` = titre de la section (porté par
/// l'arête parente). `ancestors` = divisions englobantes (de la racine au
/// parent) ; `prev`/`next` = bloc précédent / suivant en ordre de lecture (hors
/// sous-arbre de la section). `topCited` = les articles les plus cités du
/// sous-arbre ; `citingTexts` = les textes qui citent la division, au centre
/// sous les décisions citantes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LawSectionResponse {
    pub code: String,
    pub jurisdiction: String,
    /// The jurisdiction's name, « France » for `FR`.
    pub jurisdiction_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_title: Option<String>,
    pub path: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ancestors: Vec<LawSectionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev: Option<LawSectionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<LawSectionRef>,
    pub items: Vec<LawSectionItem>,
    /// Nombre d'articles dont le corps a été retiré parce que la division
    /// dépasse [`INLINE_READING_MAX`] ou [`INLINE_READING_MAX_BYTES`] : les
    /// items restent, avec leur numéro et leur lien, sans leur texte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omitted: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top_cited: Vec<TopCitedArticle>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub citing_texts: Vec<SectionCitingText>,
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
    /// Timestamp RFC 3339.
    pub created_at: String,
    pub last_seen_at: String,
}

/// Résumé du compte pour la page profil : ce que la page rend, et
/// rien de plus — les moyens d'accès y compris, pour la garde de déliaison.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub has_password: bool,
    pub identities: Vec<LinkedIdentity>,
}

/// Une identité externe liée au compte.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedIdentity {
    pub provider: String,
    pub provider_sub: String,
    /// Timestamp RFC 3339.
    pub linked_at: String,
}

// ── Signets ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BookmarkItem {
    pub id: String,
    pub title: String,
    pub jurisdiction_type: JurisdictionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    /// Solution résolue (clé + libellé référentiel).
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

/// What a reader opened: a decision, a legal text by article, by section or as
/// a whole, an entity or a notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReadKind {
    Decision,
    Article,
    Section,
    Text,
    Entity,
    Annonce,
}

impl ReadKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decision => "decision",
            Self::Article => "article",
            Self::Section => "section",
            Self::Text => "text",
            Self::Entity => "entity",
            Self::Annonce => "annonce",
        }
    }
}

/// Reported by the browser, since a page served from the CDN cache never
/// reaches the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadReport {
    pub kind: ReadKind,
    /// The page's path, which names the document read.
    pub reference: String,
    #[serde(default)]
    pub document: String,
    /// The document the browser loaded, against a navigation inside the app.
    pub landing: bool,
    pub referrer: Option<String>,
    pub utm_source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LandingReport {
    pub path: String,
    pub referrer: Option<String>,
    pub utm_source: Option<String>,
    pub utm_campaign: Option<String>,
}

/// An annuaire search the browser shows, reported by it for the same reason as
/// a reading: the results page is served from the CDN cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntitySearchReport {
    pub query: String,
}

/// A front feature used: a click on a button or on any control inside a marked block,
/// or a copy of a field's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiAction {
    DecisionBookmark,
    DecisionCopyFull,
    DecisionCopyShort,
    DecisionPrint,
    DecisionPdf,
    DecisionSelectionCopy,
    DecisionSelectionCopyReference,
    DecisionSelectionSearch,
    DecisionBackToResults,
    DecisionResultStep,
    DecisionTocSynthesis,
    DecisionTocPreamble,
    DecisionTocProcedure,
    DecisionTocVisa,
    DecisionTocExpose,
    DecisionTocMoyens,
    DecisionTocChapeau,
    DecisionTocMotivations,
    DecisionTocDispositif,
    DecisionTocAnnexes,
    DecisionTocOther,
    DecisionChronology,
    DecisionMeta,
    DecisionMetaCopyJurisdiction,
    DecisionMetaCopyCaseName,
    DecisionMetaCopyDockets,
    DecisionMetaCopyDate,
    DecisionMetaCopySolution,
    DecisionMetaCopyDomain,
    DecisionMetaCopyChamber,
    DecisionMetaCopyFormation,
    DecisionMetaCopyHearingDate,
    DecisionMetaCopyPublication,
    DecisionMetaCopyNomenclature,
    DecisionText,
    DecisionCommentaries,
    DecisionProvenance,
    DecisionReferences,
    DecisionParties,
    DecisionCiting,
    DecisionSimilar,
    SearchSuggestion,
    SearchSyntaxHelp,
    SearchView,
    SearchRailYears,
    SearchRailArticles,
    SearchRailSeats,
    SearchRailSolutions,
    SearchCardView,
    SearchFilterChip,
    HomeExample,
    NormCopyReference,
    MenuBookmarks,
}

impl UiAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DecisionBookmark => "decision_bookmark",
            Self::DecisionCopyFull => "decision_copy_full",
            Self::DecisionCopyShort => "decision_copy_short",
            Self::DecisionPrint => "decision_print",
            Self::DecisionPdf => "decision_pdf",
            Self::DecisionSelectionCopy => "decision_selection_copy",
            Self::DecisionSelectionCopyReference => "decision_selection_copy_reference",
            Self::DecisionSelectionSearch => "decision_selection_search",
            Self::DecisionBackToResults => "decision_back_to_results",
            Self::DecisionResultStep => "decision_result_step",
            Self::DecisionTocSynthesis => "decision_toc_synthesis",
            Self::DecisionTocPreamble => "decision_toc_preamble",
            Self::DecisionTocProcedure => "decision_toc_procedure",
            Self::DecisionTocVisa => "decision_toc_visa",
            Self::DecisionTocExpose => "decision_toc_expose",
            Self::DecisionTocMoyens => "decision_toc_moyens",
            Self::DecisionTocChapeau => "decision_toc_chapeau",
            Self::DecisionTocMotivations => "decision_toc_motivations",
            Self::DecisionTocDispositif => "decision_toc_dispositif",
            Self::DecisionTocAnnexes => "decision_toc_annexes",
            Self::DecisionTocOther => "decision_toc_other",
            Self::DecisionChronology => "decision_chronology",
            Self::DecisionMeta => "decision_meta",
            Self::DecisionMetaCopyJurisdiction => "decision_meta_copy_jurisdiction",
            Self::DecisionMetaCopyCaseName => "decision_meta_copy_case_name",
            Self::DecisionMetaCopyDockets => "decision_meta_copy_dockets",
            Self::DecisionMetaCopyDate => "decision_meta_copy_date",
            Self::DecisionMetaCopySolution => "decision_meta_copy_solution",
            Self::DecisionMetaCopyDomain => "decision_meta_copy_domain",
            Self::DecisionMetaCopyChamber => "decision_meta_copy_chamber",
            Self::DecisionMetaCopyFormation => "decision_meta_copy_formation",
            Self::DecisionMetaCopyHearingDate => "decision_meta_copy_hearing_date",
            Self::DecisionMetaCopyPublication => "decision_meta_copy_publication",
            Self::DecisionMetaCopyNomenclature => "decision_meta_copy_nomenclature",
            Self::DecisionText => "decision_text",
            Self::DecisionCommentaries => "decision_commentaries",
            Self::DecisionProvenance => "decision_provenance",
            Self::DecisionReferences => "decision_references",
            Self::DecisionParties => "decision_parties",
            Self::DecisionCiting => "decision_citing",
            Self::DecisionSimilar => "decision_similar",
            Self::SearchSuggestion => "search_suggestion",
            Self::SearchSyntaxHelp => "search_syntax_help",
            Self::SearchView => "search_view",
            Self::SearchRailYears => "search_rail_years",
            Self::SearchRailArticles => "search_rail_articles",
            Self::SearchRailSeats => "search_rail_seats",
            Self::SearchRailSolutions => "search_rail_solutions",
            Self::SearchCardView => "search_card_view",
            Self::SearchFilterChip => "search_filter_chip",
            Self::HomeExample => "home_example",
            Self::NormCopyReference => "norm_copy_reference",
            Self::MenuBookmarks => "menu_bookmarks",
        }
    }
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

/// What Légifrance writes as a `date_debut` when an entry into force is deferred
/// to a date it has not fixed: a flag, never a bound, so never ordered as a date.
pub const DATE_A_DETERMINER: &str = "2222-02-22";

/// Ranks of the version closing before `date` and of the one opening after it:
/// the gap a dated read fell into. `bornes` carries ISO `(start, end)` pairs, and
/// degenerate windows are dropped, a version scheduled then dropped carrying its
/// end before its start.
pub fn version_gap(bornes: &[(Option<&str>, Option<&str>)], date: &str) -> Option<(usize, usize)> {
    let saine = |b: &(Option<&str>, Option<&str>)| match (b.0, b.1) {
        (Some(debut), Some(fin)) => debut <= fin,
        _ => true,
    };
    let avant = bornes
        .iter()
        .enumerate()
        .filter(|(_, b)| saine(b) && b.1.is_some_and(|fin| fin < date))
        .max_by_key(|(_, b)| b.1)?;
    let apres = bornes
        .iter()
        .enumerate()
        .filter(|(_, b)| saine(b) && b.0.is_some_and(|debut| debut > date))
        .min_by_key(|(_, b)| b.0)?;
    Some((avant.0, apres.0))
}

/// Whether the status closing a gap says why nothing covers the date: the article
/// did not exist under that number then. Decides only that it is, the API and the
/// front not wording it in the same language.
pub fn gap_explique(status: &str) -> bool {
    status.starts_with("ABROGE") || matches!(status, "ANNULE" | "PERIME" | "TRANSFERE")
}

/// Libellé FR d'un état DILA de version d'article (`legal_article.status`) :
/// les états différés (`*_DIFF` = décidé mais pas encore effectif) et les cas
/// de chronique (mort-né, disjoint) deviennent lisibles à l'écran. État inconnu
/// ou vide → l'appelant retombe sur la valeur brute. Vit ici (pas dans
/// `lj-core`) : consommé par le front WASM comme par l'API — même statut que
/// [`instrument_with_de`].
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

// ── Fiche entité ────────────────────────────────────────────────────────────

/// An entity of the registry (`entity`): registry identity and litigation
/// aggregates from the actors linked to it.
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

/// Décisions citant l'entité, paginées, plus récentes d'abord. Les items
/// portent les mêmes hits que la recherche (rendu unifié `ResultCard`) plus le
/// rôle de l'entité.
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

// ── Autocomplétion ──────────────────────────────────────────────────────────

/// Suggestions d'autocomplétion (`GET /suggest`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SuggestResponse {
    /// Nombre de mots de fin de query que chaque suggestion remplace (contexte
    /// re-suggéré inclus — le mot en cours de frappe compte pour un).
    pub matched_tokens: u32,
    pub suggestions: Vec<String>,
}

// ── Annuaire des entités ────────────────────────────────────────────────────

/// Résultat de recherche d'entités (annuaire — registre complet).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntitySearchResponse {
    pub items: Vec<EntityDirectoryItemDto>,
}

/// Listing annuaire paginé d'une catégorie — registre complet, trié par
/// contentieux décroissant.
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
    /// Uid complet namespacé (ex. `siren:552043002`) → fiche `/entity/{juris}/{ns}/{id}`.
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
/// entités avec au moins une décision liée.
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
    /// Rang de la ligne dans la décision : la cible de `party_ord`.
    pub ord: i32,
    /// `ord` de la partie que ce conseil représente.
    pub party_ord: Option<i32>,
    /// Verbatim extrait de la décision.
    pub value: String,
    pub nature: Option<String>,
    /// Slug officiel CNB du barreau (counsel uniquement).
    pub barreau: Option<String>,
    /// Uid registre si résolu → fiche `/entity/{juris}/{ns}/{id}`.
    pub entity_uid: Option<String>,
}

// ── Fiche entité — volet registre servi par APIs externes ───────────────────

/// Volet registre d'une fiche entité, servi à l'affichage par les APIs
/// publiques (recherche-entreprises, BODACC/JOAFE Opendatasoft) — aucun stock
/// local. Chaque section absente (API indisponible, entité hors périmètre)
/// est simplement `None`/vide : le rendu dégrade sans erreur.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityRegistreResponse {
    /// Identité enrichie + dirigeants + finances (entreprises `siren:` seulement).
    pub entreprise: Option<RegistreEntrepriseDto>,
    /// Les annonces de l'entité, tous fonds confondus et groupés : le live DILA
    /// (BODACC pour `siren:`, JOAFE pour `rna:`) et nos lignes `annonce` par rôle
    /// d'arête, projetés dans le même `AnnonceHit`. Vide = aucune annonce, ou
    /// amont indisponible.
    pub annonces: Vec<EntityAnnonceGroupDto>,
    /// Liens sortants (annuaire-entreprises…).
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

/// Lien sortant vers la source officielle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistreLienDto {
    pub label: String,
    pub url: String,
}

/// A company's deeds and annual accounts as the INPI lists them, newest filing first.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityDocumentsResponse {
    pub comptes: Vec<EntityDocumentDto>,
    pub actes: Vec<EntityDocumentDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityDocumentDto {
    pub label: String,
    /// Filing date, ISO.
    pub filed: Option<String>,
    /// The file, served by librejustice.fr.
    pub url: String,
}

// ── Annonces — pot des événements juridiques en cours ───────────────────────

/// Libellé public d'un fond. `None` = jeton inconnu, que la frontière HTTP
/// refuse plutôt que de le laisser filtrer dans le vide.
pub fn annonce_fond_label(fond: &str) -> Option<&'static str> {
    match fond {
        ANNONCE_FOND_MARCHE => Some("Marchés publics"),
        ANNONCE_FOND_BODACC => Some("Vie des entreprises"),
        ANNONCE_FOND_JOAFE => Some("Journal officiel des associations"),
        _ => None,
    }
}

/// Ce que le fond nomme l'acteur porté par la ligne — l'acheteur d'une
/// consultation, l'entreprise d'une annonce BODACC. Un pot, un champ, un
/// libellé par fond.
pub fn annonce_acteur_role(fond: &str) -> &'static str {
    match fond {
        ANNONCE_FOND_BODACC => "Entreprise",
        ANNONCE_FOND_JOAFE => "Association",
        _ => "Acheteur",
    }
}

/// Ce fond porte-t-il une échéance ? Un marché ouvre un délai de dépôt ; une
/// annonce BODACC ou une parution JOAFE constate un fait déjà survenu. Un fond
/// sans échéance n'a rien à mettre dans un agenda, et on ne lui propose donc
/// pas de flux ICS.
pub fn annonce_fond_a_echeance(fond: &str) -> bool {
    fond == ANNONCE_FOND_MARCHE
}

/// Tri de la liste d'annonces. `echeance` est le défaut : le produit répond à
/// « qu'est-ce qui ferme bientôt chez moi ». `nouveautes` trie par première
/// apparition dans nos canaux — le seul curseur fiable pour les moteurs
/// scrapés, donc l'ordre du flux d'alerte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnonceSort {
    Echeance,
    Publication,
    Nouveautes,
}

/// Filtre de la fenêtre servie par `/annonces` — facettes seulement (ni BM25
/// ni embeddings), `q` est un filtre par mot sur le titre et l'acheteur.
/// `closes` élargit à l'archive (les lignes fermées restent servies : assiette
/// du « qui achète quoi »).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceSearchRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// Fonds retenus — un axe de facette comme les autres. Vide = tous les
    /// fonds, servis groupés : deux backends n'ont pas de score commun à
    /// comparer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fond: Vec<String>,
    /// Canonical département code (`75`, `2A`, `973`), [`lj_vocab::ZONE_FRANCE`]
    /// for a nationwide market, [`lj_vocab::ZONE_EU`] for a Europe-wide one,
    /// [`lj_vocab::ZONE_SANS`] for what no source located. A notice carries
    /// several; the filter overlaps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zone: Vec<String>,
    /// Suffixes d'uid `nature_marche:*` (`TRAVAUX`, `SERVICES`…).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub famille: Vec<String>,
    /// Divisions CPV — suffixes d'uid `cpv:*` (`45`, `72`…).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cpv: Vec<String>,
    /// Métiers qui répondent, suffixes d'uid `metier:*` (`charpente-couverture`,
    /// `espaces-verts`…). Une annonce en porte plusieurs ; le filtre recoupe.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metier: Vec<String>,
    /// Fenêtre d'échéance en jours (`7`, `30`) : ne garde que ce qui ferme
    /// dans l'intervalle. Les lignes sans échéance connue en sortent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub echeance_jours: Option<u32>,
    /// Inclut les annonces closes (défaut : la fenêtre vivante seule).
    #[serde(default)]
    pub closes: bool,
    #[serde(default = "default_annonce_sort")]
    pub sort: AnnonceSort,
    #[serde(default = "default_annonce_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
}

fn default_annonce_sort() -> AnnonceSort {
    AnnonceSort::Echeance
}
fn default_annonce_limit() -> u32 {
    20
}

impl Default for AnnonceSearchRequest {
    fn default() -> Self {
        Self {
            q: None,
            fond: Vec::new(),
            zone: Vec::new(),
            famille: Vec::new(),
            cpv: Vec::new(),
            metier: Vec::new(),
            echeance_jours: None,
            closes: false,
            sort: default_annonce_sort(),
            limit: default_annonce_limit(),
            offset: 0,
        }
    }
}

/// Une annonce en liste, quel que soit son fond : la projection commune où une
/// consultation de marché et une annonce BODACC se lisent dans la même ligne.
/// Le libellé d'acteur est la vérité affichable — le rattachement au graphe des
/// entités est faillible et vit à part (`entites` du détail).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceHit {
    pub uid: String,
    pub fond: String,
    pub fond_label: String,
    pub titre: String,
    /// What the notice says beyond its title. Absent where the source publishes
    /// none, and where it copied the title into it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Qui l'annonce concerne : l'acheteur d'une consultation, l'entreprise
    /// d'une annonce BODACC. Vide quand la source ne le publie pas.
    pub acteur: String,
    /// Comment ce fond nomme cet acteur (« Acheteur », « Entreprise »).
    pub acteur_role: String,
    /// Everywhere the market applies, code then the name the API resolved.
    /// Empty when no source located it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zones: Vec<FacetTag>,
    /// Famille d'avis du fond : nature du marché (`nature_marche:*`), famille
    /// BODACC (procédure collective, création, radiation…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub famille: Option<FacetTag>,
    /// Division CPV — axe propre au fond `marche`, absent partout ailleurs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpv: Option<FacetTag>,
    /// `cpv` read off the title, no source having published a code. Said rather
    /// than let pass for something the buyer announced.
    #[serde(default)]
    pub cpv_deduit: bool,
    /// The trades that answer this market, from its descriptors and works lots,
    /// else read off its title. Crosses `cpv` rather than refining it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metiers: Vec<FacetTag>,
    /// One of `metiers` read off the title, neither a descriptor nor a works lot naming it.
    #[serde(default)]
    pub metiers_deduits: bool,
    /// Date de parution ISO `YYYY-MM-DD`.
    pub date_publication: String,
    /// Date limite de remise des offres, RFC 3339. Absente = inconnue de toutes
    /// les sources — la ligne n'entre alors jamais dans la fenêtre vivante.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub echeance: Option<String>,
    /// Consultation clôturée : échéance passée.
    #[serde(default)]
    pub closed: bool,
    /// Marché attribué : un avis de résultat est arrivé. Implique `closed` — un
    /// marché attribué ne prend plus d'offres. Les trois états du marché se lisent
    /// dans cet ordre : attribué, sinon clos, sinon ouvert.
    #[serde(default)]
    pub attribue: bool,
    /// Où agir : la page de la consultation sur la plateforme de l'acheteur,
    /// le PDF officiel d'une annonce BODACC.
    pub url_action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub procedure: Option<String>,
    /// Première apparition dans nos canaux (RFC 3339) — le curseur des flux
    /// d'alerte : les moteurs scrapés n'ont pas de date de publication fiable et
    /// le BOAMP republie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_seen_at: Option<String>,
    /// Canaux qui publient l'annonce (`boamp`, `atexo`…) — la preuve de la
    /// couverture unifiée, et l'aveu du contraire quand il n'y en a qu'un.
    #[serde(default)]
    pub canaux: Vec<String>,
}

/// Un axe de la barre de filtres. La liste des axes est déclarée par le
/// backend du fond et non figée dans le contrat : on n'offre jamais un filtre
/// que la source ne sait pas exécuter — le CPV n'existe que pour les marchés,
/// la fenêtre d'échéance que pour ce qui a une échéance. `key` est la clé
/// d'URL, donc aussi celle du paramètre d'API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceFacetAxis {
    pub key: String,
    pub label: String,
    pub choices: Vec<FacetChoice>,
}

/// Les annonces d'un fond. La vue par défaut en empile une par fond plutôt que
/// d'entrelacer : deux backends n'ont pas de score commun à comparer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceFondGroup {
    pub fond: String,
    pub label: String,
    pub total: i64,
    pub hits: Vec<AnnonceHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceSearchResponse {
    /// Somme des totaux de fond.
    pub total: i64,
    /// Un groupe par fond interrogé, dans l'ordre du vocabulaire. Un seul fond
    /// filtré ⇒ un seul groupe, que le front rend en liste simple.
    pub groupes: Vec<AnnonceFondGroup>,
    pub sort: AnnonceSort,
    /// Barre de filtres pour l'état courant : chaque axe est compté sans son
    /// propre filtre (multi-sélection : cocher un département n'efface pas les
    /// autres du décompte), les autres filtres appliqués.
    #[serde(default)]
    pub axes: Vec<AnnonceFacetAxis>,
}

/// Un lien profond de la consultation, par usage : l'avis, le règlement de la
/// consultation, le dossier. Aucun fichier n'est stocké — ce sont des liens
/// sortants vers la plateforme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceLienDto {
    /// `avis` | `rc` | `dce`.
    pub kind: String,
    pub label: String,
    pub url: String,
    pub moteur: String,
}

/// Un canal de publication de l'annonce. `firstSeenAt` est le curseur des
/// alertes ; la présence de la ligne dit à elle seule que le canal publie
/// encore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceCanalDto {
    pub moteur: String,
    pub source_uid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_avis: Option<String>,
    pub first_seen_at: String,
}

/// Une entité du graphe rattachée à l'annonce, par rôle (`acheteur`,
/// `titulaire`…). `href` pointe la fiche entité.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceEntiteDto {
    pub role: String,
    pub label: String,
    /// Absent when the edge points at an entity the registry no longer holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

/// One lot of a tender, as the notice publishes it. Distinct from
/// [`AnnonceLotDto`], which is an awarded lot and carries its holder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceLotOuvertDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nom: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The lot's own CPV, finer than the market's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpv: Option<String>,
}

/// Estimated value of the market, as the notice declares it. Never a sum of
/// lots, which notices do not reconcile with the total they announce.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceMontantDto {
    pub valeur: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devise: Option<String>,
}

/// Un lot attribué d'un résultat de marché. Le SIREN n'est porté que par les
/// avis eForms ; `href` pointe la fiche entité quand il existe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceLotDto {
    pub titulaire: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub montant: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devise: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nb_offres: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_attribution: Option<String>,
}

/// L'attribution d'un marché : le même objet que la consultation, à son
/// dernier moment. Sa présence est ce qui rend `AnnonceHit::attribue` vrai.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceResultatDto {
    #[serde(default)]
    pub lots: Vec<AnnonceLotDto>,
    /// Le paragraphe d'attribution des avis simplifiés, servi tel quel : la
    /// source y nomme l'attributaire sans rien structurer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texte: Option<String>,
    /// L'avis de résultat au BOAMP, quand la consultation et son attribution
    /// n'ont pas la même page source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avis_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceDetailResponse {
    pub annonce: AnnonceHit,
    /// The tender's allotment, what the notice publishes before any award.
    /// Empty means the notice says nothing, never that the market has one lot.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lots_ouverts: Vec<AnnonceLotOuvertDto>,
    /// Award criteria, weight in brackets when the notice gives one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub criteres: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub montant_estime: Option<AnnonceMontantDto>,
    #[serde(default)]
    pub liens: Vec<AnnonceLienDto>,
    #[serde(default)]
    pub canaux_detail: Vec<AnnonceCanalDto>,
    #[serde(default)]
    pub entites: Vec<AnnonceEntiteDto>,
    /// Le résultat du marché — porté par la fiche d'une attribution, et par
    /// celle d'une consultation dont le résultat est paru.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resultat: Option<AnnonceResultatDto>,
}

/// One entry of the crawlable catalogue, one per département.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceZoneEntry {
    pub code: String,
    pub label: String,
    pub count: i64,
}

/// Catalogue `/annonces` : le stock ouvert ventilé par département, plus les
/// natures — les chemins de crawl du fond.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnonceCatalogueResponse {
    pub fond: String,
    /// Open notices, every zone, the ones nothing located included.
    pub total: i64,
    pub zones: Vec<AnnonceZoneEntry>,
    #[serde(default)]
    pub familles: Vec<FacetChoice>,
}

/// Un groupe d'annonces sur la fiche entité. La clé est un fond quand
/// l'origine est le live DILA (`bodacc`, `joafe`), un rôle d'arête quand la
/// ligne est à nous (`acheteur` = marchés en cours, `titulaire` = marchés
/// remportés) : ce qui distingue deux groupes, c'est ce que l'entité est dans
/// chacun. `total` est le compte réel, `items` la tête de liste.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityAnnonceGroupDto {
    pub key: String,
    pub label: String,
    pub total: i64,
    pub items: Vec<AnnonceHit>,
}

// ── Keys of the public surface ──────────────────────────────────────────────

/// A key as the account screen shows it: never the secret, except at creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiKeySummary {
    /// Public handle, the fingerprint in hexadecimal. Knowing a hash does not
    /// give the key, and revoking demands the bearer's session.
    pub id: String,
    pub label: String,
    pub prefix: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiKeysResponse {
    pub items: Vec<ApiKeySummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiKeyCreate {
    pub label: String,
}

/// The only response that carries the secret. It is not stored in clear, so it
/// is not served twice.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiKeyCreated {
    pub secret: String,
    pub key: ApiKeySummary,
}

/// A licence word, or a file a component ships, with the text it opens when there is one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenSourceLink {
    pub label: String,
    pub href: Option<String>,
}

/// A third-party component of the published code: a file served as is or a compiled crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenSourceComponent {
    pub name: String,
    pub version: String,
    pub home: Option<String>,
    pub licence: Vec<OpenSourceLink>,
    pub files: Vec<OpenSourceLink>,
}

/// Response of `/internal/open-source`, sorted by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenSourceResponse {
    pub components: Vec<OpenSourceComponent>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn commentaire(volume: &str, issue: &str, pages: &str) -> Commentaire {
        let opt = |s: &str| (!s.is_empty()).then(|| s.to_string());
        Commentaire {
            kind: "note".to_string(),
            volume: opt(volume),
            issue: opt(issue),
            pages: opt(pages),
            author: None,
            date: None,
            title: None,
            publisher: None,
            url: None,
            body: None,
        }
    }

    /// HAL takes the three fields as free text, so the layout has to hold the
    /// forms it carries: `page_s` is a number, a range or its own label.
    #[test]
    fn reference_rend_ce_que_la_source_porte_et_rien_de_plus() {
        assert_eq!(
            commentaire("vol. 134", "n° 2", "p. 407-422")
                .reference()
                .as_deref(),
            Some("vol. 134, n° 2, p. 407-422")
        );
        assert_eq!(
            commentaire("134", "2", "407-422").reference().as_deref(),
            Some("vol. 134, n° 2, p. 407-422")
        );
        // A page that writes its own form keeps it, `p.` posed on neither.
        assert_eq!(
            commentaire("", "", "act. 843").reference().as_deref(),
            Some("act. 843")
        );
        assert_eq!(
            commentaire("", "", "1 et suivantes").reference().as_deref(),
            Some("1 et suivantes")
        );
        assert_eq!(
            commentaire("", "", "386 p.").reference().as_deref(),
            Some("386 p.")
        );
        // A roman volume is a volume; a label with no number after it is text.
        assert_eq!(
            commentaire("XIX", "", "").reference().as_deref(),
            Some("vol. XIX")
        );
        assert_eq!(
            commentaire("", "nouvelle série", "").reference().as_deref(),
            Some("n° nouvelle série")
        );
        assert_eq!(commentaire("", "", "").reference(), None);
    }

    /// Two consumers word the same gap, so the reasoning is locked here: a
    /// degenerate window never bounds it, and outside the coverage there is none.
    #[test]
    fn version_gap_ecarte_les_fenetres_degenerees_et_les_bords() {
        let l162_22_18 = [
            (Some("2010-12-22"), Some("2017-01-01")),
            (Some("2021-12-25"), Some("2023-12-31")),
            // Scheduled then dropped: its end sits before its start.
            (Some("2022-01-01"), Some("2021-12-25")),
            (Some("2024-01-01"), None),
        ];
        assert_eq!(version_gap(&l162_22_18, "2021-09-09"), Some((0, 1)));
        assert_eq!(version_gap(&l162_22_18, "2011-06-01"), None);
        assert_eq!(version_gap(&l162_22_18, "2026-01-01"), None);

        assert!(gap_explique("ABROGE") && gap_explique("ABROGE_DIFF"));
        assert!(gap_explique("TRANSFERE"));
        assert!(!gap_explique("MODIFIE") && !gap_explique("MODIFIE_MORT_NE"));
    }

    #[test]
    fn instrument_with_de_contracte_selon_le_premier_mot() {
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
        let req: SearchRequest = serde_json::from_str(r#"{"query":"bail commercial"}"#).unwrap();
        assert_eq!(req.query, "bail commercial");
        assert_eq!(req.mode, SearchMode::Auto);
        assert_eq!(req.sort, SortOrder::Relevance);
        assert_eq!(req.limit, 20);
        assert_eq!(req.offset, 0);
        assert!(!req.ai_mode);
        assert!(req.jurisdiction_type.is_none());

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
            formation: Vec::new(),
            legal_domain: None,
            seat_path: None,
            publication_codes: vec!["B".into()],
            chars: Some(4200),
            summary: None,
            summary_quote: None,
            summary_body: None,
            official_url: Some(
                "https://www.legifrance.gouv.fr/ceta/id/CETATEXT000050414461".into(),
            ),
            cited_articles: BTreeMap::from([("code-civil".into(), vec!["1240".into()])]),
            matched_docket: None,
        };
        let v = serde_json::to_value(&hit).unwrap();
        assert_eq!(v["jurisdictionType"], "CE");
        assert_eq!(v["citedArticles"]["code-civil"][0], "1240");
        assert_eq!(v["titleHtml"], "<b>x</b>");
        assert_eq!(v["bestChunk"]["chunkIndex"], 0);
        assert_eq!(v["solution"]["key"], "REJET");
        assert_eq!(v["solution"]["label"], "Rejet");
        assert!(v.get("summary").is_none());
        assert!(v.get("procedure").is_none());
    }

    #[test]
    fn facet_choice_parent_optional() {
        let flat: FacetChoice =
            serde_json::from_str(r#"{"value":"REJET","label":"Rejet","count":3}"#).unwrap();
        assert_eq!(flat.parent, None);
        assert_eq!(flat.hint, None);
        let child = FacetChoice {
            value: "tj76351".into(),
            label: "Le Havre".into(),
            count: 2,
            parent: Some("jurisdiction_type:TJ".into()),
            hint: Some("Ancien tribunal de grande instance.".into()),
        };
        let v = serde_json::to_value(&child).unwrap();
        assert_eq!(v["parent"], "jurisdiction_type:TJ");
        assert_eq!(v["hint"], "Ancien tribunal de grande instance.");
        let flat_json = serde_json::to_value(&flat).unwrap();
        assert!(flat_json.get("parent").is_none());
        assert!(flat_json.get("hint").is_none());
    }

    #[test]
    fn legi_article_response_serializes_camel_case() {
        let art = LawArticleResponse {
            upcoming_version_date: None,
            breadcrumb: Vec::new(),
            legiarti: "LEGIARTI000006832947".into(),
            legitext: "LEGITEXT000006070721".into(),
            code: "code-civil".into(),
            jurisdiction: "FR".into(),
            jurisdiction_label: "France".into(),
            code_title: Some("Code civil".into()),
            num: "L131-4".into(),
            num_key: "L131-4".into(),
            etat: "VIGUEUR".into(),
            date_debut: "1992-05-15".into(),
            source: "legifrance".into(),
            titre_text: Some("Livre Ier > Titre III".into()),
            date_fin: None,
            expired: false,
            texte: Some("Le contrat…".into()),
            texte_spans: vec![],
            texte_original: None,
            lang_original: None,
            translation: "officiel".into(),
            source_asof: Some("2026-06-28".into()),
            source_authority: "source gouvernementale".into(),
            source_official: true,
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
            cited_by: Vec::new(),
            commentaires: Vec::new(),
            travaux_parlementaires: Vec::new(),
            applicability: None,
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
        assert!(v.get("dateFin").is_none());
        assert_eq!(v["nota"], "Conformément à la loi n° 2019-222…");
    }

    #[test]
    fn citing_decision_hit_serializes_camel_case() {
        let hit = CitingDecisionHit {
            id: "cass-1".into(),
            title: "Cour de cassation, 2024-01-10, 22-12.345".into(),
            jurisdiction_type: JurisdictionType::Cc,
            jurisdiction_code: Some("cc".into()),
            date_lecture: Some("2024-01-10".into()),
            publication_codes: vec!["r".into()],
            summary: None,
            cited_articles: vec!["748-6".into(), "748-7".into()],
        };
        let v = serde_json::to_value(&hit).unwrap();
        assert_eq!(v["jurisdictionType"], "CC");
        assert_eq!(v["dateLecture"], "2024-01-10");
        assert_eq!(v["publicationCodes"][0], "r");
        assert_eq!(v["citedArticles"][1], "748-7");
    }
}
