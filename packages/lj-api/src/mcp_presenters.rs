//! Turns the internal HTTP DTOs ([`lj_dtos`]) into the MCP outputs, whose
//! schemas are hand-mirrored in [`crate::mcp`].

use lj_dtos::{
    ArticleSearchResponse, CitationSpan, DecisionDetail, FacetChoice, FacetTag, LawArticleResponse,
    QueryMode, SearchFacets, SearchResponse,
};
use serde::Serialize;
use std::collections::BTreeMap;

use crate::referential::Referential;
use crate::titles::decision_title;

/// The raw code, key of the `jurisdiction_type:*` rows of the referential and
/// of the titles.
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
        Cph => "CPH",
        Assises => "ASSISES",
        Ccd => "CCD",
        Cedh => "CEDH",
        Cjue => "CJUE",
        Cnda => "CNDA",
        Cnil => "CNIL",
        Ddd => "DDD",
        Adlc => "ADLC",
        Amf => "AMF",
        Comeu => "COMEU",
        CourSupreme => "COUR_SUPREME",
        CourCassation => "COUR_CASSATION",
        CourConstitutionnelle => "COUR_CONSTITUTIONNELLE",
        CourAppel => "COUR_APPEL",
        CourAdministrative => "COUR_ADMINISTRATIVE",
        TribunalAdministratif => "TRIBUNAL_ADMINISTRATIF",
        TribunalPremiereInstance => "TRIBUNAL_PREMIERE_INSTANCE",
        TribunalCommerce => "TRIBUNAL_COMMERCE",
        CourCriminelle => "COUR_CRIMINELLE",
        CourJustice => "COUR_JUSTICE",
        Ccja => "CCJA",
        CourDroitsHomme => "COUR_DROITS_HOMME",
        AutoriteAdministrative => "AUTORITE_ADMINISTRATIVE",
    }
}

// ── MCP output models ────────────────────────────────────────────────────────

/// Exactly one of `ai_summary` (written by an AI), `court_summary` (the court's own)
/// and `snippet` (verbatim passage) is served per hit.
#[derive(Debug, Clone, Serialize)]
pub struct McpSearchHit {
    pub title: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub official_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub court_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub court_summary_kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    pub chars: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    /// Seat path, reusable verbatim as a filter value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seat: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ruling: Option<String>,
    /// Formation paths, reusable verbatim as filter values.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub formation: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legal_domain: Option<String>,
    /// Empty for a lower court, which carries no publication statement.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub publication: Vec<String>,
    /// Keyed by `legal_instrument` value, holding `legal_article` values; the pair
    /// composes `/norm/{juris}/{slug}/{key}`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub cited_articles: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpSearchResponse {
    pub query: String,
    /// Decisions matched under the current filters, the whole set and not the
    /// served window; the decisions served when the count could not finish.
    pub total: i64,
    pub hits: Vec<McpSearchHit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facets: Option<McpSearchFacets>,
    /// The decisions the query designates by number or by name, apart from the
    /// hits, which answer the question.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pinned: Vec<McpPinned>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpPinned {
    pub title: String,
    pub url: String,
}

/// One `filter token -> count` map per `search_decisions` parameter, under the
/// same key names. No labels: every token is self-describing, the
/// `legal_instrument` slugs included.
#[derive(Debug, Clone, Serialize)]
pub struct McpSearchFacets {
    /// Seat paths and their counts, the whole axis in one map: court types,
    /// then the top courts, then every room a filtered court exposes.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub seat: BTreeMap<String, i64>,
    /// Formation paths and their counts: the court types, then their formations.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub formation: BTreeMap<String, i64>,
    /// Top 10 by count, with no counter of the rest: the vocabulary is already
    /// in the input schema, only the counts are new.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub legal_domain: BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub solution: BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub publication: BTreeMap<String, i64>,
    /// Slugs of the cited texts, top 10 by count.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub legal_instrument: BTreeMap<String, i64>,
    /// Texts past the top 10 of `legal_instrument`, so that the truncation is
    /// announced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_instruments: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpDecisionDetail {
    pub title: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub official_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    pub jurisdiction_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jurisdiction_code: Option<String>,
    /// Chamber key, reusable verbatim as a filter value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seat: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ruling: Option<String>,
    /// Formation paths, reusable verbatim as filter values.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub formation: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legal_domain: Option<String>,
    /// Empty for a lower court, which carries no publication statement.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub publication: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub court_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub court_summary_kind: Option<&'static str>,
    pub standing: lj_dtos::Standing,
    /// Prior and subsequent decisions of the same case, current one included,
    /// most recent first. A step the corpus does not hold carries no `url`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub case_chronology: Vec<McpChronologyStep>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub commentaries: Vec<lj_dtos::Commentaire>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpChronologyStep {
    /// "Cour d'appel de Paris, 23 juin 2026".
    pub title: String,
    /// The court's own page for a case the CEDH communicated; absent when the
    /// corpus does not hold the step, then known only from the text attacking it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub current: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ruling: Option<String>,
    /// Link to the step below, the attacked decision: `APPEL_DE` |
    /// `POURVOI_CONTRE` | `RENVOI_APRES_CASSATION` | `REQUETE_CEDH_CONTRE`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_to_next: Option<String>,
}

/// Status and bounds, without the internal `legiarti` id.
#[derive(Debug, Clone, Serialize)]
pub struct McpLawArticleVersion {
    pub status: String,
    pub valid_from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<String>,
}

/// The internals of the web DTO (`legiarti`/`legitext`, translation
/// provenance, publisher authority, reading context) are left out.
#[derive(Debug, Clone, Serialize)]
pub struct McpLawArticle {
    pub code: String,
    /// Declared before anything long, so a client that cuts the answer short
    /// still leaves the caller a count to compare `articles` against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub article_count: Option<i64>,
    /// Absent when the url addresses a text with no articles (circulaire,
    /// publication decree): the whole text is then served, `text` carrying its
    /// body and `versions` being empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
    pub title: String,
    pub url: String,
    /// The text's own date, its signature; an article carries its validity
    /// instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The division directly enclosing the article, read whole from its `url`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<McpLawSectionRef>,
    pub versions: Vec<McpLawArticleVersion>,
    /// Table of contents of that root, bodies joined below
    /// `INLINE_READING_MAX` articles.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub articles: Vec<McpLawTocEntry>,
    /// Articles the table serves without their body, past the threshold.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub omitted: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub commentaries: Vec<lj_dtos::Commentaire>,
    /// Texts that modified this article, each linking to its own page here:
    /// the hop from an article to the law or decree that wrote it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub modified_by: Vec<lj_dtos::LinkedTextRef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub legislative_history: Vec<lj_dtos::LinkedTextRef>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpLawSectionRef {
    pub label: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpLawSearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_path: Option<String>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<String>,
}

/// Same contract as [`McpSearchFacets`], each axis capped to its top 10, each
/// truncation announced by its counter of the rest.
#[derive(Debug, Clone, Default, Serialize)]
pub struct McpLawSearchFacets {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub code: BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_codes: Option<usize>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub jurisdiction: BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_jurisdictions: Option<usize>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub scope: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpLawSearchResponse {
    pub query: String,
    pub total: i64,
    pub hits: Vec<McpLawSearchHit>,
    pub facets: McpLawSearchFacets,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pinned: Vec<McpPinned>,
}

// ── Presenters ───────────────────────────────────────────────────────────────

/// The preview follows the *resolved* mode (`query_mode`), not the one asked
/// for: lexical serves `snippet`, the verbatim passage with its `<mark>`
/// stripped; hybrid serves the precomputed `hit.summary`, as `court_summary` when the
/// court wrote it and `ai_summary` otherwise, falling back on `snippet` when it is missing. Reference follows hybrid, its legs
/// matching an identity card rather than a passage.
pub fn present_search_response(
    response: &SearchResponse,
    web_base_url: &str,
    seats: &[String],
) -> McpSearchResponse {
    let use_summary = matches!(
        response.query_mode,
        QueryMode::Hybrid | QueryMode::Reference
    );
    let hits = response
        .hits
        .iter()
        .map(|hit| {
            let (ai_summary, court_summary, snippet) = match (
                use_summary,
                hit.summary.as_deref(),
                hit.summary_quote.as_ref(),
            ) {
                (true, Some(summary), Some(quote)) if !summary.is_empty() => {
                    (None, Some(lj_dtos::court_words(summary, quote)), None)
                }
                (true, Some(summary), None) if !summary.is_empty() => {
                    (Some(summary.to_string()), None, None)
                }
                _ => (None, None, Some(strip_marks(&hit.best_chunk.snippet))),
            };
            McpSearchHit {
                // The web hit title already carries the composed bench.
                title: strip_marks(&hit.title_html),
                url: format!("{web_base_url}/decision/{}", hit.id),
                official_url: hit.official_url.clone(),
                ai_summary,
                court_summary_kind: court_summary
                    .as_ref()
                    .map(|_| court_summary_kind(hit.jurisdiction_type)),
                court_summary,
                snippet,
                chars: hit.chars.unwrap_or(0),
                date: hit.date_lecture.clone(),
                docket_numbers: hit.docket_numbers.clone(),
                seat: tag_key(&hit.seat_path),
                solution: solution_root(&hit.solution),
                ruling: hit.solution.as_ref().map(|t| t.label.clone()),
                formation: tag_keys(&hit.formation),
                legal_domain: tag_key(&hit.legal_domain),
                publication: publication_keys(&hit.publication_codes),
                cited_articles: hit.cited_articles.clone(),
            }
        })
        .collect();
    // Facets only help refining when the total overflows the served page.
    let facets = response
        .facets
        .as_ref()
        .filter(|_| response.total.unwrap_or_default() > response.hits.len() as i64)
        .and_then(|f| present_search_facets(f, seats));
    let pinned = response
        .pinned
        .iter()
        .map(|d| McpPinned {
            title: d.title.clone(),
            url: format!("{web_base_url}/decision/{}", d.id),
        })
        .collect();
    McpSearchResponse {
        query: response.query.clone(),
        total: response.total.unwrap_or_default(),
        hits,
        facets,
        pinned,
    }
}

const FACET_INSTRUMENTS_LIMIT: usize = 10;
const FACET_OPEN_AXIS_LIMIT: usize = 10;
const FACET_SEAT_RANK_LIMIT: usize = 20;

/// `None` when every axis is empty.
fn present_search_facets(f: &SearchFacets, seats: &[String]) -> Option<McpSearchFacets> {
    fn counts(choices: &[FacetChoice]) -> BTreeMap<String, i64> {
        choices.iter().map(|c| (c.value.clone(), c.count)).collect()
    }
    fn top(choices: &[FacetChoice]) -> BTreeMap<String, i64> {
        let mut sorted: Vec<&FacetChoice> = choices.iter().collect();
        sorted.sort_by_key(|c| std::cmp::Reverse(c.count));
        sorted
            .into_iter()
            .take(FACET_OPEN_AXIS_LIMIT)
            .map(|c| (c.value.clone(), c.count))
            .collect()
    }
    // The orders with their court types, then the rank right below each
    // filtered seat. Several seats are OR'd, so each opens its own. Serving
    // every depth instead would be the whole referential.
    let depth = |c: &FacetChoice| c.value.split('.').count();
    let mut ranks: BTreeMap<&str, Vec<&FacetChoice>> = BTreeMap::new();
    for c in f.seat.iter().filter(|c| {
        depth(c) <= 2
            || seats.iter().any(|s| {
                c.value == *s
                    || (c.value.starts_with(s.as_str())
                        && c.value.as_bytes().get(s.len()) == Some(&b'.')
                        && depth(c) == s.split('.').count() + 1)
            })
    }) {
        let parent = c.value.rsplit_once('.').map_or("", |(p, _)| p);
        ranks.entry(parent).or_default().push(c);
    }
    // A rank is capped against its siblings, never against another rank: a
    // room's count cannot exceed its court's, so one shared cap would serve
    // courts and never a room. At 20 every order's types and nearly every room
    // rank pass whole; only the city lists are really cut, and a path composes
    // without the facet naming it.
    let seat: BTreeMap<String, i64> = ranks
        .into_values()
        .flat_map(|mut rank| {
            rank.sort_unstable_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
            rank.truncate(FACET_SEAT_RANK_LIMIT);
            rank.into_iter().map(|c| (c.value.clone(), c.count))
        })
        .collect();
    // The token is the text slug, the only key the filters accept: an entry
    // without one is not filterable, so it is not shown and does not eat a slot.
    let mut instruments: Vec<(&String, i64)> = f
        .legal_instrument
        .iter()
        .filter_map(|l| l.slug.as_ref().map(|s| (s, l.count)))
        .collect();
    instruments.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    let other_instruments = instruments
        .len()
        .checked_sub(FACET_INSTRUMENTS_LIMIT)
        .filter(|n| *n > 0);
    let legal_instrument: BTreeMap<String, i64> = instruments
        .into_iter()
        .take(FACET_INSTRUMENTS_LIMIT)
        .map(|(slug, count)| (slug.clone(), count))
        .collect();
    let facets = McpSearchFacets {
        seat,
        formation: counts(&f.formation),
        legal_domain: top(&f.legal_domain),
        solution: counts(&f.solution),
        publication: counts(&f.publication),
        legal_instrument,
        other_instruments,
    };
    let empty = facets.seat.is_empty()
        && facets.formation.is_empty()
        && facets.legal_domain.is_empty()
        && facets.solution.is_empty()
        && facets.publication.is_empty()
        && facets.legal_instrument.is_empty();
    (!empty).then_some(facets)
}

/// Falls back on the raw code when the referential has no entry.
fn type_label(jt: lj_dtos::JurisdictionType, refs: &Referential) -> &str {
    let code = jurisdiction_type_code(jt);
    refs.jurisdiction_type_label(code).unwrap_or(code)
}

/// `FacetTag.key` is a uid suffix, exactly what the `search_decisions` filters
/// accept.
/// The root of a decision's solution, the value the filter takes; the tag is its leaf.
fn solution_root(tag: &Option<FacetTag>) -> Option<String> {
    tag.as_ref().and_then(|t| root_code(&t.key))
}

fn root_code(leaf: &str) -> Option<String> {
    let root = lj_dtos::Solution::of_leaf(leaf)?;
    serde_json::to_value(root)
        .ok()?
        .as_str()
        .map(str::to_string)
}

fn tag_key(tag: &Option<FacetTag>) -> Option<String> {
    tag.as_ref().map(|t| t.key.clone())
}

fn tag_keys(tags: &[FacetTag]) -> Vec<String> {
    tags.iter().map(|t| t.key.clone()).collect()
}

/// Membership is multiple: a decision can carry several publication values.
fn publication_keys(codes: &[String]) -> Vec<String> {
    lj_core::publication::publication_keys(codes)
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// What the court calls its own summary, as the decision page names it.
fn court_summary_kind(jt: lj_dtos::JurisdictionType) -> &'static str {
    use lj_dtos::JurisdictionType::*;
    match jt {
        Ce | Caa | Ta | Tc => "Analyse",
        Cedh => "Résumé juridique",
        _ => "Sommaire",
    }
}

/// The resolved citations of the body (`paragraph_spans`, the ones the front
/// makes clickable) are rendered as inline markdown links in `text`.
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
                None,
                e.docket_numbers
                    .as_deref()
                    .and_then(lj_core::titles::docket_display)
                    .as_deref(),
            ),
            url: e
                .id
                .as_ref()
                .map(|id| format!("{web_base_url}/decision/{id}"))
                .or_else(|| e.external_url.clone()),
            current: e.current,
            solution: e.solution.as_deref().and_then(root_code),
            ruling: e
                .solution
                .as_deref()
                .map(|leaf| refs.label(&format!("solution:{leaf}")).to_string()),
            link_to_next: e.link.clone(),
        })
        .collect();
    let title = decision_title(
        type_label(detail.jurisdiction_type, refs),
        detail.jurisdiction_name.as_deref(),
        detail.seat.as_deref(),
        detail.date_lecture.as_deref(),
        detail.case_name.as_deref(),
        detail.docket_numbers.as_deref(),
    );
    let (summaries, commentaries): (Vec<_>, Vec<_>) = detail
        .commentaires
        .iter()
        .cloned()
        .partition(|c| c.kind == "summary");
    let court_summary = summaries.into_iter().find_map(|c| c.body);
    McpDecisionDetail {
        title,
        url: format!("{web_base_url}/decision/{}", detail.id),
        official_url: detail.official_url.clone(),
        date: detail.date_lecture.clone(),
        docket_numbers: detail.docket_numbers.clone(),
        jurisdiction_type: jurisdiction_type_code(detail.jurisdiction_type).to_string(),
        jurisdiction_code: detail.jurisdiction_code.clone(),
        seat: tag_key(&detail.seat_path),
        solution: solution_root(&detail.solution),
        ruling: detail.solution.as_ref().map(|t| t.label.clone()),
        formation: tag_keys(&detail.formation),
        legal_domain: tag_key(&detail.legal_domain),
        publication: publication_keys(&detail.publication_codes),
        court_summary_kind: court_summary
            .as_ref()
            .map(|_| court_summary_kind(detail.jurisdiction_type)),
        court_summary,
        standing: lj_dtos::Standing::of(&detail.chronology, detail.jurisdiction_type),
        case_chronology,
        // An AI's summary of conclusions is never `body`, which holds a court's own text.
        commentaries: commentaries
            .into_iter()
            .map(|mut c| {
                if c.kind == "conclusions" {
                    c.body = None;
                }
                c
            })
            .collect(),
        text,
    }
}

/// Wraps every resolved citation into a `[text](url)` markdown link, without
/// rewriting the span. On several targets, a range or a merge of overlaps, the
/// first resolved one wraps the span and the others are appended right after as
/// labelled links, so every article of the range stays openable. A span with no
/// resolved target stays plain.
///
/// Offsets are codepoints local to the paragraph, and merged spans are ordered
/// and non-overlapping: an invariant of the producer, not checked here.
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

/// "Article {num} du/de la/de l'{codeTitle}", the contraction coming from
/// [`lj_dtos::instrument_with_de`], "Article {num}, {codeTitle}" for a title in
/// another language, "Article {num}" when the code title is missing. A whole
/// text has no number, and its title is its own.
fn law_article_title(num: &str, code_title: Option<&str>) -> String {
    match code_title {
        Some(t) if num.is_empty() => t.to_string(),
        Some(t) if !t.is_empty() => match lj_dtos::instrument_with_de(t) {
            Some(of_text) => format!("Article {num} {of_text}"),
            None => format!("Article {num}, {t}"),
        },
        _ => format!("Article {num}"),
    }
}

/// The page of an article, or of the whole text when `num_key` is empty.
fn law_url(web_base_url: &str, jurisdiction: &str, code: &str, num_key: &str) -> String {
    let text = lj_vocab::norm_path(jurisdiction, code);
    match num_key {
        "" => format!("{web_base_url}{text}"),
        key => format!("{web_base_url}{text}/{key}"),
    }
}

/// The resolved cross-references of the body (`texte_spans`, with dated hrefs
/// when the article is served at a date) are rendered as inline markdown links
/// in `text`, like the citations of a decision.
pub fn present_law_article(article: &LawArticleResponse, web_base_url: &str) -> McpLawArticle {
    McpLawArticle {
        title: law_article_title(&article.num, article.code_title.as_deref()),
        url: format!(
            "{web_base_url}{}/{}",
            lj_vocab::norm_path(&article.jurisdiction, &article.code),
            article.num_key
        ),
        code: article.code.clone(),
        number: Some(article.num.clone()),
        date: None,
        status: Some(article.etat.clone()),
        valid_from: Some(article.date_debut.clone()),
        valid_to: article.date_fin.clone(),
        text: article
            .texte
            .as_ref()
            .map(|t| paragraph_with_links(t, &article.texte_spans, web_base_url)),
        source_url: article.source_url.clone(),
        note: article.nota.clone(),
        section: article.breadcrumb.last().and_then(|division| {
            Some(McpLawSectionRef {
                label: division.label.clone(),
                url: format!("{web_base_url}{}", division.href.as_deref()?),
            })
        }),
        versions: article
            .versions
            .iter()
            .map(|v| McpLawArticleVersion {
                status: v.etat.clone(),
                valid_from: v.date_debut.clone(),
                valid_to: v.date_fin.clone(),
            })
            .collect(),
        article_count: None,
        articles: Vec::new(),
        omitted: None,
        commentaries: article.commentaires.clone(),
        modified_by: article.modified_by.clone(),
        legislative_history: article.travaux_parlementaires.clone(),
    }
}

/// A text without articles, served in the article shape:
/// a missing `number` says there is no article to ask for, and `text` carries the
/// monolithic body, the only way to read those.
pub fn present_law_text(summary: &lj_dtos::LawCodeSummary, web_base_url: &str) -> McpLawArticle {
    McpLawArticle {
        title: summary.titre.clone(),
        url: format!(
            "{web_base_url}{}",
            lj_vocab::norm_path(&summary.jurisdiction, &summary.code)
        ),
        code: summary.code.clone(),
        number: None,
        date: summary.date_texte.clone(),
        status: summary.status.clone(),
        valid_from: None,
        valid_to: None,
        text: summary.body.clone(),
        source_url: None,
        note: None,
        section: None,
        versions: Vec::new(),
        article_count: None,
        articles: Vec::new(),
        omitted: None,
        commentaries: summary.commentaires.clone(),
        modified_by: Vec::new(),
        legislative_history: Vec::new(),
    }
}

/// `url` is passed back to `get_norm`.
#[derive(Debug, Clone, Serialize)]
pub struct McpLawTocEntry {
    /// `article` or `section`, both passed back to `get_norm`, a section
    /// serving all of its articles at once.
    pub kind: &'static str,
    pub number: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// Attached only on a short text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// The table of contents of a text with articles, bodies attached when
/// `code_toc` attached them, under the product's `INLINE_READING_MAX`.
pub fn present_law_text_root(
    summary: &lj_dtos::LawCodeSummary,
    toc: &lj_dtos::CodeTocResponse,
    web_base_url: &str,
) -> McpLawArticle {
    let code = &summary.code;
    let text = lj_vocab::norm_path(&summary.jurisdiction, code);
    let link = |num_key: &str| format!("{web_base_url}{text}/{num_key}");
    let section_link = |path: &str| format!("{web_base_url}{text}/{path}");
    let mut articles: Vec<McpLawTocEntry> = Vec::new();
    let mut section = None;
    for item in &toc.reading {
        match item.num_key.as_deref() {
            Some(num_key) => articles.push(McpLawTocEntry {
                kind: "article",
                number: item.label.clone(),
                url: link(num_key),
                section: section.clone(),
                text: item
                    .texte
                    .as_deref()
                    .map(|t| paragraph_with_links(t, &item.texte_spans, web_base_url)),
            }),
            None => section = Some(item.label.clone()),
        }
    }
    if articles.is_empty() {
        // On a large text the section is what one call reads, so it enters the
        // toc with its address, not just as a heading.
        let mut path = None;
        for node in &toc.tree {
            match (node.num_key.as_deref(), node.path.as_deref()) {
                (Some(num_key), _) => articles.push(McpLawTocEntry {
                    kind: "article",
                    number: node.label.clone(),
                    url: link(num_key),
                    section: path.clone(),
                    text: None,
                }),
                (None, Some(section_path)) => {
                    articles.push(McpLawTocEntry {
                        kind: "section",
                        number: node.label.clone(),
                        url: section_link(section_path),
                        section: path.clone(),
                        text: None,
                    });
                    path = Some(node.label.clone());
                }
                (None, None) => path = Some(node.label.clone()),
            }
        }
    }
    if articles.is_empty() {
        articles.extend(toc.entries.iter().map(|e| McpLawTocEntry {
            kind: "article",
            number: e.num.clone(),
            url: link(&e.num_key),
            section: e.title_path.clone(),
            text: None,
        }));
    }
    let total = articles.len();
    articles.truncate(lj_dtos::INLINE_READING_MAX);
    let omitted = total - articles.len();
    McpLawArticle {
        code: code.clone(),
        article_count: Some(summary.article_count),
        number: None,
        title: summary.titre.clone(),
        url: format!("{web_base_url}{text}"),
        date: summary.date_texte.clone(),
        status: summary.status.clone(),
        valid_from: None,
        valid_to: None,
        text: None,
        source_url: None,
        note: None,
        section: None,
        versions: Vec::new(),
        articles,
        omitted: (omitted > 0).then_some(omitted),
        commentaries: summary.commentaires.clone(),
        modified_by: Vec::new(),
        legislative_history: Vec::new(),
    }
}

/// The articles of a section in reading order, bodies attached as long as the
/// section fits the reading bounds, otherwise reduced to their index and
/// `omitted` says so. Same shape as an article, without `number`.
pub fn present_law_section(
    section: &lj_dtos::LawSectionResponse,
    web_base_url: &str,
) -> McpLawArticle {
    let code = &section.code;
    let text = lj_vocab::norm_path(&section.jurisdiction, code);
    let mut path: Option<String> = section
        .ancestors
        .last()
        .map(|a| a.label.clone())
        .or_else(|| section.code_title.clone());
    let mut articles = Vec::new();
    for item in &section.items {
        match item.num_key.as_deref() {
            Some(num_key) => articles.push(McpLawTocEntry {
                kind: "article",
                number: item.label.clone(),
                url: format!("{web_base_url}{text}/{num_key}"),
                section: path.clone(),
                text: item
                    .texte
                    .as_deref()
                    .map(|t| paragraph_with_links(t, &item.texte_spans, web_base_url)),
            }),
            None => path = Some(item.label.clone()),
        }
    }
    McpLawArticle {
        code: code.clone(),
        article_count: Some(articles.len() as i64),
        number: None,
        title: section.title.clone(),
        url: format!("{web_base_url}{text}/{}", section.path),
        date: None,
        status: None,
        valid_from: None,
        valid_to: None,
        text: None,
        source_url: None,
        note: None,
        section: None,
        versions: Vec::new(),
        articles,
        omitted: section.omitted,
        commentaries: Vec::new(),
        modified_by: Vec::new(),
        legislative_history: Vec::new(),
    }
}

const LAW_FACET_LIMIT: usize = 10;

/// The response carries its total: the connector and the public door ask for it.
pub fn present_law_search(
    query: &str,
    response: &ArticleSearchResponse,
    web_base_url: &str,
) -> McpLawSearchResponse {
    McpLawSearchResponse {
        query: query.to_string(),
        total: response
            .total
            .expect("a norms search presented to a caller carries its total"),
        hits: response
            .hits
            .iter()
            .map(|h| McpLawSearchHit {
                title: law_article_title(&h.num, Some(&h.code_title)),
                url: law_url(web_base_url, &h.jurisdiction, &h.code, &h.num_key),
                snippet: h.snippet.clone(),
                title_path: h.titre_path.clone(),
                source: h.source.clone(),
                valid_from: h.valid_from.clone(),
                valid_to: h.valid_to.clone(),
            })
            .collect(),
        facets: response
            .facets
            .as_ref()
            .map(present_law_facets)
            .unwrap_or_default(),
        pinned: response
            .pinned
            .iter()
            .map(|a| McpPinned {
                title: law_article_title(&a.num, Some(&a.code_title)),
                url: law_url(web_base_url, &a.jurisdiction, &a.code, &a.num_key),
            })
            .collect(),
    }
}

/// Each facet is capped at ten, the slugs arriving sorted by decreasing count.
fn present_law_facets(facets: &lj_dtos::ArticleSearchFacets) -> McpLawSearchFacets {
    // A zero on the jurisdiction axis is a position outside the searched corpora.
    let counts = |choices: &[lj_dtos::FacetChoice], cap: usize| {
        choices
            .iter()
            .filter(|c| c.count > 0)
            .take(cap)
            .map(|c| (c.value.clone(), c.count))
            .collect::<BTreeMap<_, _>>()
    };
    let rest = |choices: &[lj_dtos::FacetChoice], cap: usize| {
        choices
            .iter()
            .filter(|c| c.count > 0)
            .count()
            .checked_sub(cap)
            .filter(|n| *n > 0)
    };
    // The orders keep their own cap, the types under them take theirs.
    let typed: Vec<lj_dtos::FacetChoice> = facets
        .path
        .iter()
        .filter(|c| c.value.contains('.'))
        .cloned()
        .collect();
    McpLawSearchFacets {
        code: counts(&facets.code, LAW_FACET_LIMIT),
        other_codes: rest(&facets.code, LAW_FACET_LIMIT),
        jurisdiction: counts(&facets.jurisdiction, LAW_FACET_LIMIT)
            .into_iter()
            .chain(counts(&typed, LAW_FACET_LIMIT))
            .collect(),
        other_jurisdictions: match (
            rest(&facets.jurisdiction, LAW_FACET_LIMIT),
            rest(&typed, LAW_FACET_LIMIT),
        ) {
            (None, None) => None,
            (orders, types) => Some(orders.unwrap_or(0) + types.unwrap_or(0)),
        },
        scope: counts(&facets.scope, lj_vocab::NORM_CATEGORIES.len()),
    }
}

// ── Annonces ────────────────────────────────────────────────────────────────

/// Enough to decide whether to go, and where to go: `url` is our page,
/// `platform_url` the filing platform. The deep links, tender rules and tender
/// documents, stay on the page: an MCP client downloads no tender file.
#[derive(Debug, Clone, Serialize)]
pub struct McpJournalHit {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The corpus the notice comes from (`marche`, `bodacc`, `joafe`, `jo`).
    pub fund: String,
    /// An official journal's act: the journal that printed it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub journal: Option<String>,
    /// Who the notice is about, and how its corpus names them.
    pub actor: String,
    pub actor_role: String,
    pub url: String,
    /// A tender's alone: another journal's `url_action` is its official page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform_url: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub zones: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpv: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    pub published: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// The channels publishing the notice (`boamp`, `atexo`…).
    pub channels: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpJournalSearchResponse {
    pub total: i64,
    pub hits: Vec<McpJournalHit>,
}

pub fn present_journal_search(
    response: &lj_dtos::JournalSearchResponse,
    web_base_url: &str,
) -> McpJournalSearchResponse {
    McpJournalSearchResponse {
        total: response.total,
        hits: response
            .groupes
            .iter()
            .flat_map(|g| g.hits.iter())
            .map(|h| McpJournalHit {
                title: h.titre.clone(),
                description: h.description.clone(),
                fund: h.fond.clone(),
                journal: h.journal.clone(),
                actor: h.acteur.clone(),
                actor_role: h.acteur_role.clone(),
                url: annonce_url(web_base_url, h),
                platform_url: (h.fond == lj_vocab::JOURNAL_FOND_MARCHE && !h.url_action.is_empty())
                    .then(|| h.url_action.clone()),
                zones: h.zones.iter().map(|z| z.label.clone()).collect(),
                nature: h.famille.as_ref().map(|f| f.label.clone()),
                cpv: h.cpv.as_ref().map(|c| c.label.clone()),
                deadline: h.echeance.clone(),
                published: h.date_publication.clone(),
                reference: h.reference.clone(),
                channels: h.canaux.clone(),
            })
            .collect(),
    }
}

fn annonce_url(web_base_url: &str, hit: &lj_dtos::JournalHit) -> String {
    format!(
        "{web_base_url}{}",
        lj_vocab::notice_path(&hit.jurisdiction, &hit.uid)
    )
}

/// One notice read whole: a publication fills its text, a register notice its
/// fields, a tender its buyer, deadline and lots.
#[derive(Debug, Clone, Default, Serialize)]
pub struct McpNotice {
    pub title: String,
    pub url: String,
    pub journal: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub official_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub norm_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<McpNoticeEntity>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<McpNoticeField>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanatory_note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visas: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub articles: Vec<McpNoticeArticle>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signatories: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub corrigendum: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buyer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub zones: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpv: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub procedure: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lots: Vec<McpNoticeLot>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub criteria: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_amount: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpNoticeEntity {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpNoticeField {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    pub label: String,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpNoticeArticle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpNoticeLot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpv: Option<String>,
}

/// Our paths absolute, an official address as it came.
fn absolute(web_base_url: &str, href: &str) -> String {
    match href.starts_with('/') {
        true => format!("{web_base_url}{href}"),
        false => href.to_string(),
    }
}

pub fn present_publication_notice(n: &lj_dtos::PublicationNotice, web_base_url: &str) -> McpNotice {
    let p = &n.publication;
    McpNotice {
        title: n.title.clone(),
        url: format!(
            "{web_base_url}{}",
            lj_vocab::notice_path(&n.jurisdiction, &n.uid)
        ),
        journal: n.journal_label.clone().unwrap_or_else(|| n.journal.clone()),
        issue: p.issue.as_ref().map(|i| i.label.clone()),
        published: n.date_publi.clone(),
        date: n.date_texte.clone(),
        nature: n.nature.clone(),
        nor: n.nor.clone(),
        issuer: n.issuer.clone(),
        official_url: n.source_url.clone(),
        norm_url: n
            .norm
            .as_ref()
            .and_then(|r| r.href.as_deref())
            .map(|h| absolute(web_base_url, h)),
        entity: n.entity.as_ref().and_then(|e| {
            Some(McpNoticeEntity {
                name: e.label.clone(),
                url: absolute(web_base_url, e.href.as_deref()?),
            })
        }),
        fields: n
            .fields
            .iter()
            .map(|f| McpNoticeField {
                part: f.group.clone(),
                label: f.label.clone(),
                value: f.value.clone(),
                url: f.href.as_deref().map(|h| absolute(web_base_url, h)),
            })
            .collect(),
        explanatory_note: p.notice.clone(),
        summary: p.summary.clone(),
        visas: p.visas.clone(),
        articles: n
            .articles
            .iter()
            .map(|a| McpNoticeArticle {
                number: a.num.clone(),
                text: a.text.clone(),
            })
            .collect(),
        body: n.body.clone(),
        signatories: p.signatories.clone(),
        corrigendum: p.corrigendum.clone(),
        repeal: p.repeal.clone(),
        ..McpNotice::default()
    }
}

pub fn present_tender_notice(d: &lj_dtos::AnnonceDetailResponse, web_base_url: &str) -> McpNotice {
    let a = &d.annonce;
    McpNotice {
        title: a.titre.clone(),
        url: annonce_url(web_base_url, a),
        journal: a.fond_label.clone(),
        published: a.date_publication.get(..10).map(str::to_string),
        nature: a.famille.as_ref().map(|f| f.label.clone()),
        summary: a.description.clone(),
        buyer: Some(a.acteur.clone()).filter(|b| !b.is_empty()),
        platform_url: Some(a.url_action.clone()).filter(|u| !u.is_empty()),
        deadline: a.echeance.clone(),
        zones: a.zones.iter().map(|z| z.label.clone()).collect(),
        cpv: a.cpv.as_ref().map(|c| c.label.clone()),
        procedure: a.procedure.clone(),
        reference: a.reference.clone(),
        lots: d
            .lots_ouverts
            .iter()
            .map(|l| McpNoticeLot {
                name: l.nom.clone(),
                description: l.description.clone(),
                cpv: l.cpv.clone(),
            })
            .collect(),
        criteria: d.criteres.clone(),
        estimated_amount: d.montant_estime.as_ref().map(|m| m.valeur),
        currency: d.montant_estime.as_ref().and_then(|m| m.devise.clone()),
        ..McpNotice::default()
    }
}

fn strip_marks(text: &str) -> String {
    text.replace("<mark>", "").replace("</mark>", "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_dtos::{
        ArticleSearchFacets, ArticleSearchHit, ArticleSearchResponse, BestChunk, CitationTarget,
        JurisdictionType, LawArticleResponse, LawArticleVersion, PinnedDecision, QueryMode,
        SearchHit, SearchResponse,
    };
    use lj_store::repository::FacetValueRow;

    /// The type labels and the tags the fixtures need.
    fn refs() -> Referential {
        let fv = |uid: &str, label: &str| FacetValueRow {
            uid: uid.to_string(),
            facet: uid.split(':').next().unwrap().to_string(),
            label: label.to_string(),
            abbr: None,
            parent_uid: None,
            sort: 0,
            hint: None,
        };
        Referential::new(
            vec![
                fv("jurisdiction_type:CC", "Cour de cassation"),
                fv("jurisdiction_type:CE", "Conseil d'État"),
            ],
            Vec::new(),
            Vec::new(),
        )
    }

    fn tag(key: &str, libelle: &str) -> FacetTag {
        FacetTag {
            key: key.into(),
            label: libelle.into(),
        }
    }

    fn base_hit() -> SearchHit {
        SearchHit {
            court_names: Default::default(),
            id: "cc-123".into(),
            jurisdiction_type: JurisdictionType::Cc,
            jurisdiction_code: Some("cc".into()),
            jurisdiction_name: Some("Cour de cassation".into()),
            title_html: "Cour de <mark>cassation</mark>, 29 mai 2026, 24-17.384".into(),
            seat: None,
            seat_parts: None,
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
            formation: Vec::new(),
            legal_domain: None,
            seat_path: Some(FacetTag {
                key: "CC.SOCIALE".into(),
                label: "Chambre sociale".into(),
            }),
            publication_codes: vec!["b".into()],
            chars: Some(4200),
            summary: Some("Résumé de la décision.".into()),
            summary_quote: None,
            summary_body: None,
            official_url: Some(
                "https://www.courdecassation.fr/decision/6194ba445a317cc1d116fbb9".into(),
            ),
            cited_articles: BTreeMap::from([("code-civil".into(), vec!["1240".into()])]),
            matched_docket: None,
        }
    }

    #[test]
    fn lexical_serves_stripped_snippet() {
        let resp = SearchResponse {
            query: "bail".into(),
            total: Some(1),
            hits: vec![base_hit()],
            query_mode: QueryMode::Lexical,
            facets: None,
            all_hit_paths: vec![],
            reranked: false,
            pinned: Vec::new(),
            pinned_more: false,
        };
        let out = present_search_response(&resp, "https://librejustice.fr", &[]);
        let hit = &out.hits[0];
        assert_eq!(hit.snippet.as_deref(), Some("un bail commercial"));
        assert_eq!(hit.ai_summary, None);
        assert_eq!(hit.title, "Cour de cassation, 29 mai 2026, 24-17.384");
        assert_eq!(hit.url, "https://librejustice.fr/decision/cc-123");
        assert_eq!(hit.chars, 4200);
        assert!(hit.formation.is_empty());
        assert_eq!(hit.seat.as_deref(), Some("CC.SOCIALE"));
        assert_eq!(hit.solution.as_deref(), Some("DISMISSED"));
        assert_eq!(hit.ruling.as_deref(), Some("Rejet"));
        assert_eq!(hit.publication, vec!["PUBLIE_BULLETIN".to_string()]);
    }

    #[test]
    fn hybrid_serves_ai_summary() {
        let resp = SearchResponse {
            query: "bail".into(),
            total: Some(1),
            hits: vec![base_hit()],
            query_mode: QueryMode::Hybrid,
            facets: None,
            all_hit_paths: vec![],
            reranked: false,
            pinned: Vec::new(),
            pinned_more: false,
        };
        let out = present_search_response(&resp, "https://librejustice.fr", &[]);
        assert_eq!(
            out.hits[0].ai_summary.as_deref(),
            Some("Résumé de la décision.")
        );
        assert_eq!(out.hits[0].snippet, None);
    }

    /// A collage serves the court's words alone, a pasted summary whole, its cut marked.
    #[test]
    fn hybrid_serves_the_court_s_summary_apart() {
        let present = |summary: &str, quote: std::ops::Range<usize>| {
            let mut hit = base_hit();
            hit.summary = Some(summary.into());
            hit.summary_quote = Some(quote);
            let resp = SearchResponse {
                query: "bail".into(),
                total: Some(1),
                hits: vec![hit],
                query_mode: QueryMode::Hybrid,
                facets: None,
                all_hit_paths: vec![],
                reranked: false,
                pinned: Vec::new(),
                pinned_more: false,
            };
            let out = present_search_response(&resp, "https://librejustice.fr", &[]);
            (
                out.hits[0].ai_summary.clone(),
                out.hits[0].court_summary.clone(),
            )
        };
        assert_eq!(
            present(
                "La Cour juge le litige. Le bail est résilié. Rejet.",
                24..44
            ),
            (None, Some("Le bail est résilié.".into()))
        );
        assert_eq!(
            present("Le bail est résilié. […]", 0..20),
            (None, Some("Le bail est résilié. […]".into()))
        );
    }

    #[test]
    fn hybrid_without_summary_falls_back_to_snippet() {
        let mut hit = base_hit();
        hit.summary = None;
        let resp = SearchResponse {
            query: "bail".into(),
            total: Some(1),
            hits: vec![hit],
            query_mode: QueryMode::Hybrid,
            facets: None,
            all_hit_paths: vec![],
            reranked: false,
            pinned: Vec::new(),
            pinned_more: false,
        };
        let out = present_search_response(&resp, "https://librejustice.fr", &[]);
        assert_eq!(out.hits[0].snippet.as_deref(), Some("un bail commercial"));
        assert_eq!(out.hits[0].ai_summary, None);
    }

    #[test]
    fn facets_compact_to_filter_tokens() {
        let choice = |value: &str, count: i64, parent: Option<&str>| FacetChoice {
            value: value.into(),
            label: format!("Label {value}"),
            count,
            parent: parent.map(str::to_string),
            hint: None,
        };
        let mut seat = vec![choice("FR", 74, None), choice("FR.TA", 74, Some("FR"))];
        for i in 0..17 {
            seat.push(choice(&format!("FR.TA.VILLE{i:02}"), 17 - i, Some("FR.TA")));
        }
        seat.push(choice("FR.TA.VILLE00.C1", 4, Some("FR.TA.VILLE00")));
        seat.push(choice("FR.TA.VILLE00.C1.S2", 2, Some("FR.TA.VILLE00.C1")));
        seat.push(choice("FR.TA.VILLE01.C7", 3, Some("FR.TA.VILLE01")));
        let domaines: Vec<FacetChoice> = (0..13)
            .map(|i| choice(&format!("DOMAINE{i:02}"), 13 - i, None))
            .collect();
        let facets = SearchFacets {
            seat,
            legal_domain: domaines,
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
            total: Some(92),
            hits: vec![base_hit()],
            query_mode: QueryMode::Hybrid,
            facets: Some(facets.clone()),
            all_hit_paths: vec![],
            reranked: false,
            pinned: Vec::new(),
            pinned_more: false,
        };
        let out = present_search_response(&resp, "https://librejustice.fr", &[]);
        let f = out.facets.expect("facets présentes (total > hits)");
        assert_eq!(f.seat.get("FR.TA"), Some(&74));
        assert_eq!(f.seat.len(), 2);
        assert_eq!(f.solution.get("REJET"), Some(&63));
        assert_eq!(f.legal_instrument.get("code-civil"), Some(&657));
        assert_eq!(f.legal_domain.len(), 10);
        assert_eq!(f.legal_domain.get("DOMAINE00"), Some(&13));
        assert_eq!(f.legal_domain.get("DOMAINE12"), None);

        let asked = present_search_response(
            &resp,
            "https://librejustice.fr",
            &["FR.TA.VILLE00".to_string()],
        );
        let f = asked.facets.expect("facets présentes (total > hits)");
        assert_eq!(f.seat.get("FR.TA.VILLE00"), Some(&17));
        assert_eq!(f.seat.get("FR.TA.VILLE00.C1"), Some(&4));
        assert_eq!(f.seat.get("FR.TA.VILLE00.C1.S2"), None);
        assert_eq!(f.seat.get("FR.TA.VILLE01"), None);
        assert_eq!(f.seat.len(), 4);

        let multi = present_search_response(
            &resp,
            "https://librejustice.fr",
            &["FR.TA.VILLE00.C1".to_string(), "FR.TA.VILLE01".to_string()],
        );
        let f = multi.facets.expect("facets présentes (total > hits)");
        assert_eq!(f.seat.get("FR.TA.VILLE00.C1"), Some(&4));
        assert_eq!(f.seat.get("FR.TA.VILLE00.C1.S2"), Some(&2));
        assert_eq!(f.seat.get("FR.TA.VILLE01.C7"), Some(&3));
        assert_eq!(f.seat.len(), 6);

        let racine =
            present_search_response(&resp, "https://librejustice.fr", &["FR.TA".to_string()]);
        let f = racine.facets.expect("facets présentes (total > hits)");
        assert_eq!(f.seat.len(), 19);
        assert_eq!(f.seat.get("FR.TA.VILLE16"), Some(&1));

        let resp = SearchResponse {
            query: "bail".into(),
            total: Some(1),
            hits: vec![base_hit()],
            query_mode: QueryMode::Hybrid,
            facets: Some(facets),
            all_hit_paths: vec![],
            reranked: false,
            pinned: Vec::new(),
            pinned_more: false,
        };
        let out = present_search_response(&resp, "https://librejustice.fr", &[]);
        assert!(out.facets.is_none());
    }

    #[test]
    fn a_deep_rank_is_not_capped_against_a_shallower_one() {
        let choice = |value: &str, count: i64| FacetChoice {
            value: value.into(),
            label: format!("Label {value}"),
            count,
            parent: None,
            hint: None,
        };
        let mut seat = vec![choice("FR", 74), choice("FR.TA", 74)];
        for i in 0..24 {
            seat.push(choice(&format!("FR.TA.VILLE{i:02}"), 24 - i));
        }
        seat.push(choice("FR.TA.VILLE00.C1", 4));
        seat.push(choice("FR.TA.VILLE00.C9", 2));
        let resp = SearchResponse {
            query: "bail".into(),
            total: Some(92),
            hits: vec![base_hit()],
            query_mode: QueryMode::Hybrid,
            facets: Some(SearchFacets {
                seat,
                ..Default::default()
            }),
            all_hit_paths: vec![],
            reranked: false,
            pinned: Vec::new(),
            pinned_more: false,
        };
        let out = present_search_response(
            &resp,
            "https://librejustice.fr",
            &["FR.TA".to_string(), "FR.TA.VILLE00".to_string()],
        );
        let f = out.facets.expect("facets présentes (total > hits)");
        assert_eq!(f.seat.get("FR.TA.VILLE19"), Some(&5));
        assert_eq!(f.seat.get("FR.TA.VILLE20"), None);
        // A room thinner than a dropped city stays: its rank has its own cap.
        assert_eq!(f.seat.get("FR.TA.VILLE00.C9"), Some(&2));
        assert_eq!(f.seat.len(), 24);
    }

    #[test]
    fn detail_concatenates_paragraphs_with_blank_line() {
        let detail = DecisionDetail {
            court_names: Default::default(),
            id: "ce-1".into(),
            jurisdiction_type: JurisdictionType::Ce,
            title: String::new(),
            case_name: None,
            paragraphs: vec!["Para 1.".into(), "Para 2.".into()],
            paragraph_spans: Vec::new(),
            sections: None,
            summary: None,
            summary_quote: None,
            summary_body: None,
            summary_lang: None,
            jurisdiction_code: None,
            jurisdiction_name: None,
            court_place: None,
            date_lecture: Some("2024-08-06".into()),
            solution: None,
            formation: Vec::new(),
            legal_domain: None,
            publication_codes: vec![],
            date_audience: None,
            docket_numbers: None,
            seat: None,
            seat_parts: None,
            seat_path: None,
            legal_references: None,
            source_xml: None,
            themes: Vec::new(),
            nac: None,
            ecli: None,
            source: None,
            official_url: None,
            chronology: Vec::new(),
            commentaires: vec![],
            ruling_url: None,
            citantes: vec![],
            citantes_total: None,
            lang: None,
            texts: Vec::new(),
        };
        let out = present_decision_detail(&detail, "https://librejustice.fr", &refs());
        assert_eq!(out.text, "Para 1.\n\nPara 2.");
        assert_eq!(out.url, "https://librejustice.fr/decision/ce-1");
        assert_eq!(out.title, "Conseil d'État, 6 août 2024");
    }

    #[test]
    fn detail_renders_case_chronology_with_solution_and_urls() {
        let detail = DecisionDetail {
            court_names: Default::default(),
            id: "tj-1".into(),
            jurisdiction_type: JurisdictionType::Tj,
            title: String::new(),
            case_name: None,
            paragraphs: vec!["Motifs.".into()],
            paragraph_spans: Vec::new(),
            sections: None,
            summary: None,
            summary_quote: None,
            summary_body: None,
            summary_lang: None,
            jurisdiction_code: None,
            jurisdiction_name: None,
            court_place: None,
            date_lecture: Some("2025-01-23".into()),
            solution: None,
            formation: Vec::new(),
            legal_domain: None,
            publication_codes: vec![],
            date_audience: None,
            docket_numbers: None,
            seat: None,
            seat_parts: None,
            seat_path: None,
            legal_references: None,
            source_xml: None,
            themes: Vec::new(),
            nac: None,
            ecli: None,
            source: None,
            official_url: None,
            chronology: vec![
                lj_dtos::ChronologyEntry {
                    id: Some("ca-9".into()),
                    jurisdiction_type: Some(lj_dtos::JurisdictionType::Ca),
                    jurisdiction_code: None,
                    label: "Cour d'appel de Paris".into(),
                    date: Some("2026-06-23".into()),
                    current: false,
                    solution: Some("INFIRMATION".into()),
                    docket_numbers: Some(vec!["25/03815".into()]),
                    link: Some("APPEL_DE".into()),
                    external_url: None,
                },
                lj_dtos::ChronologyEntry {
                    id: Some("tj-1".into()),
                    jurisdiction_type: Some(lj_dtos::JurisdictionType::Tj),
                    jurisdiction_code: None,
                    label: "Tribunal judiciaire de Paris".into(),
                    date: Some("2025-01-23".into()),
                    current: true,
                    solution: Some("SATISFACTION_TOTALE".into()),
                    docket_numbers: Some(vec!["21/16038".into()]),
                    link: None,
                    external_url: None,
                },
            ],
            commentaires: vec![],
            ruling_url: None,
            citantes: vec![],
            citantes_total: None,
            lang: None,
            texts: Vec::new(),
        };
        let out = present_decision_detail(&detail, "https://librejustice.fr", &refs());
        assert_eq!(out.case_chronology.len(), 2);
        let ca = &out.case_chronology[0];
        assert_eq!(ca.title, "Cour d'appel de Paris, 23 juin 2026, 25/03815");
        assert_eq!(
            ca.url.as_deref(),
            Some("https://librejustice.fr/decision/ca-9")
        );
        assert_eq!(ca.solution.as_deref(), Some("GRANTED"));
        assert_eq!(ca.ruling.as_deref(), Some("INFIRMATION"));
        assert_eq!(ca.link_to_next.as_deref(), Some("APPEL_DE"));
        assert!(!ca.current);
        assert!(out.case_chronology[1].current);
    }

    #[test]
    fn detail_renders_resolved_citations_as_markdown_links() {
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
            court_names: Default::default(),
            id: "ce-1".into(),
            jurisdiction_type: JurisdictionType::Ce,
            title: String::new(),
            case_name: None,
            paragraphs: vec![para.into(), "Rejette la requête.".into()],
            paragraph_spans: vec![spans, vec![]],
            sections: None,
            summary: None,
            summary_quote: None,
            summary_body: None,
            summary_lang: None,
            jurisdiction_code: None,
            jurisdiction_name: None,
            court_place: None,
            date_lecture: None,
            solution: None,
            formation: Vec::new(),
            legal_domain: None,
            publication_codes: vec![],
            date_audience: None,
            docket_numbers: None,
            seat: None,
            seat_parts: None,
            seat_path: None,
            legal_references: None,
            source_xml: None,
            themes: Vec::new(),
            nac: None,
            ecli: None,
            source: None,
            official_url: None,
            chronology: Vec::new(),
            commentaires: vec![],
            ruling_url: None,
            citantes: vec![],
            citantes_total: None,
            lang: None,
            texts: Vec::new(),
        };

        let resolved = detail(vec![span(vec![target(Some(
            "/norm/fr/code-de-procedure-civile/700",
        ))])]);
        let out = present_decision_detail(&resolved, "https://librejustice.fr", &refs());
        assert_eq!(
            out.text,
            "Vu l'énoncé de l'[article 700 du CPC]\
             (https://librejustice.fr/norm/fr/code-de-procedure-civile/700).\
             \n\nRejette la requête."
        );

        let multi = detail(vec![span(vec![
            target(None),
            target(Some("/norm/fr/code-de-procedure-civile/700")),
        ])]);
        let out = present_decision_detail(&multi, "https://librejustice.fr", &refs());
        assert!(out
            .text
            .contains("](https://librejustice.fr/norm/fr/code-de-procedure-civile/700)"));
        assert!(!out.text.contains("(+ "));

        let range = detail(vec![span(vec![
            target(Some("/norm/fr/code-de-procedure-civile/700")),
            CitationTarget {
                href: Some("/norm/fr/code-de-procedure-civile/701".into()),
                label: "701 — Code de procédure civile".into(),
            },
        ])]);
        let out = present_decision_detail(&range, "https://librejustice.fr", &refs());
        assert!(out.text.contains(
            "](https://librejustice.fr/norm/fr/code-de-procedure-civile/700) \
             (+ [701 — Code de procédure civile]\
             (https://librejustice.fr/norm/fr/code-de-procedure-civile/701))"
        ));

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
            jurisdiction: "FR".into(),
            jurisdiction_label: "France".into(),
            code_title: Some("Code civil".into()),
            num: "1240".into(),
            num_key: "1240".into(),
            etat: "VIGUEUR".into(),
            date_debut: "2016-10-01".into(),
            source: "legifrance".into(),
            titre_text: None,
            date_fin: None,
            expired: false,
            texte: Some("Sous réserve de l'article 1241, tout fait…".into()),
            texte_spans: vec![CitationSpan {
                start: 18,
                end: 30,
                targets: vec![CitationTarget {
                    href: Some("/norm/fr/code-civil/1241?date=2018-01-01".into()),
                    label: "Article 1241 du Code civil".into(),
                }],
            }],
            texte_original: None,
            lang_original: None,
            translation: "officiel".into(),
            source_asof: Some("2026-06-01".into()),
            source_authority: "source gouvernementale".into(),
            source_official: true,
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
            cited_by: vec![],
            commentaires: vec![],
            travaux_parlementaires: vec![],
            applicability: None,
        };
        let out = present_law_article(&article, "https://librejustice.fr");
        assert_eq!(out.title, "Article 1240 du Code civil");
        assert_eq!(out.url, "https://librejustice.fr/norm/fr/code-civil/1240");
        assert_eq!(
            out.text.as_deref(),
            Some(
                "Sous réserve de l'[article 1241]\
                 (https://librejustice.fr/norm/fr/code-civil/1241?date=2018-01-01), \
                 tout fait…"
            )
        );
        assert_eq!(out.versions.len(), 1);
        assert_eq!(out.versions[0].status, "VIGUEUR");
        conforms(
            &lj_mcp_tools::get_norm_output_schema(),
            &serde_json::to_value(&out).unwrap(),
            "get_norm",
        )
        .unwrap();
    }

    #[test]
    fn law_search_builds_citable_hits() {
        let response = ArticleSearchResponse {
            hits: vec![ArticleSearchHit {
                code: "code-de-procedure-civile".into(),
                jurisdiction: "FR".into(),
                code_title: "Code de procédure civile".into(),
                num: "700".into(),
                num_key: "700".into(),
                titre_path: Some("Livre Ier > Titre XVIII".into()),
                snippet: "les <mark>dépens</mark>".into(),
                source: "legifrance".into(),
                status: Some("VIGUEUR".into()),
                date_debut: None,
                valid_from: Some("2020-01-01".into()),
                valid_to: None,
            }],
            served: 1,
            total: Some(1),
            facets: Some(ArticleSearchFacets {
                scope: Vec::new(),
                code: vec![lj_dtos::FacetChoice {
                    value: "code-de-procedure-civile".into(),
                    label: "code-de-procedure-civile".into(),
                    count: 1,
                    parent: None,
                    hint: None,
                }],
                jurisdiction: vec![],
                source: vec![],
                path: vec![],
            }),
            query_mode: lj_dtos::QueryMode::Lexical,
            pinned: Vec::new(),
            pinned_more: false,
            jurisdictions: Vec::new(),
            reader_place: None,
        };
        let out = present_law_search("dépens", &response, "https://librejustice.fr");
        assert_eq!(out.query, "dépens");
        assert_eq!(out.total, 1);
        let hit = &out.hits[0];
        assert_eq!(hit.title, "Article 700 du Code de procédure civile");
        assert_eq!(
            hit.url,
            "https://librejustice.fr/norm/fr/code-de-procedure-civile/700"
        );
        assert_eq!(out.facets.code.get("code-de-procedure-civile"), Some(&1));
        assert!(out.facets.jurisdiction.is_empty());
        conforms(
            &lj_mcp_tools::search_norms_output_schema(),
            &serde_json::to_value(&out).unwrap(),
            "search_norms",
        )
        .unwrap();
    }

    #[test]
    fn law_listing_serves_text_roots_dated_and_typed_facets() {
        let choice = |value: &str, count| lj_dtos::FacetChoice {
            value: value.into(),
            label: value.into(),
            count,
            parent: None,
            hint: None,
        };
        let response = ArticleSearchResponse {
            hits: vec![ArticleSearchHit {
                code: "loi-n-2026-813-du-24-aout-2026".into(),
                jurisdiction: "FR".into(),
                code_title: "LOI n° 2026-813 du 24 août 2026".into(),
                num: String::new(),
                num_key: String::new(),
                titre_path: None,
                snippet: String::new(),
                source: "loi".into(),
                status: None,
                date_debut: None,
                valid_from: Some("2026-08-26".into()),
                valid_to: None,
            }],
            served: 59,
            total: Some(59),
            facets: Some(ArticleSearchFacets {
                scope: Vec::new(),
                code: Vec::new(),
                jurisdiction: vec![choice("FR", 59)],
                source: vec![],
                path: vec![
                    choice("FR", 59),
                    choice("FR.LOI", 59),
                    choice("FR.LOI.ORGANIQUE", 3),
                ],
            }),
            query_mode: lj_dtos::QueryMode::Browse,
            pinned: Vec::new(),
            pinned_more: false,
            jurisdictions: vec!["FR.LOI".into()],
            reader_place: None,
        };
        let out = present_law_search("", &response, "https://librejustice.fr");
        let hit = &out.hits[0];
        assert_eq!(hit.title, "LOI n° 2026-813 du 24 août 2026");
        assert_eq!(
            hit.url,
            "https://librejustice.fr/norm/fr/loi-n-2026-813-du-24-aout-2026"
        );
        assert_eq!(hit.valid_from.as_deref(), Some("2026-08-26"));
        assert_eq!(
            out.facets.jurisdiction.keys().collect::<Vec<_>>(),
            ["FR", "FR.LOI", "FR.LOI.ORGANIQUE"]
        );
        conforms(
            &lj_mcp_tools::search_norms_output_schema(),
            &serde_json::to_value(&out).unwrap(),
            "search_norms",
        )
        .unwrap();
    }

    /// A strict MCP client, the official Python SDK among them, validates
    /// `structuredContent` against the declared `outputSchema`: an undeclared
    /// field or a wrong type gets the whole call rejected client-side, with no
    /// server-side trace.
    fn conforms(
        schema: &serde_json::Value,
        value: &serde_json::Value,
        path: &str,
    ) -> Result<(), String> {
        use serde_json::Value;
        if let Some(branches) = schema.get("anyOf").and_then(Value::as_array) {
            return if branches.iter().any(|b| conforms(b, value, path).is_ok()) {
                Ok(())
            } else {
                Err(format!("{path}: no anyOf branch accepts {value}"))
            };
        }
        let Some(declared) = schema.get("type").and_then(Value::as_str) else {
            return Ok(());
        };
        let got = match value {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(n) if n.is_f64() && !n.is_i64() && !n.is_u64() => "number",
            Value::Number(_) => "integer",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        if declared != got && !(declared == "number" && got == "integer") {
            return Err(format!("{path}: declared {declared}, served {got}"));
        }
        match value {
            Value::Object(map) => {
                let open = schema.get("additionalProperties") != Some(&serde_json::json!(false));
                for (key, v) in map {
                    match schema.get("properties").and_then(|p| p.get(key)) {
                        Some(s) => conforms(s, v, &format!("{path}.{key}"))?,
                        None if open => {}
                        None => return Err(format!("{path}.{key}: undeclared field")),
                    }
                }
            }
            Value::Array(items) => {
                if let Some(s) = schema.get("items") {
                    for (i, v) in items.iter().enumerate() {
                        conforms(s, v, &format!("{path}[{i}]"))?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The other direction of `conforms`: every declared property is served by
    /// the fixture. Without it a field added to the DTO and missing from the
    /// schema passes the test, never being serialised, and the strict client is
    /// the one who finds out.
    fn covers(
        schema: &serde_json::Value,
        value: &serde_json::Value,
        except_: &[&str],
        path: &str,
    ) -> Result<(), String> {
        use serde_json::Value;
        // `nullable()` wraps the real schema in an `anyOf` with `null`.
        let real = |s: &Value| -> Value {
            s.get("anyOf")
                .and_then(Value::as_array)
                .and_then(|b| {
                    b.iter()
                        .find(|x| x.get("type").and_then(Value::as_str) != Some("null"))
                })
                .cloned()
                .unwrap_or_else(|| s.clone())
        };
        let (Some(props), Some(map)) = (
            schema.get("properties").and_then(Value::as_object),
            value.as_object(),
        ) else {
            return Ok(());
        };
        for (key, sub) in props {
            let here = format!("{path}.{key}");
            let Some(v) = map.get(key) else {
                if except_.contains(&here.as_str()) {
                    continue;
                }
                return Err(format!("{here}: declared, never served by the fixture"));
            };
            let sub = real(sub);
            match v {
                Value::Object(_) => covers(&sub, v, except_, &here)?,
                Value::Array(items) => {
                    if let (Some(first), Some(items_schema)) = (items.first(), sub.get("items")) {
                        covers(items_schema, first, except_, &format!("{here}[0]"))?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    #[test]
    fn search_response_conforms_to_its_output_schema() {
        let mut hit = base_hit();
        hit.seat_path = Some(tag("CA.PARIS.C3", "Troisième chambre"));
        hit.formation = vec![
            tag("FR.TJ.JUGE_UNIQUE", "Juge unique"),
            tag("FR.TJ.JEX", "Juge de l'exécution"),
        ];
        hit.legal_domain = Some(tag("BAIL", "Baux"));
        let resp = SearchResponse {
            query: "18-12.345".into(),
            total: Some(1),
            hits: vec![hit],
            query_mode: QueryMode::Lexical,
            facets: None,
            all_hit_paths: vec![],
            reranked: false,
            pinned: vec![PinnedDecision {
                id: "cc-1".into(),
                jurisdiction_type: lj_dtos::JurisdictionType::Cc,
                jurisdiction_code: Some("cc".into()),
                title: "Cour de cassation, 12 mars 2020, 18-12.345".into(),
                seat: None,
                seat_parts: None,
                solution: None,
                formation: Vec::new(),
            }],
            pinned_more: false,
        };
        let out = present_search_response(&resp, "https://librejustice.fr", &[]);
        let served = serde_json::to_value(&out).unwrap();
        assert!(!out.hits[0].publication.is_empty());
        conforms(&lj_mcp_tools::search_output_schema(), &served, "search").unwrap();
        // ai_summary, court_summary and snippet exclude each other, a lexical hit
        // carrying the snippet; facets declares no property, so there is nothing to cover.
        covers(
            &lj_mcp_tools::search_output_schema(),
            &served,
            &[
                "search.hits[0].ai_summary",
                "search.hits[0].court_summary",
                "search.hits[0].court_summary_kind",
                "search.facets",
            ],
            "search",
        )
        .unwrap();
    }

    #[test]
    fn decision_detail_conforms_to_its_output_schema() {
        let detail = DecisionDetail {
            court_names: Default::default(),
            id: "cc-1".into(),
            jurisdiction_type: JurisdictionType::Cc,
            title: String::new(),
            case_name: None,
            paragraphs: vec!["Motifs.".into()],
            paragraph_spans: Vec::new(),
            sections: None,
            summary: None,
            summary_quote: None,
            summary_body: None,
            summary_lang: None,
            jurisdiction_code: Some("cc".into()),
            jurisdiction_name: Some("Cour de cassation".into()),
            court_place: None,
            date_lecture: Some("2026-05-29".into()),
            solution: Some(tag("REJET", "Rejet")),
            formation: vec![tag("FR.CC.RESTREINTE", "Formation restreinte")],
            legal_domain: Some(tag("BAIL", "Baux")),
            publication_codes: vec!["b".into()],
            date_audience: None,
            docket_numbers: Some(vec!["24-17.384".into()]),
            seat: None,
            seat_parts: None,
            seat_path: Some(tag("CA.PARIS.C3", "Troisième chambre")),
            legal_references: None,
            source_xml: None,
            themes: Vec::new(),
            nac: None,
            ecli: None,
            source: None,
            official_url: Some("https://www.courdecassation.fr/decision/abc".into()),
            chronology: vec![
                lj_dtos::ChronologyEntry {
                    id: Some("cc-2".into()),
                    jurisdiction_type: Some(lj_dtos::JurisdictionType::Cc),
                    jurisdiction_code: None,
                    label: "Cour de cassation".into(),
                    date: Some("2026-05-29".into()),
                    current: false,
                    solution: Some("CASSATION".into()),
                    docket_numbers: Some(vec!["24-17.384".into()]),
                    link: Some("POURVOI_CONTRE".into()),
                    external_url: None,
                },
                lj_dtos::ChronologyEntry {
                    id: Some("cc-1".into()),
                    jurisdiction_type: Some(lj_dtos::JurisdictionType::Ca),
                    jurisdiction_code: None,
                    label: "Cour d'appel de Paris".into(),
                    date: Some("2024-03-14".into()),
                    current: true,
                    solution: Some("CONFIRMATION".into()),
                    docket_numbers: Some(vec!["22-04.118".into()]),
                    link: None,
                    external_url: None,
                },
            ],
            commentaires: vec![
                lj_dtos::Commentaire {
                    kind: "summary".into(),
                    author: None,
                    date: None,
                    title: None,
                    publisher: None,
                    volume: None,
                    issue: None,
                    pages: None,
                    url: None,
                    body: Some("Bail commercial - Congé - Validité.".into()),
                },
                lj_dtos::Commentaire {
                    kind: "note".into(),
                    author: Some("Cour de cassation".into()),
                    date: Some("2026-05-29".into()),
                    title: Some("Note explicative".into()),
                    publisher: Some("Cour de cassation".into()),
                    volume: Some("134".into()),
                    issue: Some("2".into()),
                    pages: Some("407-422".into()),
                    url: Some("https://www.courdecassation.fr/note/abc".into()),
                    body: None,
                },
            ],
            ruling_url: None,
            citantes: Vec::new(),
            citantes_total: None,
            lang: None,
            texts: Vec::new(),
        };
        let out = present_decision_detail(&detail, "https://librejustice.fr", &refs());
        let served = serde_json::to_value(&out).unwrap();
        assert!(!out.publication.is_empty());
        assert_eq!(out.court_summary_kind, Some("Sommaire"));
        assert!(out.commentaries.iter().all(|c| c.kind != "summary"));
        conforms(
            &lj_mcp_tools::get_decision_output_schema(),
            &served,
            "decision",
        )
        .unwrap();
        covers(
            &lj_mcp_tools::get_decision_output_schema(),
            &served,
            &[],
            "decision",
        )
        .unwrap();
    }

    fn code_summary(article_count: i64) -> lj_dtos::LawCodeSummary {
        lj_dtos::LawCodeSummary {
            legitext: "LEGITEXT000006070721".into(),
            code: "code-civil".into(),
            jurisdiction: "FR".into(),
            titre: "Code civil".into(),
            nature: "CODE".into(),
            derniere_modification: None,
            article_count,
            upcoming_versions: Vec::new(),
            body: None,
            status: Some("VIGUEUR".into()),
            nor: None,
            date_texte: Some("1804-03-21".into()),
            scope: None,
            fond: None,
            fond_label: None,
            fond_year: None,
            top_cited: Vec::new(),
            commentaires: Vec::new(),
            publication: None,
            notice: None,
        }
    }

    fn lecture(label: &str, num_key: Option<&str>) -> lj_dtos::LawSectionItem {
        lj_dtos::LawSectionItem {
            kind: if num_key.is_some() {
                "article"
            } else {
                "section"
            }
            .into(),
            depth: 1,
            label: label.into(),
            path: None,
            num_key: num_key.map(str::to_string),
            etat: "VIGUEUR".into(),
            texte: num_key.map(|k| format!("Corps de {k}.")),
            texte_spans: Vec::new(),
            nota: None,
        }
    }

    #[test]
    fn text_root_reads_the_reading_view_then_the_tree_then_the_flat_list() {
        let base = "https://librejustice.fr";
        let vide = || lj_dtos::CodeTocResponse {
            entries: Vec::new(),
            tree: Vec::new(),
            reading: Vec::new(),
        };

        let mut toc = vide();
        toc.reading = vec![
            lecture("Titre Ier", None),
            lecture("Article 1", Some("1")),
            lecture("Article 2", Some("2")),
        ];
        let out = present_law_text_root(&code_summary(2), &toc, base);
        assert_eq!(out.number, None);
        assert_eq!(out.article_count, Some(2));
        assert_eq!(out.omitted, None);
        assert_eq!(out.articles.len(), 2);
        assert_eq!(out.articles[0].url, format!("{base}/norm/fr/code-civil/1"));
        assert_eq!(out.articles[0].section.as_deref(), Some("Titre Ier"));
        assert_eq!(out.articles[0].text.as_deref(), Some("Corps de 1."));

        let mut toc = vide();
        toc.tree = vec![
            lj_dtos::TocNode {
                kind: "section".into(),
                depth: 1,
                label: "Livre Ier".into(),
                path: Some("livre-i".into()),
                num_key: None,
                etat: "VIGUEUR".into(),
            },
            lj_dtos::TocNode {
                kind: "article".into(),
                depth: 2,
                label: "Article 7".into(),
                path: None,
                num_key: Some("7".into()),
                etat: "VIGUEUR".into(),
            },
        ];
        // The section enters the toc with its address: it is what one call
        // reads, article by article being the fallback.
        let out = present_law_text_root(&code_summary(1), &toc, base);
        assert_eq!(out.articles.len(), 2);
        assert_eq!(out.articles[0].kind, "section");
        assert_eq!(
            out.articles[0].url,
            format!("{base}/norm/fr/code-civil/livre-i")
        );
        assert_eq!(out.articles[1].kind, "article");
        assert_eq!(out.articles[1].section.as_deref(), Some("Livre Ier"));
        assert_eq!(out.articles[1].text, None);

        let mut toc = vide();
        toc.entries = vec![lj_dtos::TocEntry {
            num: "L761-1".into(),
            num_key: "l761-1".into(),
            title_path: Some("Livre VII > Titre VI".into()),
            status: "VIGUEUR".into(),
            expired: false,
        }];
        let out = present_law_text_root(&code_summary(1), &toc, base);
        assert_eq!(
            out.articles[0].url,
            format!("{base}/norm/fr/code-civil/l761-1")
        );
        assert_eq!(
            out.articles[0].section.as_deref(),
            Some("Livre VII > Titre VI")
        );
    }

    #[test]
    fn a_toc_too_long_is_cut_and_says_so() {
        let toc = lj_dtos::CodeTocResponse {
            entries: (0..lj_dtos::INLINE_READING_MAX + 12)
                .map(|i| lj_dtos::TocEntry {
                    num: format!("{i}"),
                    num_key: format!("{i}"),
                    title_path: None,
                    status: "VIGUEUR".into(),
                    expired: false,
                })
                .collect(),
            tree: Vec::new(),
            reading: Vec::new(),
        };
        let out = present_law_text_root(&code_summary(162), &toc, "https://librejustice.fr");
        assert_eq!(out.articles.len(), lj_dtos::INLINE_READING_MAX);
        assert_eq!(out.omitted, Some(12));
    }

    /// A section serves its articles in one call, and says when it had to
    /// reduce them to their index.
    #[test]
    fn a_section_serves_its_articles_and_declares_what_it_bounded() {
        let base = "https://librejustice.fr";
        let section = |omitted: Option<usize>, texte: Option<&str>| lj_dtos::LawSectionResponse {
            code: "code-du-travail".into(),
            jurisdiction: "FR".into(),
            jurisdiction_label: "France".into(),
            code_title: Some("Code du travail".into()),
            path: "partie-legislative/livre-i".into(),
            title: "Sous-section 2 : Dérogations au repos dominical".into(),
            ancestors: vec![lj_dtos::LawSectionRef {
                path: "partie-legislative".into(),
                label: "Section 1 : Repos hebdomadaire".into(),
            }],
            prev: None,
            next: None,
            items: vec![lecture("Article L3132-20", Some("l3132-20"))]
                .into_iter()
                .map(|mut i| {
                    i.texte = texte.map(str::to_string);
                    i
                })
                .collect(),
            omitted,
            top_cited: Vec::new(),
            citing_texts: Vec::new(),
        };

        let out = present_law_section(&section(None, Some("Le repos peut être suspendu.")), base);
        assert_eq!(out.number, None);
        assert_eq!(out.omitted, None);
        assert_eq!(
            out.url,
            format!("{base}/norm/fr/code-du-travail/partie-legislative/livre-i")
        );
        assert_eq!(out.articles.len(), 1);
        assert_eq!(out.articles[0].kind, "article");
        assert_eq!(
            out.articles[0].url,
            format!("{base}/norm/fr/code-du-travail/l3132-20")
        );
        assert_eq!(
            out.articles[0].text.as_deref(),
            Some("Le repos peut être suspendu.")
        );
        assert_eq!(
            out.articles[0].section.as_deref(),
            Some("Section 1 : Repos hebdomadaire")
        );

        // Bounded: the bodies are gone, the link stays, and `omitted` says so.
        let bounded = present_law_section(&section(Some(82), None), base);
        assert_eq!(bounded.omitted, Some(82));
        assert_eq!(bounded.articles[0].text, None);
        assert!(bounded.articles[0].url.ends_with("/l3132-20"));

        let served = serde_json::to_value(&bounded).unwrap();
        conforms(&lj_mcp_tools::get_norm_output_schema(), &served, "section").unwrap();
    }

    /// A root serves no article field: `covers` only holds on the toc entry,
    /// where the four fields are declared.
    #[test]
    fn a_text_root_fits_the_get_norm_schema() {
        let toc = lj_dtos::CodeTocResponse {
            entries: Vec::new(),
            tree: Vec::new(),
            reading: vec![lecture("Titre Ier", None), lecture("Article 1", Some("1"))],
        };
        let out = present_law_text_root(&code_summary(1), &toc, "https://librejustice.fr");
        let served = serde_json::to_value(&out).unwrap();
        let schema = lj_mcp_tools::get_norm_output_schema();
        conforms(&schema, &served, "texte").unwrap();
        covers(
            &schema["properties"]["articles"]["items"],
            &served["articles"][0],
            &[],
            "texte.articles[0]",
        )
        .unwrap();
    }

    fn journal_hit(fond: &str, uid: &str) -> lj_dtos::JournalHit {
        serde_json::from_value(serde_json::json!({
            "uid": uid,
            "fond": fond,
            "fondLabel": "Marchés publics",
            "titre": "Réfection de la toiture de l'école",
            "description": "Travaux de couverture",
            "acteur": "Commune de Lyon",
            "acteurRole": "Acheteur",
            "jurisdiction": "FR",
            "journal": "Bulletin officiel du ministère de l'Intérieur",
            "zones": [{"key": "69", "label": "Rhône"}],
            "famille": {"key": "TRAVAUX", "label": "Travaux"},
            "cpv": {"key": "45", "label": "Travaux de construction"},
            "datePublication": "2026-11-18T08:00:00Z",
            "echeance": "2026-12-15",
            "urlAction": "https://marches.example.fr/consultation/123",
            "reference": "2026-045",
            "procedure": "Procédure adaptée",
            "canaux": ["boamp"],
        }))
        .unwrap()
    }

    #[test]
    fn a_notice_search_conforms_and_names_a_platform_for_tenders_alone() {
        let group = |fond: &str, uid: &str| lj_dtos::JournalFondGroup {
            fond: fond.into(),
            label: fond.into(),
            total: 1,
            hits: vec![journal_hit(fond, uid)],
        };
        let resp = lj_dtos::JournalSearchResponse {
            total: 2,
            groupes: vec![
                group("marche", "marche:boamp-26-123"),
                group("bodacc", "bodacc:A20260001"),
            ],
            sort: lj_dtos::JournalSort::Echeance,
            axes: Vec::new(),
        };
        let out = present_journal_search(&resp, "https://librejustice.fr");
        assert_eq!(
            out.hits[0].platform_url.as_deref(),
            Some("https://marches.example.fr/consultation/123")
        );
        assert_eq!(out.hits[1].platform_url, None);
        assert_eq!(
            out.hits[1].url,
            "https://librejustice.fr/notice/fr/bodacc:A20260001"
        );
        let served = serde_json::to_value(&out).unwrap();
        let schema = lj_mcp_tools::search_notices_output_schema();
        conforms(&schema, &served, "notices").unwrap();
        covers(&schema, &served, &[], "notices").unwrap();
    }

    /// A publication and a tender fill disjoint halves of `McpNotice`: each
    /// conforms, and together they serve every declared field.
    #[test]
    fn a_publication_and_a_tender_fit_the_get_notice_schema() {
        let notice: lj_dtos::PublicationNotice = serde_json::from_value(serde_json::json!({
            "uid": "jo-dj:DJ/ACCORD-0",
            "jurisdiction": "DJ",
            "journal": "JO-DJ",
            "journalLabel": "Journal officiel de Djibouti",
            "language": "fr",
            "title": "Décret portant ratification d'un accord",
            "nature": "Décret",
            "nor": "JUSC2600001D",
            "dateTexte": "2026-11-01",
            "datePubli": "2026-11-03",
            "issuer": "Présidence",
            "sourceUrl": "https://www.presidence.dj/texte.php?ID=1",
            "publication": {
                "issue": {"label": "JO n° 21 du 3 novembre 2026"},
                "notice": "Publics concernés : tous.",
                "visas": "Le Président,",
                "signatories": "Le Président",
                "summary": "Ratification.",
                "repeal": "Abroge le décret de 2001.",
                "corrigendum": "Rectificatif au JO n° 22.",
            },
            "articles": [{"num": "1", "text": "L'accord est ratifié."}],
            "body": "Texte intégral.",
            "norm": {"label": "Décret 2026-1", "href": "/norm/dj/decret-2026-1"},
            "fields": [{"group": "Immatriculation", "label": "SIREN", "value": "123 456 789", "href": "/entreprise/123456789"}],
            "entity": {"label": "ACME SAS", "href": "https://annuaire-entreprises.data.gouv.fr/entreprise/123456789"},
        }))
        .unwrap();
        let detail: lj_dtos::AnnonceDetailResponse = serde_json::from_value(serde_json::json!({
            "annonce": journal_hit("marche", "marche:boamp-26-123"),
            "lotsOuverts": [{"nom": "Lot 1", "description": "Couverture", "cpv": "45261000"}],
            "criteres": ["Prix 60 %"],
            "montantEstime": {"valeur": 120000.0, "devise": "EUR"},
        }))
        .unwrap();
        let base = "https://librejustice.fr";
        let publication = present_publication_notice(&notice, base);
        let tender = present_tender_notice(&detail, base);
        assert_eq!(
            publication.url,
            "https://librejustice.fr/notice/dj/jo-dj:DJ/ACCORD-0"
        );
        assert_eq!(
            publication.norm_url.as_deref(),
            Some("https://librejustice.fr/norm/dj/decret-2026-1")
        );
        assert_eq!(tender.published.as_deref(), Some("2026-11-18"));

        let schema = lj_mcp_tools::get_notice_output_schema();
        let publication = serde_json::to_value(&publication).unwrap();
        let tender = serde_json::to_value(&tender).unwrap();
        conforms(&schema, &publication, "publication").unwrap();
        conforms(&schema, &tender, "tender").unwrap();
        let mut both = publication.as_object().unwrap().clone();
        both.extend(tender.as_object().unwrap().clone());
        covers(&schema, &serde_json::Value::Object(both), &[], "notice").unwrap();
    }
}
