//! Présentateurs MCP dédiés — port fidèle de `mcp_presenters.py`.
//!
//! Transforme les DTO HTTP internes ([`lj_dtos`]) en sorties MCP plus concises
//! et plus lisibles. Les sorties sont des structs `camelCase` (= `_CamelModel`
//! côté Python, `alias_generator=to_camel`, `extra="forbid"`).

use lj_dtos::{
    ArticleSearchResponse, BookmarkItem, CitationSpan, DecisionDetail, DecisionViewItem,
    FacetChoice, FacetTag, LawArticleResponse, QueryMode, SearchFacets, SearchHistoryEntry,
    SearchResponse,
};
use serde::Serialize;
use std::collections::BTreeMap;

use crate::referential::Referential;
use crate::titles::decision_title;

/// `jurisdiction_type` (enum) → code brut, clé des lignes `jurisdiction_type:*` du
/// référentiel et des titres. Les présentateurs Python recevaient déjà la
/// chaîne brute ; ici on reconstruit le code depuis l'enum sérialisé.
fn jurisdiction_type_code(jt: lj_dtos::JurisdictionType) -> &'static str {
    use lj_dtos::JurisdictionType::*;
    match jt {
        Ta => "TA",
        Caa => "CAA",
        Ce => "CE",
        Constit => "CONSTIT",
        Tc => "TC",
        Cc => "CC",
        Ca => "CA",
        Tj => "TJ",
        Tcom => "TCOM",
        Cedh => "CEDH",
        Cjue => "CJUE",
        Cnda => "CNDA",
        Cnil => "CNIL",
    }
}

/// Rend un enum (ex. `ActivitySource`) en sa valeur sérialisée brute via serde.
fn enum_code<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

// ── Modèles de sortie MCP (`_CamelModel`) ────────────────────────────────────

/// Hit de recherche : identité citable (`title`, `url`, `aiSummary` XOR
/// `snippet`) + bloc de métadonnées où **chaque champ porte le token du filtre
/// `search_decisions` du même nom** (`jurisdictionType` → `jurisdiction_type`,
/// etc.) — la valeur se repasse verbatim pour raffiner.
///
/// L'aperçu porte le nom de sa nature : `aiSummary` (résumé rédigé par IA —
/// jamais les mots de la cour) ou `snippet` (passage verbatim du texte).
/// Exactement un des deux est servi par hit.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSearchHit {
    pub title: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    pub chars: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    pub jurisdiction_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jurisdiction_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chamber: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub procedure: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub office: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legal_domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publication: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSearchResponse {
    pub query: String,
    pub hits: Vec<McpSearchHit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facets: Option<McpSearchFacets>,
}

/// Bloc facettes compact : une map `token de filtre → nombre de décisions`
/// par paramètre de `search_decisions` (mêmes noms de clés) — chaque clé se
/// repasse verbatim dans le filtre du même nom pour raffiner. Pas de labels :
/// tous les tokens sont auto-descriptifs, y compris les slugs de
/// `legal_instrument` (« code-civil », « code-de-justice-administrative »).
#[derive(Debug, Clone, Serialize)]
pub struct McpSearchFacets {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub jurisdiction_type: BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub jurisdiction_code: BTreeMap<String, i64>,
    /// Cours au-delà du top-15 de `jurisdiction_code` (troncature annoncée).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_courts: Option<usize>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub chamber: BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub office: BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub legal_domain: BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub solution: BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub significance: BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub publication: BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub date_lecture_year: BTreeMap<String, i64>,
    /// Slugs des textes cités (tokens des filtres `legal_instrument` /
    /// `legal_article`), top-10 par count.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub legal_instrument: BTreeMap<String, i64>,
}

/// Détail d'une décision : identité + le même bloc de métadonnées-tokens que
/// les hits de `search_decisions` (mêmes noms de champs, mêmes valeurs de
/// filtre) + le texte intégral.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDecisionDetail {
    pub title: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    pub jurisdiction_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jurisdiction_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chamber: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub procedure: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub office: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legal_domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publication: Option<String>,
    /// Sort de CETTE décision devant la juridiction qui l'a révisée,
    /// pré-calculé depuis `case_chronology` : « INFIRMATION — Cour d'appel
    /// de Paris, 1 juillet 2025 (url) ». Absent = aucun recours connu du
    /// corpus, ce qui ne prouve jamais qu'aucun recours n'existe.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub appellate_fate: Option<String>,
    /// Chronologie de l'affaire (liens appel/pourvoi/renvoi résolus, décision
    /// courante incluse, de la plus récente à la plus ancienne). Le sort d'une
    /// décision se lit dans la `solution` de celle qui la révise. Absente ≠
    /// pas de recours : seuls les liens connus du corpus sont chaînés.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub case_chronology: Vec<McpChronologyStep>,
    /// Commentaires institutionnels (ADR 0204), type DTO tel quel : `analyse`
    /// (corps servi sur place), `conclusions` / `note` (liens sortants).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub commentaires: Vec<lj_dtos::Commentaire>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpChronologyStep {
    /// « Cour d'appel de Paris, 23 juin 2026 ».
    pub title: String,
    pub url: String,
    pub current: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solution: Option<String>,
    /// Nature du lien vers l'étape suivante (la décision attaquée) :
    /// `APPEL_DE` | `POURVOI_CONTRE` | `RENVOI_APRES_CASSATION`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_to_next: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSavedSearch {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filters: Option<serde_json::Value>,
    pub source: String,
    pub searched_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSavedSearchesResponse {
    pub total: i64,
    pub searches: Vec<McpSavedSearch>,
}

/// Référence décision pour signets/consultations : assez pour citer ou chaîner
/// vers `get_decision` via `url`, sans le texte intégral.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDecisionRef {
    pub title: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    /// Token `solution` (clé du référentiel `solution:*`, = filtre de
    /// `search_decisions`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bookmarked_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_viewed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpBookmarksResponse {
    pub total: i64,
    pub bookmarks: Vec<McpDecisionRef>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpReadingHistoryResponse {
    pub total: i64,
    pub decisions: Vec<McpDecisionRef>,
}

/// Sortie unique de `list_my_activity` : chaque tranche n'est remplie que si
/// elle a été demandée (`kind` ciblé ou `all`).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpActivityResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub searches: Option<McpSavedSearchesResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bookmarks: Option<McpBookmarksResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reading_history: Option<McpReadingHistoryResponse>,
}

/// Une version dans la timeline d'un article law-at-date : état + bornes, sans
/// l'identifiant `legiarti` interne.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpLawArticleVersion {
    pub etat: String,
    pub date_debut: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_fin: Option<String>,
}

/// Article de loi servi à une date (law-at-date) : identité citable + texte +
/// `url` publique + timeline des versions. Slim comme [`McpDecisionDetail`] :
/// on laisse tomber les internes du DTO web (`legiarti`/`legitext`, provenance
/// de traduction, autorité de diffuseur, contexte de lecture…).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpLawArticle {
    pub code: String,
    pub num: String,
    pub title: String,
    pub url: String,
    pub etat: String,
    pub date_debut: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_fin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nota: Option<String>,
    pub versions: Vec<McpLawArticleVersion>,
    /// Commentaires de norme (ADR 0212), même forme que côté décision.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub commentaires: Vec<lj_dtos::Commentaire>,
}

/// Un hit de recherche d'articles : assez pour citer et chaîner (`title` porte
/// déjà l'article et son texte ; `url` = `/texte/{code}/{clé}`, à repasser telle
/// quelle à `get_legal_text`), avec l'extrait surligné (`snippet`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpLawSearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_path: Option<String>,
    pub source: String,
}

/// Bloc facettes compact de la recherche d'articles, même contrat que
/// [`McpSearchFacets`] : une map `token de filtre → nombre d'articles` par
/// paramètre de `search_legal_texts`, chaque clé se repasse verbatim dans le
/// filtre du même nom. Chaque axe est plafonné au top-10 (la traîne
/// `jurisdiction` monte à 60+ pays à quelques hits — mesuré ~700 chars de
/// bruit par réponse).
#[derive(Debug, Clone, Serialize)]
pub struct McpLawSearchFacets {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub code: BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub jurisdiction: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpLawSearchResponse {
    pub query: String,
    pub total: i64,
    pub hits: Vec<McpLawSearchHit>,
    pub facets: McpLawSearchFacets,
}

// ── Présentateurs ────────────────────────────────────────────────────────────

/// Présente une [`SearchResponse`] pour MCP.
///
/// L'aperçu suit le mode *résolu* (`query_mode`), pas le mode demandé :
/// - lexical → `snippet` (passage verbatim, marques `<mark>` retirées) ;
/// - hybrid → `aiSummary` (résumé pré-calculé `hit.summary`), repli sur
///   `snippet` si le résumé manque.
pub fn present_search_response(response: &SearchResponse, web_base_url: &str) -> McpSearchResponse {
    let use_summary = response.query_mode == QueryMode::Hybrid;
    let hits = response
        .hits
        .iter()
        .map(|hit| {
            let (ai_summary, snippet) = match (use_summary, hit.summary.as_deref()) {
                (true, Some(summary)) if !summary.is_empty() => (Some(summary.to_string()), None),
                _ => (None, Some(strip_marks(&hit.best_chunk.snippet))),
            };
            McpSearchHit {
                // Le titre du hit web porte déjà le siège composé (ADR 0170) :
                // on le reprend débarrassé du highlight.
                title: strip_marks(&hit.title_html),
                url: format!("{web_base_url}/decision/{}", hit.id),
                ai_summary,
                snippet,
                chars: hit.chars.unwrap_or(0),
                date_lecture: hit.date_lecture.clone(),
                docket_numbers: hit.docket_numbers.clone(),
                jurisdiction_type: jurisdiction_type_code(hit.jurisdiction_type).to_string(),
                jurisdiction_code: hit.jurisdiction_code.clone(),
                chamber: tag_key(&hit.chamber),
                solution: tag_key(&hit.solution),
                procedure: tag_key(&hit.procedure),
                office: tag_key(&hit.office),
                legal_domain: tag_key(&hit.legal_domain),
                publication: tag_key(&hit.publication),
            }
        })
        .collect();
    // Le bloc facettes n'apporte du raffinement que si le total déborde la
    // page servie ; une requête déjà couverte par ses hits s'en passe.
    let facets = response
        .facets
        .as_ref()
        .filter(|_| response.total > response.hits.len() as i64)
        .and_then(present_search_facets);
    McpSearchResponse {
        query: response.query.clone(),
        hits,
        facets,
    }
}

const FACET_COURTS_LIMIT: usize = 15;
const FACET_INSTRUMENTS_LIMIT: usize = 10;

/// Compacte les [`SearchFacets`] du flux web (listes `{value, label, count,
/// parent}`) en maps `token → count` (cf. [`McpSearchFacets`]). `None` si tout
/// est vide.
fn present_search_facets(f: &SearchFacets) -> Option<McpSearchFacets> {
    fn counts(choices: &[FacetChoice]) -> BTreeMap<String, i64> {
        choices.iter().map(|c| (c.value.clone(), c.count)).collect()
    }
    // Juridiction 2 niveaux : racines = tokens `jurisdiction_type` ; enfants =
    // codes de cours, top-N par count, troncature annoncée dans `other_courts`.
    let jurisdiction_type: BTreeMap<String, i64> = f
        .jurisdiction
        .iter()
        .filter(|c| c.parent.is_none())
        .map(|c| (c.value.clone(), c.count))
        .collect();
    let mut courts: Vec<&FacetChoice> = f
        .jurisdiction
        .iter()
        .filter(|c| c.parent.is_some())
        .collect();
    courts.sort_by_key(|c| std::cmp::Reverse(c.count));
    let other_courts = courts
        .len()
        .checked_sub(FACET_COURTS_LIMIT)
        .filter(|n| *n > 0);
    let jurisdiction_code: BTreeMap<String, i64> = courts
        .iter()
        .take(FACET_COURTS_LIMIT)
        .map(|c| (c.value.clone(), c.count))
        .collect();
    let mut instruments: Vec<&lj_dtos::LegalInstrumentFacet> = f.legal_instrument.iter().collect();
    instruments.sort_by_key(|l| std::cmp::Reverse(l.count));
    // Token = slug du texte, seule clé que les filtres acceptent — une entrée
    // sans slug n'est pas filtrable, elle ne se montre pas.
    let legal_instrument: BTreeMap<String, i64> = instruments
        .into_iter()
        .take(FACET_INSTRUMENTS_LIMIT)
        .filter_map(|l| l.slug.clone().map(|s| (s, l.count)))
        .collect();
    let facets = McpSearchFacets {
        jurisdiction_type,
        jurisdiction_code,
        other_courts,
        chamber: counts(&f.chamber),
        office: counts(&f.office),
        legal_domain: counts(&f.legal_domain),
        solution: counts(&f.solution),
        significance: counts(&f.significance),
        publication: counts(&f.publication),
        date_lecture_year: counts(&f.date_lecture_year),
        legal_instrument,
    };
    let empty = facets.jurisdiction_type.is_empty()
        && facets.jurisdiction_code.is_empty()
        && facets.chamber.is_empty()
        && facets.office.is_empty()
        && facets.legal_domain.is_empty()
        && facets.solution.is_empty()
        && facets.significance.is_empty()
        && facets.publication.is_empty()
        && facets.date_lecture_year.is_empty()
        && facets.legal_instrument.is_empty();
    (!empty).then_some(facets)
}

/// Libellé du type de juridiction depuis le référentiel `jurisdiction_type:*`
/// (repli sur le code brut faute d'entrée — cache d'une heure vs seed frais).
fn type_label(jt: lj_dtos::JurisdictionType, refs: &Referential) -> &str {
    let code = jurisdiction_type_code(jt);
    refs.jurisdiction_type_label(code).unwrap_or(code)
}

/// Token de filtre d'un tag référentiel optionnel (`FacetTag.key` =
/// suffixe d'uid, exactement ce que les filtres de `search_decisions`
/// acceptent).
fn tag_key(tag: &Option<FacetTag>) -> Option<String> {
    tag.as_ref().map(|t| t.key.clone())
}

/// Présente le détail complet d'une décision (titre + url + texte concaténé).
/// Les citations résolues du corps (`paragraph_spans`, ADR 0134 — celles que le
/// front rend cliquables) sont rendues en liens markdown inline dans `text`.
pub fn present_decision_detail(
    detail: &DecisionDetail,
    web_base_url: &str,
    refs: &Referential,
) -> McpDecisionDetail {
    let text = detail
        .paragraphs
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let spans = detail.paragraph_spans.get(i).map_or(&[][..], Vec::as_slice);
            paragraph_with_links(p, spans, web_base_url)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let case_chronology: Vec<McpChronologyStep> = detail
        .chronology
        .iter()
        .map(|e| McpChronologyStep {
            title: lj_core::titles::decision_title(
                &e.label,
                None,
                e.date.as_deref(),
                e.docket_numbers
                    .as_deref()
                    .and_then(|d| d.first())
                    .map(String::as_str),
            ),
            url: format!("{web_base_url}/decision/{}", e.id),
            current: e.current,
            solution: e.solution.clone(),
            link_to_next: e.link.clone(),
        })
        .collect();
    let appellate_fate = appellate_fate(&case_chronology);
    // Le sort connu ouvre ET clôt le texte : les consommateurs LLM lisent le
    // corps et sautent les métadonnées — une session réelle a écrit « aucun
    // recours connu » sur un jugement dont la réponse énonçait la
    // confirmation en premier champ ; la même erreur a survécu à la bannière
    // en première ligne (l'attention des modèles privilégie la fin d'un long
    // output), d'où la répétition en dernière ligne.
    let text = match &appellate_fate {
        Some(fate) => format!(
            "[SORT DE CETTE DÉCISION SUR RECOURS : {fate}]\n\n{text}\n\n[RAPPEL — SORT DE CETTE DÉCISION SUR RECOURS : {fate}]"
        ),
        None => text,
    };
    // Le sort marque aussi le titre : c'est le champ copié au moment de
    // rédiger une citation — une session réelle a reçu la bannière aux deux
    // extrémités du texte et a quand même cité le jugement infirmé comme
    // soutien (« aucun recours connu »).
    let title = {
        let base = decision_title(
            type_label(detail.jurisdiction_type, refs),
            detail.jurisdiction_name.as_deref(),
            detail.seat.as_deref(),
            detail.date_lecture.as_deref(),
            detail.docket_numbers.as_deref(),
        );
        match &appellate_fate {
            Some(fate) => {
                let solution = fate.split(" — ").next().unwrap_or("RECOURS");
                format!("{base} [{solution} SUR RECOURS]")
            }
            None => base,
        }
    };
    McpDecisionDetail {
        title,
        url: format!("{web_base_url}/decision/{}", detail.id),
        date_lecture: detail.date_lecture.clone(),
        docket_numbers: detail.docket_numbers.clone(),
        jurisdiction_type: jurisdiction_type_code(detail.jurisdiction_type).to_string(),
        jurisdiction_code: detail.jurisdiction_code.clone(),
        chamber: tag_key(&detail.chamber),
        solution: tag_key(&detail.solution),
        procedure: tag_key(&detail.procedure),
        office: tag_key(&detail.office),
        legal_domain: tag_key(&detail.legal_domain),
        publication: tag_key(&detail.publication),
        appellate_fate,
        case_chronology,
        commentaires: detail.commentaires.clone(),
        text,
    }
}

/// L'étape qui révise directement la décision courante (adjacente au-dessus
/// dans la chaîne, ordonnée de la plus récente à la plus ancienne), résumée
/// en une ligne : « INFIRMATION — Cour d'appel de Paris, 1 juillet 2025 (url) ».
fn appellate_fate(chronology: &[McpChronologyStep]) -> Option<String> {
    let idx = chronology.iter().position(|s| s.current)?;
    let above = chronology.get(idx.checked_sub(1)?)?;
    above.link_to_next.as_ref()?;
    Some(format!(
        "{} — {} ({})",
        above.solution.as_deref().unwrap_or("RECOURS"),
        above.title,
        above.url
    ))
}

/// Enveloppe chaque citation résolue d'un texte (paragraphe de décision,
/// corps d'article) en lien markdown `[texte](url)`, sans réécrire le span.
/// Cible unique = son `href`. Cibles multiples (plage « articles 3 à 6 »,
/// « et suivants », fusion de chevauchements — le front rend un menu, ADR
/// 0125) : la première résolue enveloppe le span, les suivantes sont
/// appendues juste après en liens labellisés « (+ [label](url), …) » —
/// chaque article de la plage reste ouvrable. Les spans sans aucune cible
/// résolue restent en clair. Offsets en codepoints locaux au paragraphe,
/// spans fusionnés non chevauchants et ordonnés (invariant du producteur,
/// ADR 0134).
fn paragraph_with_links(text: &str, spans: &[CitationSpan], web_base_url: &str) -> String {
    if spans.is_empty() {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + spans.len() * 64);
    let mut cursor = 0usize;
    for span in spans {
        let mut resolved = span
            .targets
            .iter()
            .filter_map(|t| t.href.as_deref().map(|href| (href, t.label.as_str())));
        let Some((href, _)) = resolved.next() else {
            continue;
        };
        out.extend(&chars[cursor..span.start]);
        out.push('[');
        out.extend(&chars[span.start..span.end]);
        out.push_str("](");
        out.push_str(web_base_url);
        out.push_str(href);
        out.push(')');
        let extras: Vec<String> = resolved
            .map(|(h, l)| format!("[{l}]({web_base_url}{h})"))
            .collect();
        if !extras.is_empty() {
            out.push_str(" (+ ");
            out.push_str(&extras.join(", "));
            out.push(')');
        }
        cursor = span.end;
    }
    out.extend(&chars[cursor..]);
    out
}

/// Présente l'historique des recherches sauvegardées.
pub fn present_saved_searches(entries: &[SearchHistoryEntry]) -> McpSavedSearchesResponse {
    McpSavedSearchesResponse {
        total: entries.len() as i64,
        searches: entries
            .iter()
            .map(|entry| McpSavedSearch {
                query: entry.query.clone(),
                // `entry.filters or None` : un objet vide / null → None.
                filters: filters_or_none(&entry.filters),
                source: enum_code(&entry.source),
                searched_at: entry.created_at.clone(),
            })
            .collect(),
    }
}

/// Référence décision commune signets/lectures ; les champs propres à chaque
/// variante (date de signet, compteur/date de lecture) arrivent en paramètres.
#[allow(clippy::too_many_arguments)]
fn decision_ref(
    id: &str,
    jurisdiction_type: lj_dtos::JurisdictionType,
    jurisdiction_name: Option<&str>,
    date_lecture: Option<&str>,
    docket_numbers: Option<&[String]>,
    solution: &Option<FacetTag>,
    summary: Option<&str>,
    web_base_url: &str,
    refs: &Referential,
    bookmarked_at: Option<String>,
    view_count: Option<i64>,
    last_source: Option<String>,
    last_viewed_at: Option<String>,
) -> McpDecisionRef {
    let jt_label = type_label(jurisdiction_type, refs);
    McpDecisionRef {
        title: decision_title(
            jt_label,
            jurisdiction_name,
            None,
            date_lecture,
            docket_numbers,
        ),
        url: format!("{web_base_url}/decision/{id}"),
        summary: summary.map(str::to_string),
        date_lecture: date_lecture.map(str::to_string),
        solution: tag_key(solution),
        bookmarked_at,
        view_count,
        last_source,
        last_viewed_at,
    }
}

/// Présente les signets de l'utilisateur.
pub fn present_bookmarks(
    items: &[BookmarkItem],
    web_base_url: &str,
    refs: &Referential,
) -> McpBookmarksResponse {
    McpBookmarksResponse {
        total: items.len() as i64,
        bookmarks: items
            .iter()
            .map(|item| {
                decision_ref(
                    &item.id,
                    item.jurisdiction_type,
                    item.jurisdiction_name.as_deref(),
                    item.date_lecture.as_deref(),
                    item.docket_numbers.as_deref(),
                    &item.solution,
                    item.summary.as_deref(),
                    web_base_url,
                    refs,
                    Some(item.bookmarked_at.clone()),
                    None,
                    None,
                    None,
                )
            })
            .collect(),
    }
}

/// Présente l'historique de lecture (consultations) de l'utilisateur.
pub fn present_reading_history(
    items: &[DecisionViewItem],
    web_base_url: &str,
    refs: &Referential,
) -> McpReadingHistoryResponse {
    McpReadingHistoryResponse {
        total: items.len() as i64,
        decisions: items
            .iter()
            .map(|item| {
                decision_ref(
                    &item.id,
                    item.jurisdiction_type,
                    item.jurisdiction_name.as_deref(),
                    item.date_lecture.as_deref(),
                    item.docket_numbers.as_deref(),
                    &item.solution,
                    item.summary.as_deref(),
                    web_base_url,
                    refs,
                    None,
                    Some(item.view_count),
                    Some(enum_code(&item.last_source)),
                    Some(item.last_viewed_at.clone()),
                )
            })
            .collect(),
    }
}

/// Titre citable d'un article : « Article {num} du/de la/de l'{codeTitle} »
/// (contraction par [`lj_dtos::instrument_with_de`]), repli « Article {num} »
/// si le titre du code manque.
fn law_article_title(num: &str, code_title: Option<&str>) -> String {
    match code_title {
        Some(t) if !t.is_empty() => {
            format!("Article {num} {}", lj_dtos::instrument_with_de(t))
        }
        _ => format!("Article {num}"),
    }
}

/// Présente un article law-at-date pour MCP : identité, texte, `url` publique
/// `/texte/{code}/{numKey}`, et la timeline des versions (état + bornes).
/// Les renvois résolus du corps (`texte_spans`, ADR 0217 — ceux que le front
/// rend cliquables, hrefs datés quand l'article est servi à date) sont rendus
/// en liens markdown inline dans `text`, comme les citations des décisions.
pub fn present_law_article(article: &LawArticleResponse, web_base_url: &str) -> McpLawArticle {
    McpLawArticle {
        title: law_article_title(&article.num, article.code_title.as_deref()),
        url: format!("{web_base_url}/texte/{}/{}", article.code, article.num_key),
        code: article.code.clone(),
        num: article.num.clone(),
        etat: article.etat.clone(),
        date_debut: article.date_debut.clone(),
        date_fin: article.date_fin.clone(),
        text: article
            .texte
            .as_ref()
            .map(|t| paragraph_with_links(t, &article.texte_spans, web_base_url)),
        source_url: article.source_url.clone(),
        nota: article.nota.clone(),
        versions: article
            .versions
            .iter()
            .map(|v| McpLawArticleVersion {
                etat: v.etat.clone(),
                date_debut: v.date_debut.clone(),
                date_fin: v.date_fin.clone(),
            })
            .collect(),
        commentaires: article.commentaires.clone(),
    }
}

/// Présente une recherche d'articles pour MCP : hits citables (titre + `url`
/// `/texte/{code}/{numKey}` + snippet), total exact et facettes `code` (top-10,
/// les slugs arrivent triés count décroissant) / `jurisdiction`, à chaîner
/// vers `get_legal_text` ou repasser en filtre.
pub fn present_law_search(
    query: &str,
    response: &ArticleSearchResponse,
    web_base_url: &str,
) -> McpLawSearchResponse {
    let counts = |choices: &[lj_dtos::FacetChoice], cap: usize| {
        choices
            .iter()
            .take(cap)
            .map(|c| (c.value.clone(), c.count))
            .collect::<BTreeMap<_, _>>()
    };
    McpLawSearchResponse {
        query: query.to_string(),
        total: response.total,
        hits: response
            .hits
            .iter()
            .map(|h| McpLawSearchHit {
                title: law_article_title(&h.num, Some(&h.code_title)),
                url: format!("{web_base_url}/texte/{}/{}", h.code, h.num_key),
                snippet: h.snippet.clone(),
                title_path: h.titre_path.clone(),
                source: h.source.clone(),
            })
            .collect(),
        facets: McpLawSearchFacets {
            code: counts(&response.facets.code, 10),
            jurisdiction: counts(&response.facets.jurisdiction, 10),
        },
    }
}

/// `entry.filters or None` côté Python : un objet `{}`/`null` est tombé sur
/// `None`, sinon on garde la valeur telle quelle.
fn filters_or_none(filters: &serde_json::Value) -> Option<serde_json::Value> {
    match filters {
        serde_json::Value::Null => None,
        serde_json::Value::Object(map) if map.is_empty() => None,
        other => Some(other.clone()),
    }
}

/// Retire les balises `<mark>`/`</mark>` du snippet web pour MCP (port de
/// `_strip_marks`).
fn strip_marks(text: &str) -> String {
    text.replace("<mark>", "").replace("</mark>", "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_dtos::{
        ArticleSearchFacets, ArticleSearchHit, ArticleSearchResponse, BestChunk, CitationTarget,
        JurisdictionType, LawArticleResponse, LawArticleVersion, QueryMode, SearchHit,
        SearchResponse,
    };
    use lj_store::repository::FacetValueRow;

    /// Référentiel de test : labels de types + tags nécessaires aux fixtures.
    fn refs() -> Referential {
        let fv = |uid: &str, label: &str| FacetValueRow {
            uid: uid.to_string(),
            facet: uid.split(':').next().unwrap().to_string(),
            label: label.to_string(),
            abbr: None,
            parent_uid: None,
            sort: 0,
        };
        Referential::new(
            vec![
                fv("jurisdiction_type:CC", "Cour de cassation"),
                fv("jurisdiction_type:CE", "Conseil d'État"),
            ],
            Vec::new(),
        )
    }

    fn base_hit() -> SearchHit {
        SearchHit {
            id: "cc-123".into(),
            jurisdiction_type: JurisdictionType::Cc,
            jurisdiction_code: Some("cc".into()),
            jurisdiction_name: Some("Cour de cassation".into()),
            title_html: "Cour de <mark>cassation</mark>, 29 mai 2026, 24-17.384".into(),
            seat: None,
            score: 1.0,
            best_chunk: BestChunk {
                chunk_index: 0,
                snippet: "un <mark>bail</mark> commercial".into(),
            },
            date_lecture: Some("2026-05-29".into()),
            docket_numbers: Some(vec!["24-17.384".into()]),
            solution: Some(FacetTag {
                key: "REJET".into(),
                label: "Rejet".into(),
            }),
            procedure: None,
            office: None,
            legal_domain: None,
            chamber: Some(FacetTag {
                key: "SOC".into(),
                label: "Chambre sociale".into(),
            }),
            publication: Some(FacetTag {
                key: "PUBLIE_BULLETIN".into(),
                label: "Publié au bulletin".into(),
            }),
            publication_codes: vec!["b".into()],
            chars: Some(4200),
            summary: Some("Résumé de la décision.".into()),
        }
    }

    #[test]
    fn lexical_serves_stripped_snippet() {
        let resp = SearchResponse {
            query: "bail".into(),
            total: 1,
            hits: vec![base_hit()],
            query_mode: QueryMode::Lexical,
            facets: None,
            all_hit_ids: vec![],
        };
        let out = present_search_response(&resp, "https://librejustice.fr");
        let hit = &out.hits[0];
        // mode lexical → snippet sans <mark>, jamais le résumé.
        assert_eq!(hit.snippet.as_deref(), Some("un bail commercial"));
        assert_eq!(hit.ai_summary, None);
        assert_eq!(hit.title, "Cour de cassation, 29 mai 2026, 24-17.384");
        assert_eq!(hit.url, "https://librejustice.fr/decision/cc-123");
        assert_eq!(hit.chars, 4200);
        // Procedure absente (procédure ordinaire = null, ADR 0146) → champ omis.
        assert_eq!(hit.procedure, None);
        // Métadonnées = tokens de filtre (FacetTag.key), pas les labels FR.
        assert_eq!(hit.jurisdiction_type, "CC");
        assert_eq!(hit.jurisdiction_code.as_deref(), Some("cc"));
        assert_eq!(hit.chamber.as_deref(), Some("SOC"));
        assert_eq!(hit.solution.as_deref(), Some("REJET"));
        assert_eq!(hit.publication.as_deref(), Some("PUBLIE_BULLETIN"));
    }

    #[test]
    fn hybrid_serves_ai_summary() {
        let resp = SearchResponse {
            query: "bail".into(),
            total: 1,
            hits: vec![base_hit()],
            query_mode: QueryMode::Hybrid,
            facets: None,
            all_hit_ids: vec![],
        };
        let out = present_search_response(&resp, "https://librejustice.fr");
        assert_eq!(
            out.hits[0].ai_summary.as_deref(),
            Some("Résumé de la décision.")
        );
        assert_eq!(out.hits[0].snippet, None);
    }

    #[test]
    fn hybrid_without_summary_falls_back_to_snippet() {
        let mut hit = base_hit();
        hit.summary = None;
        let resp = SearchResponse {
            query: "bail".into(),
            total: 1,
            hits: vec![hit],
            query_mode: QueryMode::Hybrid,
            facets: None,
            all_hit_ids: vec![],
        };
        let out = present_search_response(&resp, "https://librejustice.fr");
        assert_eq!(out.hits[0].snippet.as_deref(), Some("un bail commercial"));
        assert_eq!(out.hits[0].ai_summary, None);
    }

    /// Bloc facettes compact : clés = tokens de filtre (racines juridiction
    /// dépréfixées, top-15 cours avec troncature annoncée, instruments par
    /// slug) ; omis quand les hits couvrent déjà le total.
    #[test]
    fn facets_compact_to_filter_tokens() {
        let choice = |value: &str, count: i64, parent: Option<&str>| FacetChoice {
            value: value.into(),
            label: format!("Label {value}"),
            count,
            parent: parent.map(str::to_string),
        };
        let mut jurisdiction = vec![choice("TA", 74, None)];
        for i in 0..17 {
            jurisdiction.push(choice(&format!("ta_ville{i:02}"), 17 - i, Some("TA")));
        }
        let facets = SearchFacets {
            jurisdiction,
            solution: vec![choice("REJET", 63, None)],
            legal_instrument: vec![lj_dtos::LegalInstrumentFacet {
                value: "LEGITEXT000006070721".into(),
                label: "Code civil".into(),
                slug: Some("code-civil".into()),
                count: 657,
                articles: vec![],
            }],
            ..Default::default()
        };
        let resp = SearchResponse {
            query: "bail".into(),
            total: 92,
            hits: vec![base_hit()],
            query_mode: QueryMode::Hybrid,
            facets: Some(facets.clone()),
            all_hit_ids: vec![],
        };
        let out = present_search_response(&resp, "https://librejustice.fr");
        let f = out.facets.expect("facets présentes (total > hits)");
        assert_eq!(f.jurisdiction_type.get("TA"), Some(&74));
        assert_eq!(f.jurisdiction_code.len(), 15);
        assert_eq!(f.jurisdiction_code.get("ta_ville00"), Some(&17));
        // 17 cours, top-15 gardées → 2 annoncées.
        assert_eq!(f.other_courts, Some(2));
        assert_eq!(f.solution.get("REJET"), Some(&63));
        assert_eq!(f.legal_instrument.get("code-civil"), Some(&657));

        // total couvert par la page → pas de bloc.
        let resp = SearchResponse {
            query: "bail".into(),
            total: 1,
            hits: vec![base_hit()],
            query_mode: QueryMode::Hybrid,
            facets: Some(facets),
            all_hit_ids: vec![],
        };
        let out = present_search_response(&resp, "https://librejustice.fr");
        assert!(out.facets.is_none());
    }

    #[test]
    fn detail_concatenates_paragraphs_with_blank_line() {
        let detail = DecisionDetail {
            id: "ce-1".into(),
            jurisdiction_type: JurisdictionType::Ce,
            title: String::new(),
            paragraphs: vec!["Para 1.".into(), "Para 2.".into()],
            paragraph_spans: Vec::new(),
            sections: None,
            summary: None,
            jurisdiction_code: None,
            jurisdiction_name: None,
            date_lecture: Some("2024-08-06".into()),
            solution: None,
            procedure: None,
            office: None,
            legal_domain: None,
            publication: None,
            publication_codes: vec![],
            date_audience: None,
            docket_numbers: None,
            seat: None,
            chamber: None,
            formation: None,
            legal_references: None,
            source_xml: None,
            themes: Vec::new(),
            nac: None,
            ecli: None,
            source: None,
            chronology: Vec::new(),
            commentaires: vec![],
        };
        let out = present_decision_detail(&detail, "https://librejustice.fr", &refs());
        assert_eq!(out.text, "Para 1.\n\nPara 2.");
        assert_eq!(out.url, "https://librejustice.fr/decision/ce-1");
        // jurisdiction_name absent → libellé du type (référentiel `jurisdiction_type:*`).
        assert_eq!(out.title, "Conseil d'État, 6 août 2024");
    }

    #[test]
    fn detail_renders_case_chronology_with_solution_and_urls() {
        let detail = DecisionDetail {
            id: "tj-1".into(),
            jurisdiction_type: JurisdictionType::Tj,
            title: String::new(),
            paragraphs: vec!["Motifs.".into()],
            paragraph_spans: Vec::new(),
            sections: None,
            summary: None,
            jurisdiction_code: None,
            jurisdiction_name: None,
            date_lecture: Some("2025-01-23".into()),
            solution: None,
            procedure: None,
            office: None,
            legal_domain: None,
            publication: None,
            publication_codes: vec![],
            date_audience: None,
            docket_numbers: None,
            seat: None,
            chamber: None,
            formation: None,
            legal_references: None,
            source_xml: None,
            themes: Vec::new(),
            nac: None,
            ecli: None,
            source: None,
            chronology: vec![
                lj_dtos::ChronologyEntry {
                    id: "ca-9".into(),
                    label: "Cour d'appel de Paris".into(),
                    date: Some("2026-06-23".into()),
                    current: false,
                    solution: Some("INFIRMATION".into()),
                    docket_numbers: Some(vec!["25/03815".into()]),
                    link: Some("APPEL_DE".into()),
                },
                lj_dtos::ChronologyEntry {
                    id: "tj-1".into(),
                    label: "Tribunal judiciaire de Paris".into(),
                    date: Some("2025-01-23".into()),
                    current: true,
                    solution: Some("SATISFACTION_TOTALE".into()),
                    docket_numbers: Some(vec!["21/16038".into()]),
                    link: None,
                },
            ],
            commentaires: vec![],
        };
        let out = present_decision_detail(&detail, "https://librejustice.fr", &refs());
        assert_eq!(out.case_chronology.len(), 2);
        let ca = &out.case_chronology[0];
        assert_eq!(ca.title, "Cour d'appel de Paris, 23 juin 2026, 25/03815");
        assert_eq!(ca.url, "https://librejustice.fr/decision/ca-9");
        assert_eq!(ca.solution.as_deref(), Some("INFIRMATION"));
        assert_eq!(ca.link_to_next.as_deref(), Some("APPEL_DE"));
        assert!(!ca.current);
        assert!(out.case_chronology[1].current);
        assert_eq!(
            out.appellate_fate.as_deref(),
            Some(
                "INFIRMATION — Cour d'appel de Paris, 23 juin 2026, 25/03815 \
                 (https://librejustice.fr/decision/ca-9)"
            )
        );
        assert!(out.text.starts_with(
            "[SORT DE CETTE DÉCISION SUR RECOURS : INFIRMATION — \
             Cour d'appel de Paris, 23 juin 2026, 25/03815"
        ));
        assert!(out.text.ends_with(
            "[RAPPEL — SORT DE CETTE DÉCISION SUR RECOURS : INFIRMATION — \
             Cour d'appel de Paris, 23 juin 2026, 25/03815 \
             (https://librejustice.fr/decision/ca-9)]"
        ));
        assert!(out.title.ends_with("[INFIRMATION SUR RECOURS]"));
    }

    #[test]
    fn detail_renders_resolved_citations_as_markdown_links() {
        // Offsets en CODEPOINTS : les « é » (2 octets UTF-8) avant le span
        // décaleraient le rendu si le code comptait en octets.
        let para = "Vu l'énoncé de l'article 700 du CPC.";
        let span = |targets: Vec<CitationTarget>| CitationSpan {
            start: 17,
            end: 35,
            targets,
        };
        let target = |href: Option<&str>| CitationTarget {
            href: href.map(str::to_string),
            label: "Article 700 du Code de procédure civile".into(),
        };
        let detail = |spans: Vec<CitationSpan>| DecisionDetail {
            id: "ce-1".into(),
            jurisdiction_type: JurisdictionType::Ce,
            title: String::new(),
            paragraphs: vec![para.into(), "Rejette la requête.".into()],
            paragraph_spans: vec![spans, vec![]],
            sections: None,
            summary: None,
            jurisdiction_code: None,
            jurisdiction_name: None,
            date_lecture: None,
            solution: None,
            procedure: None,
            office: None,
            legal_domain: None,
            publication: None,
            publication_codes: vec![],
            date_audience: None,
            docket_numbers: None,
            seat: None,
            chamber: None,
            formation: None,
            legal_references: None,
            source_xml: None,
            themes: Vec::new(),
            nac: None,
            ecli: None,
            source: None,
            chronology: Vec::new(),
            commentaires: vec![],
        };

        // Cible unique résolue → span enveloppé, URL préfixée du domaine.
        let resolved = detail(vec![span(vec![target(Some(
            "/texte/code-de-procedure-civile/700",
        ))])]);
        let out = present_decision_detail(&resolved, "https://librejustice.fr", &refs());
        assert_eq!(
            out.text,
            "Vu l'énoncé de l'[article 700 du CPC]\
             (https://librejustice.fr/texte/code-de-procedure-civile/700).\
             \n\nRejette la requête."
        );

        // Multi-cibles, une seule résolue : elle enveloppe, pas d'appendice.
        let multi = detail(vec![span(vec![
            target(None),
            target(Some("/texte/code-de-procedure-civile/700")),
        ])]);
        let out = present_decision_detail(&multi, "https://librejustice.fr", &refs());
        assert!(out
            .text
            .contains("](https://librejustice.fr/texte/code-de-procedure-civile/700)"));
        assert!(!out.text.contains("(+ "));

        // Plage multi-résolue : la première enveloppe le span, les suivantes
        // sont appendues en liens labellisés — chaque cible reste ouvrable.
        let range = detail(vec![span(vec![
            target(Some("/texte/code-de-procedure-civile/700")),
            CitationTarget {
                href: Some("/texte/code-de-procedure-civile/701".into()),
                label: "701 — Code de procédure civile".into(),
            },
        ])]);
        let out = present_decision_detail(&range, "https://librejustice.fr", &refs());
        assert!(out.text.contains(
            "](https://librejustice.fr/texte/code-de-procedure-civile/700) \
             (+ [701 — Code de procédure civile]\
             (https://librejustice.fr/texte/code-de-procedure-civile/701))"
        ));

        // Aucune cible résolue → texte inchangé.
        let unresolved = detail(vec![span(vec![target(None)])]);
        let out = present_decision_detail(&unresolved, "https://librejustice.fr", &refs());
        assert_eq!(out.text, format!("{para}\n\nRejette la requête."));
    }

    #[test]
    fn law_article_is_slimmed_and_linked() {
        let article = LawArticleResponse {
            upcoming_version_date: None,
            breadcrumb: Vec::new(),
            legiarti: "LEGIARTI000006419292".into(),
            legitext: "LEGITEXT000006070721".into(),
            code: "code-civil".into(),
            code_title: Some("Code civil".into()),
            num: "1240".into(),
            num_key: "1240".into(),
            etat: "VIGUEUR".into(),
            date_debut: "2016-10-01".into(),
            source: "legifrance".into(),
            titre_text: None,
            date_fin: None,
            texte: Some("Sous réserve de l'article 1241, tout fait…".into()),
            // Renvoi ADR 0217, href daté (article servi à date explicite).
            texte_spans: vec![CitationSpan {
                start: 18,
                end: 30,
                targets: vec![CitationTarget {
                    href: Some("/texte/code-civil/1241?date=2018-01-01".into()),
                    label: "Article 1241 du Code civil".into(),
                }],
            }],
            texte_original: None,
            lang_original: None,
            translation: "officiel".into(),
            source_asof: Some("2026-06-01".into()),
            source_authority: "source gouvernementale".into(),
            source_url: Some(
                "https://www.legifrance.gouv.fr/codes/article_lc/LEGIARTI000006419292/2018-01-01"
                    .into(),
            ),
            source_upstream_url: None,
            nota: None,
            versions: vec![LawArticleVersion {
                upcoming: false,
                legiarti: "LEGIARTI000006419292".into(),
                etat: "VIGUEUR".into(),
                date_debut: "2016-10-01".into(),
                date_fin: None,
            }],
            context: vec![],
            modifications: vec![],
            modified_by: vec![],
            cites: vec![],
            cited_by: vec![],
            commentaires: vec![],
            travaux_parlementaires: vec![],
        };
        let out = present_law_article(&article, "https://librejustice.fr");
        assert_eq!(out.title, "Article 1240 du Code civil");
        assert_eq!(out.url, "https://librejustice.fr/texte/code-civil/1240");
        assert_eq!(
            out.text.as_deref(),
            Some(
                "Sous réserve de l'[article 1241]\
                 (https://librejustice.fr/texte/code-civil/1241?date=2018-01-01), \
                 tout fait…"
            )
        );
        assert_eq!(out.versions.len(), 1);
        assert_eq!(out.versions[0].etat, "VIGUEUR");
    }

    #[test]
    fn law_search_builds_citable_hits() {
        let response = ArticleSearchResponse {
            hits: vec![ArticleSearchHit {
                code: "code-de-procedure-civile".into(),
                code_title: "Code de procédure civile".into(),
                num: "700".into(),
                num_key: "700".into(),
                titre_path: Some("Livre Ier > Titre XVIII".into()),
                snippet: "les <mark>dépens</mark>".into(),
                source: "legifrance".into(),
            }],
            total: 1,
            facets: ArticleSearchFacets {
                scope: Vec::new(),
                code: vec![lj_dtos::FacetChoice {
                    value: "code-de-procedure-civile".into(),
                    label: "code-de-procedure-civile".into(),
                    count: 1,
                    parent: None,
                }],
                jurisdiction: vec![],
                nature: vec![],
                source: vec![],
            },
        };
        let out = present_law_search("dépens", &response, "https://librejustice.fr");
        assert_eq!(out.query, "dépens");
        assert_eq!(out.total, 1);
        let hit = &out.hits[0];
        assert_eq!(hit.title, "Article 700 du Code de procédure civile");
        assert_eq!(
            hit.url,
            "https://librejustice.fr/texte/code-de-procedure-civile/700"
        );
        assert_eq!(out.facets.code.get("code-de-procedure-civile"), Some(&1));
        assert!(out.facets.jurisdiction.is_empty());
    }

    #[test]
    fn filters_or_none_drops_empty_object() {
        assert_eq!(filters_or_none(&serde_json::json!({})), None);
        assert_eq!(filters_or_none(&serde_json::Value::Null), None);
        let v = serde_json::json!({"jurisdictionType": ["CC"]});
        assert_eq!(filters_or_none(&v), Some(v));
    }
}
