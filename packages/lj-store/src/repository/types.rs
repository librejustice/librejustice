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
    pub jurisdiction_type: String,
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
    pub jurisdiction_type: String,
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
    /// Corps monolithique d'un texte SANS articles numérotés (circulaires,
    /// réponses ministérielles… — ADR 0196). `None` pour les familles à articles.
    pub body: Option<String>,
    /// État de diffusion du texte lui-même (`VIGUEUR`/`ABROGE`, familles sans
    /// articles porteurs de statut — ADR 0196). `None` = non renseigné.
    pub status: Option<String>,
}

/// Texte sans slug, avec tout ce que la cascade de désambiguïsation de la
/// passe d'assignation peut mobiliser (ADR 0206). `date_texte` déjà rendue
/// `YYYY-MM-DD` (suffixe prêt à l'emploi).
#[derive(Debug, Clone)]
pub struct SlugSourceRow {
    pub text_uid: String,
    pub title: String,
    pub jurisdiction: String,
    pub date_texte: Option<String>,
    pub nor: Option<String>,
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

/// Propriétaire d'arêtes `legal_link` (ADR 0174) : la version d'article dont le
/// XML porte le bloc `<LIENS>`, ou le texte lui-même (`num_key` vide,
/// `date_debut` `None` → sentinelle '0001-01-01' à l'écriture, aligné
/// `legal_article`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegalLinkOwner {
    pub text_uid: String,
    pub num_key: String,
    pub date_debut: Option<NaiveDate>,
}

/// Ligne `legal_link` (ADR 0174) : miroir fidèle d'un `<LIEN>` DILA. `typelien`
/// brut + famille `verb` normalisée + `direction` vue de l'owner (`outgoing` =
/// il agit/cite, `incoming` = il subit/est cité). Cible en clé pendante (IDs
/// DILA), `target_num_key` = `normalize_article(target_num)` posé au bord
/// ingest pour la résolution des liens sans ID. `seq` = ordre du fichier,
/// implicite (position dans le `Vec` du propriétaire).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegalLinkRow {
    pub typelien: String,
    pub verb: String,
    pub direction: String,
    pub target_kind: String,
    pub target_uid: Option<String>,
    pub target_text_uid: Option<String>,
    pub target_num: Option<String>,
    pub target_num_key: Option<String>,
    pub target_nature: Option<String>,
    pub target_label: String,
    pub target_date: Option<NaiveDate>,
    pub target_nor: Option<String>,
}

/// Une arête `legal_link` lue pour affichage, cible résolue au read-time
/// (ADR 0174) : `target_text_slug`/`target_text_title` = le texte porteur de la
/// cible s'il est en base ; `resolved_slug`/`resolved_num_key` = la version
/// d'article cible résolue (par `source_uid`, sinon par `(texte, num_key)`) —
/// `Some` ⇒ le lien `/texte/{resolved_slug}/{resolved_num_key}` est garanti.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLegalLink {
    pub typelien: String,
    pub verb: String,
    pub direction: String,
    pub target_kind: String,
    pub target_num: Option<String>,
    pub target_nature: Option<String>,
    pub target_label: String,
    pub target_date: Option<NaiveDate>,
    /// Uid du texte porteur de la cible (JORFTEXT d'une loi modificatrice…) —
    /// compose le lien travaux parlementaires (ADR 0215).
    pub target_text_uid: Option<String>,
    pub target_text_slug: Option<String>,
    pub target_text_title: Option<String>,
    pub resolved_slug: Option<String>,
    pub resolved_num_key: Option<String>,
    /// Cible section : cid stable résolu via `legal_toc_edge` (ADR 0207) —
    /// `Some` ⇒ l'ancre `/texte/{target_text_slug}#{cid}` existe au sommaire.
    pub resolved_section_cid: Option<String>,
}

/// Propriétaire d'arêtes `legal_toc_edge` (ADR 0207) : le cid du texte
/// (fichier `texte/struct`) ou l'ID de version d'une section (`section_ta`).
/// `text_uid` = texte porteur (dénormalisé pour la purge et l'ancrage CTE).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocOwner {
    pub owner_uid: String,
    pub text_uid: String,
}

/// Ligne `legal_toc_edge` (ADR 0207) : un enfant (article ou section) tel que
/// listé par son propriétaire, fenêtre `[date_debut, date_fin)` (sentinelles
/// de fin absorbées côté parser ; un `date_debut` sentinelle reste une vraie
/// date → exclu de tout filtrage daté). `seq` implicite (position dans le `Vec`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocEdgeRow {
    pub child_kind: String,
    pub child_uid: String,
    pub child_cid: Option<String>,
    pub child_num_key: Option<String>,
    pub label: String,
    pub etat: String,
    pub date_debut: Option<NaiveDate>,
    pub date_fin: Option<NaiveDate>,
    pub niv: Option<i32>,
}

/// Un nœud de l'arbre structurel aplati en ordre de lecture (CTE récursive,
/// ADR 0207) : `depth` = profondeur (1 = premier niveau du texte).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocTreeRow {
    pub depth: i32,
    pub child_kind: String,
    pub child_uid: String,
    pub child_cid: Option<String>,
    pub child_num_key: Option<String>,
    pub label: String,
    pub etat: String,
}

/// Un item de vue-lecture d'une section (ADR 0207) : les nœuds du sous-arbre
/// en ordre de lecture, corps (`texte`/`nota`) joint pour les articles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocReadingRow {
    pub depth: i32,
    pub child_kind: String,
    pub child_cid: Option<String>,
    pub child_num_key: Option<String>,
    pub label: String,
    pub etat: String,
    pub texte: Option<String>,
    pub nota: Option<String>,
}

/// Une décision citant un article de référentiel (backlink `legal_citation`,
/// ADR 0145). Champs bruts ; la conversion vers le DTO
/// `CitingDecisionHit` (`JurisdictionType`, titre) est faite côté `lj-api`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitingDecisionRow {
    pub id: i64,
    pub public_id: String,
    pub jurisdiction_type: String,
    pub jurisdiction_name: Option<String>,
    pub date_lecture: Option<String>,
    pub docket_numbers: Option<Vec<String>>,
    /// Codes de publication (`b`, `A`…) — portée jurisprudentielle côté `lj-api`
    /// (`lj_core::publication::significance_key`).
    pub publication_codes: Option<Vec<String>>,
    /// Résumé de la décision (carte citante, première phrase côté front).
    pub summary: Option<String>,
}

/// Un article co-cité avec l'article de la page dans les décisions
/// (croisement `legal_citation`, plan graphe Phase D). `count` = décisions de
/// l'échantillon citant les deux ; `text_slug` `None` = texte sans page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoCitedArticleRow {
    pub num_key: String,
    pub count: i64,
    pub text_slug: Option<String>,
    pub text_title: String,
}

/// Un hit de recherche plein-texte d'article (ADR 0114, `/recherche-textes`).
/// `slug` = slug du `legal_text` parent (lien `/texte/{slug}/{num}`) ; `texte` =
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

/// Un hit du banc de ranking articles (`lj-bench article-rank-eval`) :
/// identité publique du doc (`slug` + `num`, `num` vide = jambe « textes à
/// corps ») + matière de jugement (titre, chemin de division, corps).
#[derive(Debug, Clone, PartialEq)]
pub struct ArticleRankHit {
    pub slug: String,
    pub num: String,
    pub code_title: String,
    pub title_path: Option<String>,
    pub texte: Option<String>,
}

/// Mode de la jambe titre du banc de ranking articles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArticleTitleMode {
    /// OR boosté ×4 (prod actuelle).
    Or,
    /// Conjonctif par alternative (requête nue / expansion) boosté ×4.
    Conj,
    /// Conjonctif ×4 + OR ×1 : le conjonctif prime quand il matche, l'OR
    /// reste un filet pour les requêtes à mots-outils.
    OrConj,
}

/// Paramètres de la fusion RRF inter-jambes du banc articles : chaque jambe
/// est classée seule (top-`leg_limit` par score BM25), puis fusionnée par
/// `w_jambe / (k + rang)` — `txt_weight` pondère les jambes « textes à
/// corps » (1.0 = interleave strict). `split_title` scinde titre et corps en
/// jambes séparées (4 jambes : art-titre, art-corps, txt-titre, txt-corps,
/// scores RRF sommés par doc — le schéma de la recherche décisions) au lieu
/// des 2 jambes à prédicat mixte titre+corps. `container` ajoute une jambe
/// « conteneurs » : `legal_text` sans corps mais à articles (les codes), titre
/// en match conjonctif SEUL — un hit `slug` sans `num` (lien `/texte/{slug}`),
/// prioritaire à rang RRF égal (requête navigationnelle « code de … »).
/// `foreign_weight` scinde la jambe articles en domestique (jurisdiction
/// FR/UE/INTL + pays nommés dans la requête,
/// [`lj_core::jurisdictions::query_jurisdictions`]) et étrangère, pondérée
/// par ce poids — les codes napoléoniens étrangers matchent mot pour mot les
/// requêtes françaises et trustent le top sans ce prior. `container_alias`
/// admet dans la jambe conteneurs les expansions d'alias **pleine requête**
/// ([`lj_core::aliases::whole_query_expansions`] : « code civil du sénégal »
/// → « code de la famille sénégalais »). `country_leg` ajoute une jambe
/// « pays nommé » : articles des juridictions nommées, requête débarrassée
/// des tokens pays ([`lj_core::jurisdictions::strip_query_jurisdictions`]) —
/// les articles de fond étrangers ne contiennent pas le nom de leur pays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArticleRrf {
    pub k: f64,
    pub txt_weight: f64,
    pub leg_limit: i64,
    pub split_title: bool,
    pub container: bool,
    pub foreign_weight: Option<f64>,
    pub container_alias: bool,
    pub country_leg: bool,
    /// Fusionne la jambe étrangère dans la jambe articles par SCORE
    /// (`score × foreign_weight`) au lieu de l'interleave par rang — même
    /// index BM25, scores comparables : équivalent mathématique d'une requête
    /// Tantivy unifiée à branche étrangère `paradedb.boost`. Sans effet si
    /// `foreign_weight` est `None`. NB : l'échelle diffère du poids par rang
    /// (score ×0,10 ≈ rang w 0,25).
    pub foreign_score_merge: bool,
    /// Jambe « termes d'usage » optionnelle (poids RRF) : sacs de n-grammes
    /// des contextes de citation (`scratch_usage_terms`, working-note
    /// 2026-07-20) — le vocabulaire que les décisions emploient autour d'un
    /// article, absent de sa lettre (« frais irrépétibles » → art. 700 CPC).
    pub usage_weight: Option<f64>,
    /// Table des sacs interrogée par la jambe usage (défaut :
    /// `legal_article_usage`). Permet de comparer des recettes de sacs
    /// alternatives matérialisées en tables scratch, à jambe identique.
    pub usage_table: Option<&'static str>,
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
/// `code` (slug) / `jurisdiction` / `nature` portés par `legal_text` (`nature`
/// normalisée `upper()`), `source` par `legal_article`. Chaque axe trié count
/// décroissant puis valeur ascendante. Mapping vers `ArticleSearchFacets` côté `lj-api`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleSearchStats {
    pub total: i64,
    /// Slugs des textes porteurs de hits — token du filtre `code` (`/texte/{slug}`).
    pub code: Vec<FacetCount>,
    pub jurisdiction: Vec<FacetCount>,
    pub nature: Vec<FacetCount>,
    pub source: Vec<FacetCount>,
}

/// Une entrée du catalogue des codes (ADR 0114, `/codes`) : un `legal_text` à slug +
/// nombre d'articles en vigueur. `text_uid` = identité globale, `slug` = code court de
/// l'URL `/texte/{slug}`. Mapping vers `CodeCatalogueEntry` côté `lj-api`.
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
/// vigueur par `num_key`, sans corps (clic = navigation `/texte/{slug}/{num}`).
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
/// corps (clic = navigation `/texte/{slug}/{num}`). `current` marque l'article de la
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
    /// Dates programmées de versions futures du texte (`VERSIONS_A_VENIR`,
    /// ADR 0178) ; peut contenir la sentinelle `2222-02-22` (date inconnue).
    pub upcoming_versions: Vec<NaiveDate>,
    /// Corps monolithique d'un texte sans articles (ADR 0196).
    pub body: Option<String>,
    /// État de diffusion (`VIGUEUR`/`ABROGE`, ADR 0196).
    pub status: Option<String>,
    pub nor: Option<String>,
    /// Date de signature ISO.
    pub date_texte: Option<String>,
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
    /// Span porteur de « et suivants » (ADR 0226) : la cible désigne une
    /// famille d'articles à partir de l'ancre — expansée dans les arrays de
    /// facettes via `_suivants_family`.
    pub suivants: bool,
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
    pub date_lecture: Option<NaiveDate>,
    pub date_audience: Option<NaiveDate>,
    pub docket_numbers: Vec<String>,
    /// Formation structurée (ADR 0170) : position recomposée affichable +
    /// uids référentiels `chamber:*` / `formation:*` (FK `facet_value`).
    pub chamber_position: Option<String>,
    pub chamber_uid: Option<String>,
    pub formation_uid: Option<String>,
    /// Titre canonique persistant (ADR 0170 ét.5) : composé à l'ingest via
    /// `lj_core::titles` (juridiction, siège, date FR, premier numéro) —
    /// colonne simple `decisions.search_title`, champ indexé BM25.
    pub search_title: Option<String>,
    pub publication_codes: Vec<String>,
    /// Uids référentiels (ADR 0146/0148, v12 : émis par les scanners) — FK
    /// vers `facet_value`.
    pub solution_uid: Option<String>,
    pub procedure_uid: Option<String>,
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
    /// Intervenants (rôle intervenant, ontologie 0180), sans côté.
    pub intervenors: Vec<String>,
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
    /// Acteurs au grain `decision_party` (ADR 0182) — la relation canonique
    /// dont les 7 cellules NER plates ci-dessus sont la projection. `None` si
    /// non extraits.
    pub parties: Option<Vec<DecisionPartyRow>>,
}

/// Ligne `decision_party` (ADR 0182) prête à écrire : un acteur de la
/// décision — qualité × côté × valeur, clé de résolution pliée, nature (axe 1
/// ontologie 0180, `None` = moteur muet) et spans-évidences en tableaux
/// parallèles (codepoints sur `full_text`, convention 0143 ; vides =
/// provenance métadonnée sans occurrence corps). Miroir store de
/// `lj_extract::parties::ActorRow` (lj-store ne tire pas lj-extract).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionPartyRow {
    /// `party` | `law_firm` | `counsel_name` (la qualité `intervenor` est
    /// gatée en prod, ADR 0182 §7).
    pub quality: String,
    /// `applicant` | `defendant` | `None`.
    pub side: Option<String>,
    pub value: String,
    pub resolve_key: String,
    /// `physique` | `morale_privee` | `morale_publique`.
    pub nature: Option<String>,
    /// Slug officiel du barreau en apposition (`counsel_name`, ADR 0188).
    pub barreau: Option<String>,
    /// `substituant` | `substitue` | `postulant` | `plaidant` (`counsel_name`,
    /// ADR 0194) — `None` = aucun marqueur.
    pub role: Option<String>,
    pub char_starts: Vec<i32>,
    pub char_ends: Vec<i32>,
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

/// Ligne `text_case_citation` (ADR 0196 §5) : un texte/article du référentiel
/// cite une décision. Émetteur = article `(owner_num_key, owner_date_debut)`
/// ou, si les deux sont `None`, le corps `legal_text.body` du texte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextCaseCitationRow {
    pub owner_num_key: Option<String>,
    pub owner_date_debut: Option<NaiveDate>,
    pub char_start: i32,
    pub char_end: i32,
    /// Clé pendante par famille, même format que `case_citation` (ADR 0165).
    pub target_ref: String,
}

/// Ligne `text_legal_citation` (ADR 0217) : un texte/article du référentiel
/// renvoie à un article de norme. Émetteur = article `(owner_num_key,
/// owner_date_debut)` ou, si les deux sont `None`, le corps
/// `legal_text.body`. Toujours résolue (`ref_text_uid` du catalogue) ;
/// `ref_num_key` `None` = mention nue du texte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextLegalCitationRow {
    pub owner_num_key: Option<String>,
    pub owner_date_debut: Option<NaiveDate>,
    pub char_start: i32,
    pub char_end: i32,
    pub ref_text_uid: String,
    pub ref_num_key: Option<String>,
}

/// Span de renvoi résolu d'une version d'article servie (ADR 0217), prêt à
/// composer côté API : offsets codepoints sur le `texte` émetteur + cible
/// jointe au catalogue (slug pour le href, titre pour le libellé).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleCitationSpanRow {
    pub char_start: i32,
    pub char_end: i32,
    pub ref_slug: Option<String>,
    pub ref_num_key: Option<String>,
    pub ref_title: String,
    /// Le texte cible a ≥ 1 article en base — gate du lien « mention nue »
    /// vers `/texte/{slug}` (même doctrine que les décisions).
    pub ref_has_articles: bool,
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
/// `code` = code canonique user-facing (`tj_paris`) ; `source_code` = code de
/// la source (location Judilibre `tj75056`, ADR 0201), clé de résolution à
/// l'ingest et des snapshots d'extraction (chrono, labels).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JurisdictionRow {
    pub code: String,
    pub source_code: String,
    pub jurisdiction_type: String,
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

// ── Fiche entité (lecture, ADR 0189) ─────────────────────────────────────────

/// Identité registre d'une entité (`entity`) pour l'en-tête de fiche.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityHeaderRow {
    pub uid: String,
    pub nature: String,
    pub denomination: String,
    pub sigle: Option<String>,
    pub forme: Option<String>,
    pub active: bool,
}

/// Une dénomination datée d'une entité (`entity_denomination`). Dates rendues en
/// ISO `String` (`::text` côté SQL) ; `None` = borne ouverte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityDenominationReadRow {
    pub denomination: String,
    pub date_debut: Option<String>,
    pub date_fin: Option<String>,
}

/// Comptes contentieux agrégés d'une entité (décisions distinctes) : total et
/// répartition par côté (`applicant`/`defendant`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityContentieuxCounts {
    pub decision_count: i64,
    pub as_applicant: i64,
    pub as_defendant: i64,
}

/// Décisions d'une entité groupées par année de lecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityYearCountRow {
    pub year: i32,
    pub count: i64,
}

/// Décisions d'une entité groupées par juridiction : `jurisdiction_code` du
/// référentiel (nullable) + `jurisdiction_type` de repli, le libellé étant résolu
/// côté `lj-api` (référentiel).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityJurisdictionCountRow {
    pub jurisdiction_code: Option<String>,
    pub jurisdiction_type: String,
    pub count: i64,
}

/// Un conseil (avocat/cabinet) co-occurrent d'une entité : verbatim extrait +
/// uid registre résolu si le conseil l'est lui-même.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityCounselRow {
    pub value: String,
    pub entity_uid: Option<String>,
    pub count: i64,
}

/// Une décision citant l'entité (liste paginée) : id interne pour
/// l'hydratation `SearchHit` côté `lj-api` (rendu unifié avec la recherche) +
/// côté et qualité représentatifs de l'entité dans cette décision (pick stable
/// par priorité `party` > `law_firm` > `counsel_name`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityDecisionRow {
    pub decision_id: i64,
    pub quality: String,
    pub side: Option<String>,
}

/// Une entité de l'annuaire (ADR 0192/0239) : ligne d'`entity` servant
/// recherche et listing sans jointure. `barreau` non `None` pour les
/// avocats `cnb:` (2e segment de l'uid).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityDirectoryRow {
    pub uid: String,
    pub namespace: String,
    pub denomination: String,
    pub nature: String,
    pub forme: Option<String>,
    pub active: bool,
    pub barreau: Option<String>,
    pub decision_count: i64,
}

/// Une ligne `decision_party` lue pour l'encart « Parties » d'une décision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionPartyReadRow {
    pub quality: String,
    pub side: Option<String>,
    pub value: String,
    pub nature: Option<String>,
    pub barreau: Option<String>,
    pub entity_uid: Option<String>,
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
