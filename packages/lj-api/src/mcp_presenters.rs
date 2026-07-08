//! Présentateurs MCP dédiés — port fidèle de `mcp_presenters.py`.
//!
//! Transforme les DTO HTTP internes ([`lj_dtos`]) en sorties MCP plus concises
//! et plus lisibles. Les sorties sont des structs `camelCase` (= `_CamelModel`
//! côté Python, `alias_generator=to_camel`, `extra="forbid"`).

use lj_core::publication::publication_label;
use lj_dtos::{
    ArticleSearchResponse, BookmarkItem, DecisionDetail, DecisionViewItem, FacetTag,
    LawArticleResponse, QueryMode, SearchHistoryEntry, SearchResponse,
};
use serde::Serialize;

use crate::referential::Referential;
use crate::titles::{decision_jurisdiction, decision_title};

/// `juridiction_type` (enum) → code brut, clé des lignes `juridiction:*` du
/// référentiel et des titres. Les présentateurs Python recevaient déjà la
/// chaîne brute ; ici on reconstruit le code depuis l'enum sérialisé.
fn juridiction_code(jt: lj_dtos::JuridictionType) -> &'static str {
    use lj_dtos::JuridictionType::*;
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSearchHit {
    pub id: String,
    pub title: String,
    pub url: String,
    pub preview: String,
    pub chars: i64,
    pub jurisdiction: String,
    pub date_lecture: Option<String>,
    pub docket_numbers: Option<Vec<String>>,
    /// Libellés FR des tags référentiels (ADR 0146), résolus par l'API.
    pub solution: Option<String>,
    pub voie: Option<String>,
    pub office: Option<String>,
    pub legal_domain: Option<String>,
    pub publication: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSearchResponse {
    pub query: String,
    pub total: i64,
    pub hits: Vec<McpSearchHit>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDecisionDetail {
    pub title: String,
    pub url: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSavedSearch {
    pub query: String,
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
/// vers `get_decision` via `id`, sans le texte intégral.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDecisionRef {
    pub id: String,
    pub title: String,
    pub url: String,
    pub summary: Option<String>,
    pub jurisdiction: String,
    pub date_lecture: Option<String>,
    /// Libellé FR de la solution (référentiel `solution:*`, ADR 0146).
    pub solution: Option<String>,
    pub bookmarked_at: Option<String>,
    pub view_count: Option<i64>,
    pub last_source: Option<String>,
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
    pub searches: Option<McpSavedSearchesResponse>,
    pub bookmarks: Option<McpBookmarksResponse>,
    pub reading_history: Option<McpReadingHistoryResponse>,
}

/// Une version dans la timeline d'un article law-at-date : état + bornes, sans
/// l'identifiant `legiarti` interne.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpLawArticleVersion {
    pub etat: String,
    pub date_debut: String,
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
    pub date_fin: Option<String>,
    pub text: Option<String>,
    pub source_url: Option<String>,
    pub nota: Option<String>,
    pub versions: Vec<McpLawArticleVersion>,
}

/// Un hit de recherche d'articles : assez pour citer (`title`, `url`) ou chaîner
/// vers `get_law_article` (`code` + `num`), avec l'extrait surligné. Analogue de
/// [`McpSearchHit`] (`num` joue le rôle d'`id`, `snippet` celui de `preview`,
/// `codeTitle` celui de `jurisdiction`). Le lien exact `/loi/{code}/{numKey}` est
/// pré-construit dans `url` ; le modèle chaîne via `num` (canonisé serveur).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpLawSearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub code: String,
    pub code_title: String,
    pub num: String,
    pub title_path: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpLawSearchResponse {
    pub query: String,
    pub total: i64,
    pub hits: Vec<McpLawSearchHit>,
}

// ── Présentateurs ────────────────────────────────────────────────────────────

/// Présente une [`SearchResponse`] pour MCP.
///
/// Le `preview` suit le mode *résolu* (`query_mode`), pas le mode demandé :
/// - lexical → le snippet (passage verbatim, marques `<mark>` retirées) ;
/// - hybrid → le résumé pré-calculé (`hit.summary`), repli sur le snippet si
///   le résumé manque.
pub fn present_search_response(
    response: &SearchResponse,
    web_base_url: &str,
    refs: &Referential,
) -> McpSearchResponse {
    let use_summary = response.query_mode == QueryMode::Hybrid;
    let hits = response
        .hits
        .iter()
        .map(|hit| {
            let preview = match (use_summary, hit.summary.as_deref()) {
                (true, Some(summary)) if !summary.is_empty() => summary.to_string(),
                _ => strip_marks(&hit.best_chunk.snippet),
            };
            let jt_label = type_label(hit.juridiction_type, refs);
            McpSearchHit {
                id: hit.id.clone(),
                title: decision_title(
                    jt_label,
                    hit.jurisdiction_name.as_deref(),
                    None,
                    hit.date_lecture.as_deref(),
                    hit.docket_numbers.as_deref(),
                ),
                url: format!("{web_base_url}/decision/{}", hit.id),
                preview,
                chars: hit.chars.unwrap_or(0),
                jurisdiction: decision_jurisdiction(jt_label, hit.jurisdiction_name.as_deref()),
                date_lecture: hit.date_lecture.clone(),
                docket_numbers: hit.docket_numbers.clone(),
                solution: tag_label(&hit.solution),
                voie: tag_label(&hit.voie),
                office: tag_label(&hit.office),
                legal_domain: tag_label(&hit.legal_domain),
                publication: publication_label(Some(&hit.publication_codes)),
            }
        })
        .collect();
    McpSearchResponse {
        query: response.query.clone(),
        total: response.total,
        hits,
    }
}

/// Libellé du type de juridiction depuis le référentiel `juridiction:*`
/// (repli sur le code brut faute d'entrée — cache d'une heure vs seed frais).
fn type_label(jt: lj_dtos::JuridictionType, refs: &Referential) -> &str {
    let code = juridiction_code(jt);
    refs.juridiction_type_label(code).unwrap_or(code)
}

/// Libellé FR d'un tag référentiel optionnel (déjà résolu par l'API).
fn tag_label(tag: &Option<FacetTag>) -> Option<String> {
    tag.as_ref().map(|t| t.label.clone())
}

/// Présente le détail complet d'une décision (titre + url + texte concaténé).
pub fn present_decision_detail(
    detail: &DecisionDetail,
    web_base_url: &str,
    refs: &Referential,
) -> McpDecisionDetail {
    McpDecisionDetail {
        title: decision_title(
            type_label(detail.juridiction_type, refs),
            detail.jurisdiction_name.as_deref(),
            detail.formation_or_chamber.as_deref(),
            detail.date_lecture.as_deref(),
            detail.docket_numbers.as_deref(),
        ),
        url: format!("{web_base_url}/decision/{}", detail.id),
        text: detail.paragraphs.join("\n\n"),
    }
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
    juridiction_type: lj_dtos::JuridictionType,
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
    let jt_label = type_label(juridiction_type, refs);
    McpDecisionRef {
        id: id.to_string(),
        title: decision_title(
            jt_label,
            jurisdiction_name,
            None,
            date_lecture,
            docket_numbers,
        ),
        url: format!("{web_base_url}/decision/{id}"),
        summary: summary.map(str::to_string),
        jurisdiction: decision_jurisdiction(jt_label, jurisdiction_name),
        date_lecture: date_lecture.map(str::to_string),
        solution: tag_label(solution),
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
                    item.juridiction_type,
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
                    item.juridiction_type,
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

/// Titre citable d'un article : « Article {num} du {codeTitle} », repli
/// « Article {num} » si le titre du code manque.
fn law_article_title(num: &str, code_title: Option<&str>) -> String {
    match code_title {
        Some(t) if !t.is_empty() => format!("Article {num} du {t}"),
        _ => format!("Article {num}"),
    }
}

/// Présente un article law-at-date pour MCP : identité, texte, `url` publique
/// `/loi/{code}/{numKey}`, et la timeline des versions (état + bornes).
pub fn present_law_article(article: &LawArticleResponse, web_base_url: &str) -> McpLawArticle {
    McpLawArticle {
        title: law_article_title(&article.num, article.code_title.as_deref()),
        url: format!("{web_base_url}/loi/{}/{}", article.code, article.num_key),
        code: article.code.clone(),
        num: article.num.clone(),
        etat: article.etat.clone(),
        date_debut: article.date_debut.clone(),
        date_fin: article.date_fin.clone(),
        text: article.texte.clone(),
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
    }
}

/// Présente une recherche d'articles pour MCP : hits citables (titre + `url`
/// `/loi/{code}/{numKey}` + snippet) et total exact, à chaîner vers
/// `get_law_article`.
pub fn present_law_search(
    query: &str,
    response: &ArticleSearchResponse,
    web_base_url: &str,
) -> McpLawSearchResponse {
    McpLawSearchResponse {
        query: query.to_string(),
        total: response.total,
        hits: response
            .hits
            .iter()
            .map(|h| McpLawSearchHit {
                title: law_article_title(&h.num, Some(&h.code_title)),
                url: format!("{web_base_url}/loi/{}/{}", h.code, h.num_key),
                snippet: h.snippet.clone(),
                code: h.code.clone(),
                code_title: h.code_title.clone(),
                num: h.num.clone(),
                title_path: h.titre_path.clone(),
                source: h.source.clone(),
            })
            .collect(),
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
        ArticleSearchFacets, ArticleSearchHit, ArticleSearchResponse, BestChunk, JuridictionType,
        LawArticleResponse, LawArticleVersion, QueryMode, SearchHit, SearchResponse,
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
                fv("juridiction:CC", "Cour de cassation"),
                fv("juridiction:CE", "Conseil d'État"),
            ],
            Vec::new(),
        )
    }

    fn base_hit() -> SearchHit {
        SearchHit {
            id: "cc-123".into(),
            juridiction_type: JuridictionType::Cc,
            jurisdiction_name: Some("Cour de cassation".into()),
            title_html: String::new(),
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
            voie: None,
            office: None,
            legal_domain: None,
            publication_codes: vec!["b".into()],
            chars: Some(4200),
            summary: Some("Résumé de la décision.".into()),
        }
    }

    #[test]
    fn lexical_preview_uses_stripped_snippet() {
        let resp = SearchResponse {
            query: "bail".into(),
            total: 1,
            hits: vec![base_hit()],
            query_mode: QueryMode::Lexical,
            facets: None,
            all_hit_ids: vec![],
        };
        let out = present_search_response(&resp, "https://librejustice.fr", &refs());
        let hit = &out.hits[0];
        // mode lexical → snippet sans <mark>, pas le résumé.
        assert_eq!(hit.preview, "un bail commercial");
        assert_eq!(hit.title, "Cour de cassation, 29 mai 2026, 24-17.384");
        assert_eq!(hit.url, "https://librejustice.fr/decision/cc-123");
        assert_eq!(hit.chars, 4200);
        // Voie absente (procédure ordinaire = null, ADR 0146) → champ omis.
        assert_eq!(hit.voie, None);
        // Labels FR déjà résolus (FacetTags servis par l'API).
        assert_eq!(hit.solution.as_deref(), Some("Rejet"));
        // publication "b" → Publié au bulletin.
        assert!(hit.publication.is_some());
    }

    #[test]
    fn hybrid_preview_prefers_summary() {
        let resp = SearchResponse {
            query: "bail".into(),
            total: 1,
            hits: vec![base_hit()],
            query_mode: QueryMode::Hybrid,
            facets: None,
            all_hit_ids: vec![],
        };
        let out = present_search_response(&resp, "https://librejustice.fr", &refs());
        assert_eq!(out.hits[0].preview, "Résumé de la décision.");
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
        let out = present_search_response(&resp, "https://librejustice.fr", &refs());
        assert_eq!(out.hits[0].preview, "un bail commercial");
    }

    #[test]
    fn detail_concatenates_paragraphs_with_blank_line() {
        let detail = DecisionDetail {
            id: "ce-1".into(),
            juridiction_type: JuridictionType::Ce,
            title: String::new(),
            paragraphs: vec!["Para 1.".into(), "Para 2.".into()],
            paragraph_spans: Vec::new(),
            sections: None,
            summary: None,
            jurisdiction_name: None,
            date_lecture: Some("2024-08-06".into()),
            solution: None,
            voie: None,
            office: None,
            legal_domain: None,
            publication_codes: vec![],
            date_audience: None,
            docket_numbers: None,
            formation_or_chamber: None,
            legal_references: None,
            source_xml: None,
            themes: Vec::new(),
            nac: None,
            ecli: None,
            source: None,
            chronology: Vec::new(),
        };
        let out = present_decision_detail(&detail, "https://librejustice.fr", &refs());
        assert_eq!(out.text, "Para 1.\n\nPara 2.");
        assert_eq!(out.url, "https://librejustice.fr/decision/ce-1");
        // jurisdiction_name absent → libellé du type (référentiel `juridiction:*`).
        assert_eq!(out.title, "Conseil d'État, 6 août 2024");
    }

    #[test]
    fn law_article_is_slimmed_and_linked() {
        let article = LawArticleResponse {
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
            texte: Some("Tout fait quelconque de l'homme…".into()),
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
                legiarti: "LEGIARTI000006419292".into(),
                etat: "VIGUEUR".into(),
                date_debut: "2016-10-01".into(),
                date_fin: None,
            }],
            context: vec![],
        };
        let out = present_law_article(&article, "https://librejustice.fr");
        assert_eq!(out.title, "Article 1240 du Code civil");
        assert_eq!(out.url, "https://librejustice.fr/loi/code-civil/1240");
        assert_eq!(
            out.text.as_deref(),
            Some("Tout fait quelconque de l'homme…")
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
            "https://librejustice.fr/loi/code-de-procedure-civile/700"
        );
    }

    #[test]
    fn filters_or_none_drops_empty_object() {
        assert_eq!(filters_or_none(&serde_json::json!({})), None);
        assert_eq!(filters_or_none(&serde_json::Value::Null), None);
        let v = serde_json::json!({"juridictionType": ["CC"]});
        assert_eq!(filters_or_none(&v), Some(v));
    }
}
