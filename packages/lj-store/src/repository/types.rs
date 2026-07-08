//! Types publics du repository : structs/enums de contrat (lignes de résultat,
//! bundles d'écriture, statuts d'upsert) consommés par `lj-api`/`lj-ingest`/
//! `lj-bench`. Réexportés tels quels par [`super`].

use chrono::NaiveDate;
use lj_core::decision::Decision;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertStatus {
    Created,
    Updated,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpsertResult {
    pub id: i64,
    pub status: UpsertStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingDecisionState {
    pub id: i64,
    pub source_uid: String,
    pub content_checksum: String,
    pub has_embeddings: bool,
    pub public_id: Option<String>,
}

/// Une ligne du backfill summary (port du tuple `iter_decisions_missing_summary`).
///
/// `date_lecture` est rendu en `text` côté SQL (`::text`) pour matcher le
/// `row[4]` Python (str ISO ou `None`) — l'appelant reconstruit le titre à
/// partir de ces champs bruts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingSummaryRow {
    pub decision_id: i64,
    pub public_id: String,
    pub juridiction_type: String,
    pub jurisdiction_name: Option<String>,
    pub date_lecture: Option<String>,
    pub docket_numbers: Option<Vec<String>>,
}

/// Corps + métadonnées grain décision d'une décision poolée pour le GT
/// (`lj-bench gt-pool`). `full_text` est le texte indexé (ADR 0085) ; les autres
/// champs alimentent le `_meta_<slug>.yaml` lu par l'agent `search-gt-builder`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GtDoc {
    pub id: i64,
    pub public_id: String,
    pub jurisdiction_name: String,
    pub juridiction_type: String,
    pub date_lecture: String,
    pub search_title: String,
    pub full_text: String,
}

/// Un fichier sitemap prêt à publier en base (`sitemaps`). `body` est le
/// contenu servi tel quel (`.xml` brut ou `.xml.gz` déjà gzippé).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SitemapRow {
    pub filename: String,
    pub content_type: String,
    pub body: Vec<u8>,
    pub lastmod: NaiveDate,
}

/// Ligne `legal_text` (catalogue des textes de loi, ADR 0112 §1). `text_uid` =
/// IDENTITÉ globalement unique (ex-`source_uid` : LEGITEXT/JORFTEXT/CELEX/EU…),
/// `source` HORS identité (provenance dérivée des versions). `slug` = code court
/// pour l'URL. `title_key` = `normalize_instrument(title)`, posé côté Rust à
/// l'upsert (clé du fold de titres du linker, ADR 0145). `date_texte` = date du
/// texte (par quoi on cite) ; `date_publi` = publication JO. Dates `NaiveDate` au
/// bord store ; le parser pur `lj-core` émet des ISO `String` (sentinelles → None).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegalTextRow {
    pub text_uid: String,
    pub jurisdiction: String,
    pub title: String,
    pub title_key: String,
    pub nature: String,
    pub last_modified: Option<NaiveDate>,
    pub date_texte: Option<NaiveDate>,
    pub date_publi: Option<NaiveDate>,
    /// Identité canonique diffuseur-agnostique (ADR 0115), cascade `eli → nor →
    /// instrument_key`. `eli` = `<ID_ELI>` (autoritaire, ~20 %) ; `nor` = `<NOR>`
    /// (cross-diffuseur, ~80 %) ; `instrument_key` = `nature|date|num` (filet). Tous
    /// nullables : un texte peut n'en porter aucun (codes : identité = slug). Servent
    /// le collapse des manifestations LEGI/JORF d'un même acte en une `legal_text`.
    pub eli: Option<String>,
    pub nor: Option<String>,
    pub instrument_key: Option<String>,
}

/// Ligne `legal_article` (une version datée d'un article de loi, ADR 0112 §1).
/// IDENTITÉ = `(text_uid, num_key, date_debut)`. PROVENANCE par version (hybride) :
/// `source` le fournisseur (legifrance/jorf/gisti/onu…), `source_uid` l'identifiant
/// natif (LEGIARTI/JORFARTI/`treaty/…`, le « CID » d'où l'URL Légifrance se dérive,
/// ADR §Principe 3), `source_url` l'URL **non** template-dérivable (curé, nullable).
/// `position` = ordre de lecture réel (≠ tri lexical 26<26-1<26-2). `content_checksum`
/// xxh3-64 en `BIGINT` (cast `i64::from_ne_bytes`). `date_debut` `None` côté write →
/// sentinelle '0001-01-01' (borne ouverte) à l'upsert ; en lecture la colonne est
/// NOT NULL. `date_fin` `None` = pas de fin. Sentinelles normalisées au parsing (#12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegalArticleRow {
    pub text_uid: String,
    pub num: String,
    pub num_key: String,
    pub position: Option<i32>,
    pub title_path: Option<String>,
    pub status: String,
    pub date_debut: Option<NaiveDate>,
    pub date_fin: Option<NaiveDate>,
    pub texte: Option<String>,
    /// Corps dans la langue d'origine (ADR 0116), si disponible — couche front/
    /// vérification, souvent `None`. `BM25` ne couvre que `texte` (FR).
    pub texte_original: Option<String>,
    /// Langue de `texte_original` (ISO-639-1, `ar`…). `None` si pas d'original.
    pub lang_original: Option<String>,
    /// Provenance de `texte` (ADR 0116) : `officiel` / `non_officiel` / `automatique`.
    pub translation: String,
    pub nota: Option<String>,
    pub content_checksum: u64,
    pub source: String,
    pub source_uid: String,
    pub source_url: Option<String>,
    /// Date « as-of » de fraîcheur (ADR 0129) : dernière base crédible que cette copie
    /// reflète le droit en vigueur. `None` pour les sources *live* (legifrance/kali/jorf)
    /// dont la fraîcheur se dérive de `ingest_freshness` ; posée pour le curé/étranger.
    pub source_asof: Option<NaiveDate>,
    /// Source secondaire amont (ADR 0129) : le site qu'un agrégateur (jafbase) pointe.
    /// `None` le plus souvent.
    pub source_upstream_url: Option<String>,
}

/// Une décision citant un article de référentiel (backlink `legal_citation`,
/// ADR 0145). Champs bruts ; la conversion vers le DTO
/// `CitingDecisionHit` (`JuridictionType`, titre) est faite côté `lj-api`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitingDecisionRow {
    pub id: i64,
    pub public_id: String,
    pub juridiction_type: String,
    pub jurisdiction_name: Option<String>,
    pub date_lecture: Option<String>,
    pub docket_numbers: Option<Vec<String>>,
}

/// Un hit de recherche plein-texte d'article (ADR 0114, `/recherche-textes`).
/// `slug` = slug du `legal_text` parent (lien `/loi/{slug}/{num}`) ; `texte` =
/// corps brut pour le snippet (calculé côté `lj-api`). `score` = `paradedb.score`.
#[derive(Debug, Clone, PartialEq)]
pub struct ArticleSearchRow {
    pub text_uid: String,
    pub slug: Option<String>,
    pub code_title: String,
    pub num: String,
    pub num_key: String,
    pub title_path: Option<String>,
    pub status: String,
    pub source: String,
    pub texte: Option<String>,
    pub score: f32,
}

/// Un comptage de facette de recherche d'articles (ADR 0114) : valeur de la facette
/// (jurisdiction/nature/source) et nombre de hits sous le même prédicat BM25 + filtres.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetCount {
    pub value: String,
    pub count: i64,
}

/// Total + facettes d'une recherche d'articles (ADR 0114), calculés en UNE requête
/// `GROUPING SETS` sous le même prédicat BM25 + filtres que la recherche.
/// `jurisdiction`/`nature` portés par `legal_text` (`nature` normalisée `upper()`),
/// `source` par `legal_article`. Chaque axe trié count décroissant puis valeur
/// ascendante. Mapping vers `ArticleSearchFacets` côté `lj-api`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleSearchStats {
    pub total: i64,
    pub jurisdiction: Vec<FacetCount>,
    pub nature: Vec<FacetCount>,
    pub source: Vec<FacetCount>,
}

/// Une entrée du catalogue des codes (ADR 0114, `/codes`) : un `legal_text` à slug +
/// nombre d'articles en vigueur. `text_uid` = identité globale, `slug` = code court de
/// l'URL `/loi/{slug}`. Mapping vers `CodeCatalogueEntry` côté `lj-api`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegalTextCatalogRow {
    pub text_uid: String,
    pub slug: String,
    pub title: String,
    pub nature: String,
    pub jurisdiction: String,
    pub article_count: i64,
}

/// Une entrée de la table des matières d'un code (ADR 0114, sommaire) : version en
/// vigueur par `num_key`, sans corps (clic = navigation `/loi/{slug}/{num}`).
/// `position` = ordre de lecture réel (≠ tri lexical) ; mapping vers `TocEntry` côté
/// `lj-api`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocArticleRow {
    pub num: String,
    pub num_key: String,
    pub title_path: Option<String>,
    pub status: String,
    pub position: Option<i32>,
}

/// Un article voisin pour le contexte de lecture (ADR 0114) : numéro + état, sans
/// corps (clic = navigation `/loi/{slug}/{num}`). `current` marque l'article de la
/// page courante.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleNeighborRow {
    pub num: String,
    pub num_key: String,
    pub status: String,
    pub current: bool,
}

/// Une version d'article dans la timeline (ADR 0097). Dates en ISO `String`
/// (`::text` côté SQL) ; mapping vers `LawArticleVersion` côté `lj-api`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LawVersionRow {
    pub source_uid: String,
    pub status: String,
    pub date_debut: Option<String>,
    pub date_fin: Option<String>,
}

/// Sommaire d'un code (ADR 0097) avec le décompte d'articles en vigueur.
/// `last_modified` en ISO `String` ; mapping vers `LawCodeSummary` côté `lj-api`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LawCodeSummaryRow {
    pub text_uid: String,
    pub slug: Option<String>,
    pub title: String,
    pub nature: String,
    pub last_modified: Option<String>,
    pub article_count: i64,
}

/// Un chunk prêt à insérer dans `decision_chunks`. `embedding` peut être `None`
/// (mode ingest sans backend embed ; la colonne `rabitq8(1024)` accepte NULL).
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkWrite {
    pub chunk_index: i32,
    pub char_start: i32,
    pub char_end: i32,
    pub body: String,
    pub embedding: Option<Vec<f32>>,
}

/// Une occurrence de citation v9 (ADR 0143/0145) prête à écrire dans
/// `legal_citation` : span token en codepoints sur `decisions.full_text` +
/// cible catalogue inline (`None` = non lié — posée par le linker in-pass,
/// M3′). Livrées par décision TRIÉES par `char_start`, sans doublon ni
/// chevauchement (dédup déterministe de l'aplatissement lj-extract) — la PK
/// `(decision_id, char_start)` en dépend, violation = bug amont.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitationOccurrenceRow {
    pub char_start: i32,
    pub char_end: i32,
    pub ref_text_uid: Option<String>,
    pub ref_num_key: Option<String>,
}

/// Métadonnées catalogue d'un `legal_text` pour le banc offline :
/// `(title_key, slug, num)`. `num` = numéro d'acte en tête de titre (`Décret n°
/// 2000-1093 …` → `2000-1093`, `None` si non numéroté), qui distingue le faux-lien
/// daté (même `title_key` date-rayé, numéros différents) du fanout version bénin.
pub type LegalTextMeta = (String, Option<String>, Option<String>);

/// Champs structurés extraits d'une décision, prêts à persister. Construits à
/// l'ingest (`lj_ingest::extract`), plus jamais par lj-store (ADR 0123 §3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractedFields {
    pub jurisdiction_name: Option<String>,
    pub date_lecture: Option<NaiveDate>,
    pub date_audience: Option<NaiveDate>,
    pub docket_numbers: Vec<String>,
    pub formation_or_chamber: Option<String>,
    /// Formation structurée (ADR 0170) : position recomposée affichable +
    /// uids référentiels `chambre:*` / `formation:*` (FK `facet_value`).
    pub chamber_position: Option<String>,
    pub chambre_uid: Option<String>,
    pub formation_uid: Option<String>,
    pub publication_codes: Vec<String>,
    /// Uids référentiels (ADR 0146/0148, v12 : émis par les scanners) — FK
    /// vers `facet_value`.
    pub solution_uid: Option<String>,
    pub voie_uid: Option<String>,
    pub office_uid: Option<String>,
    pub legal_domain_uid: Option<String>,
    pub publication_uid: Option<String>,
    /// Matrice NER {applicant, defendant} × {counsel_names, law_firms,
    /// companies} (#29, directive 2026-07-03).
    pub applicant_counsel_names: Vec<String>,
    pub applicant_law_firms: Vec<String>,
    pub applicant_companies: Vec<String>,
    pub defendant_counsel_names: Vec<String>,
    pub defendant_law_firms: Vec<String>,
    pub defendant_companies: Vec<String>,
    /// Thèmes Judilibre verbatim (`source_fields->'themes'`, ADR 0159).
    pub themes: Vec<String>,
    /// Ligne du référentiel `jurisdiction` portée par la décision : le code
    /// devient `decisions.jurisdiction_code` (FK), la ligne est créée à la
    /// volée (`ON CONFLICT DO NOTHING`) par le chemin d'écriture.
    pub jurisdiction: Option<JurisdictionRow>,
    /// Occurrences de citations à plat (ADR 0145), `None` si non extraites.
    pub citation_occurrences: Option<Vec<CitationOccurrenceRow>>,
    /// Liens de chronologie (ADR 0161), `None` si non extraits.
    pub decision_links: Option<Vec<DecisionLinkRow>>,
    /// Citations de jurisprudence (ADR 0165), `None` si non extraites.
    pub case_citations: Option<Vec<CaseCitationRow>>,
}

/// Ligne `case_citation` (ADR 0165) prête à écrire : span (offsets
/// codepoints, convention 0143 : token identifiant) + clé pendante par
/// famille (`cc|1823954`, `cjue|c-561/19`, `rg|{jurisdiction_code}|21/04532`,
/// `af|{jurisdiction_code}|1906041` fond administratif TA/CAA…).
/// Livrées par décision TRIÉES par `char_start`, sans chevauchement — la PK
/// `(decision_id, char_start)` en dépend, violation = bug amont.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseCitationRow {
    pub char_start: i32,
    pub char_end: i32,
    pub target_ref: String,
}

/// Ligne `decision_links` (ADR 0161) prête à écrire : type de lien + clé
/// pendante `canonical_ref` de la décision attaquée. Miroir store de
/// `lj_extract::chrono::PriorRef` (lj-store ne tire pas lj-extract).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionLinkRow {
    /// `APPEL_DE` | `POURVOI_CONTRE` | `RENVOI_APRES_CASSATION`.
    pub link_type: String,
    pub target_ref: String,
}

/// Ligne du référentiel `jurisdiction` (ADR 0146). Miroir store de
/// `lj_extract::facets::JurisdictionRef` (lj-store ne tire pas lj-extract).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JurisdictionRow {
    pub code: String,
    pub juridiction_type: String,
    pub city: Option<String>,
    pub label: String,
}

/// Ligne du référentiel `facet_value` (ADR 0146, migration 0100) : uid
/// namespacé (`solution:REJET`…), libellé FR, hiérarchie (`parent_uid`) et
/// ordre d'affichage (`sort` du seed). Consommée par le cache référentiel de
/// `lj-api`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetValueRow {
    pub uid: String,
    pub facet: String,
    pub label: String,
    pub abbr: Option<String>,
    pub parent_uid: Option<String>,
    pub sort: i32,
}

/// Bundle d'écriture pour une décision (utilisé par le pipeline).
#[derive(Debug, Clone)]
pub struct BulkDecisionWrite {
    pub decision_id: Option<i64>,
    pub public_id: String,
    pub decision: Decision,
    pub content_checksum: String,
    /// `canonical_ref` (ADR 0100) calculé à l'ingest (`lj_ingest::extract`), passé
    /// à l'upsert : lj-store ne tire plus l'extracteur (ADR 0123 §3).
    pub canonical_ref: Option<String>,
    pub write_mode: String,
    pub chunks: Vec<ChunkWrite>,
    /// `xml` (opendata) | `json` (judilibre) — le payload brut n'est plus
    /// stocké (ADR 0085) ; seul le format est persisté (`set_payload_format`).
    pub payload_format: String,
    pub extracted: Option<ExtractedFields>,
    /// Payload source moins le texte, offsets rebasés sur `full_text` (ADR 0085).
    pub source_fields: Value,
    /// Version chunk+embed des chunks écrits (`None` = écrits sans embeddings).
    pub embed_version: Option<i16>,
}
