//! Recherche hybride (BM25 ParadeDB `@@@` + ANN `<=>`).
//!
//! 1. `embed_query` produit le vecteur requête (chemin hybride).
//! 2. En parallèle : BM25 body (`decisions_bm25` sur `full_text`, phrase-combo ou
//!    traduction booléenne), BM25 titre (`search_title`), et ANN VectorChord sur
//!    `decision_chunks` — la seule jambe encore au grain chunk (max-pool par
//!    décision).
//! 3. Fusion RRF au grain décision, pondération adaptative, cap et seuil relatif.
//! 4. Hydratation métadonnées + facettes + snippets.
//!
//! Le tri stable tie-break par `source_uid`/id vient de l'ordre déterministe des
//! `ORDER BY` SQL et du tri final sur le score puis l'id.

mod adaptive;
mod aggregate;
mod browse;
mod dates;
mod filters;
mod hydrate;
mod legs;
mod query;
pub mod suggest;

use std::collections::{HashMap, HashSet};

use deadpool_postgres::Client;
use lj_llm::backend::Embedder;
use lj_store::repository::DecisionRepository;

use crate::error::{ApiError, Result};
use crate::referential::referential;
use crate::rerank::{
    cached_order, rerank_shortlist, DocScores, RerankItem, RERANK_BUDGET, RERANK_BUDGET_MCP,
    RERANK_K,
};
use crate::signals;
use crate::snippets;
use crate::state::AppState;

use lj_dtos::{
    FacetChoice, LegalInstrumentFacet, PinnedDecision, QueryMode, SearchFacets, SearchHit,
    SearchMode, SearchRequest, SearchResponse, SortOrder,
};

use adaptive::compute_adaptive_weights;
use aggregate::{aggregate_facets, Aggs, Scope};
use hydrate::{
    assemble_page, display_seat, display_title, facets_from_counters, fetch_decision_paths,
    hydrate_decisions, opt_tag, DecisionMeta,
};
use legs::{
    ann_leg, ann_within, bm25_leg, bm25_parse_leg, fuse_ranks, pool_max_per_decision,
    reference_leg, title_leg, truncate_fused, LegHit, Legs, ReferenceKeys, Written,
};
use query::{
    detect_query_mode, has_boolean_operator, lexical_parse, query_lacks_searchable_terms,
    quoted_constraint, without_quotes,
};

pub(crate) use dates::{parse_search_date, DateError, DATE_GE, DATE_LE};
pub use query::{body_query_for_arm, BodyArm};

// ── API des bancs offline ────────────────────────────────────────────────────

/// Whether the offline benches run the title leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleLeg {
    On,
    Off,
}

/// Pure lexical retrieval for the offline ranking A/B: BM25 body leg (body built
/// per `arm`), title leg, RRF at base weights, relative truncation.
pub async fn lexical_rank_for_arm(
    conn: &Client,
    query: &str,
    arm: BodyArm,
    leg_limit: i64,
    title: TitleLeg,
) -> anyhow::Result<Vec<i64>> {
    let req: SearchRequest = serde_json::from_value(serde_json::json!({ "query": query }))?;
    let body_query = body_query_for_arm(query, arm);
    let bm25_scores = bm25_parse_leg(conn, &req, &body_query, leg_limit).await?;
    let title_scores = match title {
        TitleLeg::On => title_leg(conn, &req, leg_limit).await?,
        TitleLeg::Off => HashMap::new(),
    };
    let empty: HashMap<i64, LegHit> = HashMap::new();
    let fused = truncate_fused(fuse_ranks(
        Legs {
            bm25: &bm25_scores,
            ann: &empty,
            title: &title_scores,
        },
        signals::BASE_WEIGHTS,
    ));
    Ok(fused.into_iter().map(|(d, _, _)| d).collect())
}

/// Hybrid retrieval for the offline ranking A/B: query embedding, body / ANN /
/// title legs, adaptive RRF weights, relative truncation.
pub async fn hybrid_rank_for_arm(
    state: &AppState,
    query: &str,
    arm: BodyArm,
    title: TitleLeg,
) -> anyhow::Result<Vec<i64>> {
    let req: SearchRequest = serde_json::from_value(serde_json::json!({ "query": query }))?;
    let leg_limit = state.settings.leg_limit as i64;
    let vchord_probes = state.settings.vchord_probes;

    let query_vec = embed_query_cached(state, &req.query).await?;

    let body_query = body_query_for_arm(query, arm);
    let bm25_conn = client(state).await?;
    let mut ann_conn = client(state).await?;
    let title_conn = client(state).await?;
    let (bm25_scores, ann_hits, title_scores) = tokio::try_join!(
        bm25_parse_leg(&bm25_conn, &req, &body_query, leg_limit),
        ann_leg(&mut ann_conn, &req, &query_vec, leg_limit, vchord_probes),
        async {
            match title {
                TitleLeg::On => title_leg(&title_conn, &req, leg_limit).await,
                TitleLeg::Off => Ok(HashMap::new()),
            }
        },
    )?;
    let ann_best = pool_max_per_decision(&ann_hits, leg_limit as usize);
    let weights = compute_adaptive_weights(state, &req.query, &bm25_scores, &ann_hits).await?;
    let fused = truncate_fused(fuse_ranks(
        Legs {
            bm25: &bm25_scores,
            ann: &ann_best,
            title: &title_scores,
        },
        weights,
    ));
    Ok(fused.into_iter().map(|(d, _, _)| d).collect())
}

/// Ce qu'un balayage de plancher ANN observe ([`hybrid_rank_ann_floor_sweep`]).
pub struct AnnFloorSweep {
    /// Similarité cosinus du chunk gagnant, par décision atteinte par la jambe.
    pub ann_similarity: HashMap<i64, f64>,
    /// Similarité du dernier chunk rendu, avant plancher. Sous un plancher, la
    /// jambe a épuisé ses appariements et son décompte devient exact.
    pub ann_worst_similarity: Option<f64>,
    /// Classement fusionné, un par plancher, dans l'ordre des `floors` reçus.
    pub rankings: Vec<Vec<i64>>,
}

/// Balayage offline d'un plancher sur la jambe ANN : une seule récupération, une
/// fusion par plancher, poids adaptatifs recalculés sur la jambe filtrée.
/// `floors` en similarité cosinus minimale, `f64::NEG_INFINITY` valant prod.
pub async fn hybrid_rank_ann_floor_sweep(
    state: &AppState,
    query: &str,
    floors: &[f64],
    ann_limit: i64,
) -> anyhow::Result<AnnFloorSweep> {
    let req: SearchRequest = serde_json::from_value(serde_json::json!({ "query": query }))?;
    let leg_limit = state.settings.leg_limit as i64;

    let query_vec = embed_query_cached(state, &req.query).await?;
    let bm25_conn = client(state).await?;
    let mut ann_conn = client(state).await?;
    let title_conn = client(state).await?;
    let (bm25_scores, ann_hits, title_scores) = tokio::try_join!(
        bm25_leg(&bm25_conn, &req, leg_limit),
        ann_leg(
            &mut ann_conn,
            &req,
            &query_vec,
            ann_limit,
            state.settings.vchord_probes
        ),
        title_leg(&title_conn, &req, leg_limit),
    )?;

    let mut rankings = Vec::with_capacity(floors.len());
    for floor in floors {
        let kept: Vec<LegHit> = ann_hits
            .iter()
            .filter(|h| h.raw_score >= *floor)
            .cloned()
            .collect();
        let ann_best = pool_max_per_decision(&kept, ann_limit as usize);
        let weights = compute_adaptive_weights(state, &req.query, &bm25_scores, &kept).await?;
        let fused = truncate_fused(fuse_ranks(
            Legs {
                bm25: &bm25_scores,
                ann: &ann_best,
                title: &title_scores,
            },
            weights,
        ));
        rankings.push(fused.into_iter().map(|(d, _, _)| d).collect());
    }

    Ok(AnnFloorSweep {
        ann_similarity: pool_max_per_decision(&ann_hits, ann_limit as usize)
            .into_iter()
            .map(|(d, h)| (d, h.raw_score))
            .collect(),
        ann_worst_similarity: ann_hits
            .iter()
            .map(|h| h.raw_score)
            .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)),
        rankings,
    })
}

/// Recherche complète SANS le cache résultats moka (banc de charge, sonde
/// profonde) : exécute toujours récupération + RRF + hydratation + assemblage de
/// page, en conservant le permit d'admission et le cache embeddings. `ai_mode`
/// ignoré. Le décompte n'en fait pas partie, comme sur le chemin prod.
pub async fn search_nocache(state: &AppState, req: &SearchRequest) -> Result<SearchResponse> {
    nocache(state, req, false).await
}

/// La même chose, décompte résolu sur la MÊME récupération : ce qui permet de
/// comparer le total annoncé à la liste servie.
pub async fn search_nocache_counted(
    state: &AppState,
    req: &SearchRequest,
) -> Result<SearchResponse> {
    nocache(state, req, true).await
}

async fn nocache(
    state: &AppState,
    req: &SearchRequest,
    avec_decompte: bool,
) -> Result<SearchResponse> {
    let leg_limit = state.settings.leg_limit as i64;
    let vchord_probes = state.settings.vchord_probes;
    let t0 = std::time::Instant::now();
    let (ranked, retrieval_ms) = {
        let _db_permit = state
            .search_permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| ApiError::Internal(format!("search semaphore: {e}")))?;
        let resolution = resolve(state, req).await?;
        let (retrieval, mode) = retrieve(state, req, resolution, leg_limit, vchord_probes).await?;
        let retrieval_ms = t0.elapsed().as_secs_f64() * 1000.0;
        (
            std::sync::Arc::new(rank_results(state, retrieval, mode).await?),
            retrieval_ms,
        )
    };
    let hydrate_ms = t0.elapsed().as_secs_f64() * 1000.0 - retrieval_ms;
    if avec_decompte {
        comptage(state, req, &ranked).await?;
    }
    let t1 = std::time::Instant::now();
    let resp = paginate(state, req, &ranked, None, snippets::DEFAULT_MAX_CHARS).await?;
    tracing::info!(
        target: "lj_api::timing",
        retrieval_ms,
        hydrate_ms,
        paginate_ms = t1.elapsed().as_secs_f64() * 1000.0,
        "search_nocache phases"
    );
    Ok(resp)
}

/// Embedding requête via le cache in-process : un texte → un vecteur stable,
/// partagé entre les bras et les jambes d'une même requête.
async fn embed_query_cached(state: &AppState, text: &str) -> anyhow::Result<Vec<f32>> {
    let arr = match state.embedding_cache.get(text).await {
        Some(vec) => vec,
        None => {
            let owned = text.to_string();
            let arr = state
                .embedder
                .embed_query(std::slice::from_ref(&owned))
                .await
                .map_err(|e| anyhow::anyhow!("embed: {e}"))?;
            let vec = std::sync::Arc::new(arr.row(0).to_owned());
            state
                .embedding_cache
                .set(text, std::sync::Arc::clone(&vec))
                .await;
            vec
        }
    };
    Ok(arr.iter().copied().collect())
}

/// Latence isolée de la jambe BM25 body pour un bras — la seule jambe qui varie entre bras. Renvoie `(nb_hits, durée
/// d'UN appel)` ; l'appelant boucle pour la statistique.
pub async fn time_body_leg_for_arm(
    conn: &Client,
    query: &str,
    arm: BodyArm,
    leg_limit: i64,
) -> anyhow::Result<(usize, std::time::Duration)> {
    let req: SearchRequest = serde_json::from_value(serde_json::json!({ "query": query }))?;
    let body_query = body_query_for_arm(query, arm);
    let t = std::time::Instant::now();
    let hits = bm25_parse_leg(conn, &req, &body_query, leg_limit).await?;
    Ok((hits.len(), t.elapsed()))
}

/// Ranking de la seule jambe body BM25, sans jambe titre ni fusion RRF, pour un
/// bras de construction de requête. Renvoie `(decision_id, score)` trié score
/// décroissant puis id croissant.
pub async fn body_leg_ranks(
    conn: &Client,
    query: &str,
    arm: BodyArm,
    leg_limit: i64,
) -> anyhow::Result<Vec<(i64, f64)>> {
    let req: SearchRequest = serde_json::from_value(serde_json::json!({ "query": query }))?;
    let body_query = body_query_for_arm(query, arm);
    let scores = bm25_parse_leg(conn, &req, &body_query, leg_limit).await?;
    let mut ranked: Vec<(i64, f64)> = scores.into_iter().collect();
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    Ok(ranked)
}

/// Latence isolée de la jambe titre (banc `arm-latency`). Renvoie (nb_hits,
/// durée).
pub async fn time_title_leg(
    conn: &Client,
    query: &str,
    leg_limit: i64,
) -> anyhow::Result<(usize, std::time::Duration)> {
    let req: SearchRequest = serde_json::from_value(serde_json::json!({ "query": query }))?;
    let t = std::time::Instant::now();
    let scores = title_leg(conn, &req, leg_limit).await?;
    Ok((scores.len(), t.elapsed()))
}

/// Latence isolée de la jambe ANN (banc `arm-latency`), embedding hors chrono.
/// Renvoie (nb_hits, durée de la requête ANN seule).
pub async fn time_ann_leg(
    state: &AppState,
    conn: &mut Client,
    query: &str,
    leg_limit: i64,
) -> anyhow::Result<(usize, std::time::Duration)> {
    let req: SearchRequest = serde_json::from_value(serde_json::json!({ "query": query }))?;
    let query_vec = embed_query_cached(state, &req.query).await?;
    let probes = state.settings.vchord_probes;
    let t = std::time::Instant::now();
    let hits = ann_leg(conn, &req, &query_vec, leg_limit, probes).await?;
    Ok((hits.len(), t.elapsed()))
}

// ── Orchestration ──────────────────────────────────────────────────────────

/// Hydratation DB de tous les hits rankés (≤100) : metadata seule, aucun
/// highlight. C'est le Tier 1 du cache (cf. [`RankedResults`]) ; le highlight est
/// par-page ([`assemble_page`]).
async fn hydrate_all(
    state: &AppState,
    fused: &[(i64, f64, LegHit)],
) -> Result<HashMap<i64, DecisionMeta>> {
    let decision_ids: Vec<i64> = fused.iter().map(|(d, _, _)| *d).collect();
    let meta_conn = client(state).await?;
    hydrate_decisions(&meta_conn, &decision_ids)
        .await
        .map_err(ApiError::Store)
}

async fn client(state: &AppState) -> Result<Client> {
    state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))
}

/// `(decision_id, hit)` in the order given, score 0, the snippet highlighting
/// `query`; an id without displayable metadata is left out.
pub(crate) async fn hits_for_decision_ids(
    conn: &Client,
    decision_ids: &[i64],
    query: &str,
    refs: &crate::referential::Referential,
) -> Result<Vec<(i64, SearchHit)>> {
    let meta = hydrate_decisions(conn, decision_ids)
        .await
        .map_err(ApiError::Store)?;
    let page: Vec<(i64, f64, LegHit)> = decision_ids
        .iter()
        .filter(|id| meta.contains_key(id))
        .map(|id| (*id, 0.0, LegHit::synthetic_title_only(*id)))
        .collect();
    let hits = assemble_page(conn, &page, &meta, query, refs, snippets::DEFAULT_MAX_CHARS).await?;
    Ok(page.iter().map(|(id, _, _)| *id).zip(hits).collect())
}

/// Ordre relevance → ordre d'affichage : trie une vue de la liste fused par date
/// via la metadata hydratée, sans tie-break. `sort_by` stable + `order` déjà en
/// pertinence ⇒ égalité de date = ordre de pertinence préservé.
fn sort_fused(
    order: &mut [&(i64, f64, LegHit)],
    meta: &HashMap<i64, DecisionMeta>,
    sort: SortOrder,
) {
    let date_of = |d: i64| {
        meta.get(&d)
            .and_then(|m| m.date_lecture.clone())
            .unwrap_or_default()
    };
    match sort {
        SortOrder::Relevance => {}
        SortOrder::DateDesc => order.sort_by_key(|x| std::cmp::Reverse(date_of(x.0))),
        SortOrder::DateAsc => order.sort_by_key(|x| date_of(x.0)),
    }
}

/// Retrieval output: the fused list, and the card above it when the resolution
/// carries one.
struct Retrieval {
    fused: Vec<(i64, f64, LegHit)>,
    lexicales: std::collections::HashSet<i64>,
    pinned: Vec<i64>,
    /// The card holds a sample: the reference retrieval carries more than it shows.
    pinned_more: bool,
}

/// How many decisions the card above a content list holds.
const PINNED_MAX: usize = 3;

/// The filters the search no longer has. A client still sending them, from a
/// cached tool list or an old integration, is answered without them and told so.
pub(crate) const RETIRED_FILTERS: [&str; 2] = ["procedure", "office"];

/// ASCII, since the public door sends it as a header.
pub(crate) const RETIRED_FILTERS_WARNING: &str = "`procedure` and `office` no longer filter: \
the bench is `formation`, and an interim-relief track or a QPC is reached by `legal_article`.";

/// What `auto` decides for a query, or what a stated mode collapses to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolution {
    Reference,
    Lexical,
    Semantic,
    LexicalWithCard,
    SemanticWithCard,
}

impl Resolution {
    fn query_mode(self) -> QueryMode {
        match self {
            Resolution::Reference => QueryMode::Reference,
            Resolution::Lexical | Resolution::LexicalWithCard => QueryMode::Lexical,
            Resolution::Semantic | Resolution::SemanticWithCard => QueryMode::Hybrid,
        }
    }

    fn with_card(self) -> bool {
        matches!(
            self,
            Resolution::LexicalWithCard | Resolution::SemanticWithCard
        )
    }
}

/// What a content query designates besides what it asks: a number, weak when
/// its four digits beside a court could be a year, or the decisions a name
/// designates, with the words of the query that name covers.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Designation {
    Number {
        weak: bool,
    },
    Name {
        bearers: Vec<i64>,
        covered: Vec<String>,
    },
}

impl Designation {
    /// The designation is the whole query: nothing is left to embed.
    fn consumes(&self, query: &str) -> bool {
        match self {
            Designation::Number { .. } => lj_core::docket_key::is_bare_number(query),
            Designation::Name { covered, .. } => {
                let folded = lj_core::case_names::case_name_key(query);
                let words: Vec<&str> = folded.split(' ').collect();
                lj_core::citation::only_designates(&words_outside(&words, covered))
            }
        }
    }

    /// The rest of the query cites the designation, a court named: the query
    /// is a reference. A number takes every word carrying a digit with it; a
    /// weak number never declares.
    fn cited(&self, query: &str, seats: &std::collections::HashSet<String>) -> bool {
        let folded = lj_core::case_names::case_name_key(query);
        let words: Vec<&str> = folded.split(' ').collect();
        let around: Vec<&str> = match self {
            Designation::Number { weak: true } => return false,
            Designation::Number { weak: false } => words
                .into_iter()
                .filter(|w| !w.chars().any(|c| c.is_ascii_digit()))
                .collect(),
            Designation::Name { covered, .. } => words_outside(&words, covered),
        };
        lj_core::citation::remainder_cites(&around, seats)
    }
}

/// The words of a folded query once each covered word is taken out.
fn words_outside<'a>(words: &[&'a str], covered: &[String]) -> Vec<&'a str> {
    let mut left: Vec<&str> = covered.iter().map(String::as_str).collect();
    words
        .iter()
        .copied()
        .filter(|w| !w.is_empty())
        .filter(|w| match left.iter().position(|c| c == w) {
            Some(i) => {
                left.swap_remove(i);
                false
            }
            None => true,
        })
        .collect()
}

/// A stated mode is served as stated, without a card; `auto` settles the
/// declared reference on the query shape and leaves the rest to [`resolve_content`].
fn resolve_shape(req: &SearchRequest) -> Option<Resolution> {
    match req.mode {
        lj_dtos::SearchMode::Lexical => Some(Resolution::Lexical),
        lj_dtos::SearchMode::Semantic => Some(Resolution::Semantic),
        lj_dtos::SearchMode::Reference => Some(Resolution::Reference),
        lj_dtos::SearchMode::Auto => {
            ((lj_core::docket_key::declares_reference_explicitly(&req.query)
                || lj_core::docket_key::bare_number_declares(&req.query))
                && !has_boolean_operator(&req.query))
            .then_some(Resolution::Reference)
        }
    }
}

/// A reference settled by its shape holds Postgres like any search, for a tenth
/// of a hybrid's time.
pub(crate) fn budget_cost(req: &SearchRequest) -> crate::ratelimit::Cost {
    match resolve_shape(req) {
        Some(Resolution::Reference) => crate::ratelimit::Cost::Reference,
        _ => crate::ratelimit::Cost::Search,
    }
}

/// The content mode, and a card above it when the query designates a decision.
/// A designation cited with its court is the reference, as are a court and a
/// day alone; a designation that consumes the query leaves nothing for the
/// ANN: lexical.
fn resolve_content(
    query: &str,
    designation: Option<Designation>,
    seats: &std::collections::HashSet<String>,
) -> Resolution {
    let lexical = detect_query_mode(query) == QueryMode::Lexical;
    let boolean = has_boolean_operator(query);
    match designation {
        None if !boolean && lj_core::citation::cites_a_day(query, seats) => Resolution::Reference,
        None if lexical => Resolution::Lexical,
        None => Resolution::Semantic,
        Some(d) if !boolean && d.cited(query, seats) => Resolution::Reference,
        Some(d) if lexical || d.consumes(query) => Resolution::LexicalWithCard,
        Some(_) => Resolution::SemanticWithCard,
    }
}

async fn designation(state: &AppState, query: &str) -> Result<Option<Designation>> {
    let numbers = lj_core::docket_key::query_numbers(query);
    let eclis = lj_core::docket_key::query_eclis(query);
    if !numbers.is_empty() || !eclis.is_empty() {
        let weak = eclis.is_empty() && numbers.iter().all(|n| n.weak);
        return Ok(Some(Designation::Number { weak }));
    }
    let refs = crate::referential::referential(state).await?;
    let folded = lj_core::case_names::case_name_key(query);
    let words: Vec<&str> = folded.split(' ').filter(|w| !w.is_empty()).collect();
    let (distinctive, designates) = lj_core::citation::distinctive_words(&words, refs.seat_words());
    let seated = lj_core::citation::strip_court(&words)
        .is_some_and(|(kind, rest)| refs.seats_court(kind, &rest));
    if distinctive.is_empty() && seated {
        return Ok(None);
    }
    let bearers = match distinctive.is_empty() {
        true => Vec::new(),
        false => refs.name_bearers(&distinctive, lj_core::case_names::NAME_BEARERS_MAX),
    };
    if !bearers.is_empty() && (designates || !all_written_often(state, &distinctive).await?) {
        return Ok(Some(Designation::Name {
            bearers,
            covered: distinctive.iter().map(|w| w.to_string()).collect(),
        }));
    }
    let keys = lj_core::case_names::candidate_keys(query);
    if !refs.names_a_case(&keys) {
        return Ok(None);
    }
    let conn = client(state).await?;
    let Some(named) = DecisionRepository::new(&conn)
        .case_name_match(&keys)
        .await
        .map_err(ApiError::Store)?
    else {
        return Ok(None);
    };
    if lj_core::citation::key_is_glued(&words, &named.key, &distinctive) {
        return Ok(None);
    }
    Ok(Some(Designation::Name {
        bearers: named.decisions.iter().map(|d| d.id).collect(),
        covered: named.key.split(' ').map(str::to_string).collect(),
    }))
}

/// Without a court or a designating word, a name's words must not all be
/// common words of the fonds: « harcèlement » alone asks, it names nobody.
async fn all_written_often(state: &AppState, words: &[&str]) -> Result<bool> {
    let conn = client(state).await?;
    let frequent = DecisionRepository::new(&conn)
        .frequent_names(words, lj_core::case_names::SHORT_NAME_RARITY)
        .await
        .map_err(ApiError::Store)?;
    Ok(frequent.len() == words.len())
}

/// A date order on content lists every decision holding all the words, sorted in
/// the index: re-sorting the relevance pool, capped at 100, dropped the newest.
async fn dated_listing(state: &AppState, req: &SearchRequest) -> Result<bool> {
    if req.sort == SortOrder::Relevance || query_lacks_searchable_terms(&req.query) {
        return Ok(false);
    }
    Ok(resolve(state, req).await? != Resolution::Reference)
}

async fn resolve(state: &AppState, req: &SearchRequest) -> Result<Resolution> {
    if let Some(resolution) = resolve_shape(req) {
        return Ok(resolution);
    }
    let designation = designation(state, &req.query).await?;
    let refs = crate::referential::referential(state).await?;
    Ok(resolve_content(&req.query, designation, refs.seat_words()))
}

/// The legs a resolution runs, and the mode served. A card outcome runs the
/// reference retrieval after the content one, connections released in between,
/// and keeps its top [`PINNED_MAX`]; an empty content list is replaced by it.
async fn retrieve(
    state: &AppState,
    req: &SearchRequest,
    resolution: Resolution,
    leg_limit: i64,
    vchord_probes: u32,
) -> Result<(Retrieval, QueryMode)> {
    let mode = resolution.query_mode();
    if query_lacks_searchable_terms(&req.query) {
        let empty = Retrieval {
            fused: Vec::new(),
            lexicales: std::collections::HashSet::new(),
            pinned: Vec::new(),
            pinned_more: false,
        };
        return Ok((empty, mode));
    }
    let mut retrieval = match mode {
        QueryMode::Reference => retrieve_reference(state, req, leg_limit).await?,
        QueryMode::Hybrid => retrieve_hybrid(state, req, leg_limit, vchord_probes).await?,
        _ => retrieve_boolean(state, req, leg_limit).await?,
    };
    if resolution.with_card() {
        let card = retrieve_reference(state, req, leg_limit).await?;
        if retrieval.fused.is_empty() && !card.fused.is_empty() {
            return Ok((card, QueryMode::Reference));
        }
        retrieval.pinned_more = card.fused.len() > PINNED_MAX;
        retrieval.pinned = card
            .fused
            .iter()
            .take(PINNED_MAX)
            .map(|(d, _, _)| *d)
            .collect();
    }
    Ok((retrieval, mode))
}

async fn reference_keys(state: &AppState, req: &SearchRequest) -> Result<ReferenceKeys> {
    let numbers = lj_core::docket_key::query_numbers(&req.query);
    let eclis = lj_core::docket_key::query_eclis(&req.query);
    if !numbers.is_empty() || !eclis.is_empty() {
        return Ok(ReferenceKeys {
            numbers,
            eclis,
            bearers: Vec::new(),
        });
    }
    Ok(match designation(state, &req.query).await? {
        Some(Designation::Name { bearers, .. }) => ReferenceKeys {
            bearers,
            ..ReferenceKeys::default()
        },
        _ => ReferenceKeys::default(),
    })
}

/// The court and the day a query writes, read against the referential: the
/// court by its code when the seat resolves, by its type otherwise.
fn written_citation(refs: &crate::referential::Referential, query: &str) -> Written {
    let folded = lj_core::case_names::case_name_key(query);
    let words: Vec<&str> = folded.split(' ').collect();
    let (kind, code) = match lj_core::citation::strip_court(&words) {
        Some((kind, rest)) => (Some(kind), refs.court_code(kind, &rest)),
        None => (None, None),
    };
    Written {
        kind,
        code,
        day: lj_core::citation::query_day(query),
    }
}

/// The request the title leg of a reference runs: the court and the day the
/// query writes become its filters, unless the request carries its own.
fn title_request(refs: &crate::referential::Referential, req: &SearchRequest) -> SearchRequest {
    let written = written_citation(refs, &req.query);
    let mut title_req = req.clone();
    if req.date_from.is_none() && req.date_to.is_none() {
        if let Some(day) = written.day {
            title_req.date_from = Some(day.clone());
            title_req.date_to = Some(day);
        }
    }
    if req.jurisdiction_code.is_none() && req.jurisdiction_type.is_none() && req.seat.is_none() {
        match (written.code, written.kind) {
            (Some(code), _) => title_req.jurisdiction_code = Some(vec![code]),
            (None, Some(kind)) => {
                title_req.jurisdiction_type = serde_json::from_value(serde_json::json!([kind])).ok()
            }
            (None, None) => {}
        }
    }
    title_req
}

/// Retrieval for a query that designates a decision: the title, the canonical
/// keys and the ECLIs in one BM25 query, the only fields that index a decision's
/// identity. No embedding and no ANN, so an absent reference answers with an
/// empty list instead of its neighbours.
///
/// The court and the day the query writes filter the whole retrieval only when
/// nothing else designates: with a number in hand, they rank its bearers, and a
/// bearer of another court is what tells the caller the number is not that
/// court's.
#[tracing::instrument(skip(state, req), fields(leg_limit))]
async fn retrieve_reference(
    state: &AppState,
    req: &SearchRequest,
    leg_limit: i64,
) -> Result<Retrieval> {
    let reference = reference_keys(state, req).await?;
    let designated = reference.bearers.is_empty()
        && !(reference.numbers.is_empty() && reference.eclis.is_empty());
    let refs = crate::referential::referential(state).await?;
    let (leg_req, written) = match designated {
        true => (req.clone(), written_citation(&refs, &req.query)),
        false => (title_request(&refs, req), Written::default()),
    };
    let conn = client(state).await?;
    let hits = reference_leg(&conn, &leg_req, &reference, &written, leg_limit)
        .await
        .map_err(ApiError::Store)?;
    let fused: Vec<(i64, f64, LegHit)> = hits
        .into_iter()
        .map(|(d, s)| (d, s, LegHit::synthetic_title_only(d)))
        .collect();
    // Every bearer of a key or a name is an answer, not a tail: only the title alone has one to cut.
    let fused = match designated || !reference.bearers.is_empty() {
        true => fused,
        false => truncate_fused(fused),
    };
    Ok(Retrieval {
        fused,
        lexicales: std::collections::HashSet::new(),
        pinned: Vec::new(),
        pinned_more: false,
    })
}

#[tracing::instrument(skip(state, req), fields(leg_limit))]
async fn retrieve_boolean(
    state: &AppState,
    req: &SearchRequest,
    leg_limit: i64,
) -> Result<Retrieval> {
    let bm25_conn = client(state).await?;
    let bm25_scores = bm25_parse_leg(&bm25_conn, req, &lexical_parse(&req.query), leg_limit)
        .await
        .map_err(ApiError::Store)?;
    let empty: HashMap<i64, LegHit> = HashMap::new();
    let none: HashMap<i64, f64> = HashMap::new();
    let fused = truncate_fused(fuse_ranks(
        Legs {
            bm25: &bm25_scores,
            ann: &empty,
            title: &none,
        },
        signals::BASE_WEIGHTS,
    ));
    Ok(Retrieval {
        fused,
        lexicales: bm25_scores.keys().copied().collect(),
        pinned: Vec::new(),
        pinned_more: false,
    })
}

#[tracing::instrument(skip(state, req), fields(leg_limit, vchord_probes))]
async fn retrieve_hybrid(
    state: &AppState,
    req: &SearchRequest,
    leg_limit: i64,
    vchord_probes: u32,
) -> Result<Retrieval> {
    let constraint = quoted_constraint(&req.query);
    let question = match constraint {
        Some(_) => without_quotes(&req.query),
        None => req.query.clone(),
    };
    let query_arr = match state.embedding_cache.get(&question).await {
        Some(vec) => {
            crate::cache_metrics::record_access("embedding", true);
            vec
        }
        None => {
            crate::cache_metrics::record_access("embedding", false);
            let arr = state
                .embedder
                .embed_query(std::slice::from_ref(&question))
                .await
                .map_err(|e| ApiError::Internal(format!("embed: {e}")))?;
            let vec = std::sync::Arc::new(arr.row(0).to_owned());
            state
                .embedding_cache
                .set(&question, std::sync::Arc::clone(&vec))
                .await;
            vec
        }
    };
    let query_vec: Vec<f32> = query_arr.iter().copied().collect();

    // Les connexions des jambes sont relâchées dès le `try_join` fini, AVANT
    // `compute_adaptive_weights` qui en reprend 2 : le pic reste à
    // `PEAK_CONNS_PER_SEARCH`.
    let (bm25_scores, ann_hits) = {
        let bm25_conn = client(state).await?;
        let mut ann_conn = client(state).await?;
        tokio::try_join!(bm25_leg(&bm25_conn, req, leg_limit), async {
            match &constraint {
                Some(c) => {
                    ann_within(&mut ann_conn, req, c, &query_vec, leg_limit, vchord_probes).await
                }
                None => ann_leg(&mut ann_conn, req, &query_vec, leg_limit, vchord_probes).await,
            }
        })
        .map_err(ApiError::Store)?
    };

    let ann_best = pool_max_per_decision(&ann_hits, leg_limit as usize);
    let weights = compute_adaptive_weights(state, &req.query, &bm25_scores, &ann_hits).await?;
    let none: HashMap<i64, f64> = HashMap::new();
    let fused = truncate_fused(fuse_ranks(
        Legs {
            bm25: &bm25_scores,
            ann: &ann_best,
            title: &none,
        },
        weights,
    ));
    Ok(Retrieval {
        fused,
        lexicales: bm25_scores.keys().copied().collect(),
        pinned: Vec::new(),
        pinned_more: false,
    })
}

/// Clé du cache rerank : `(query, ensemble TRIÉ du top-[`RERANK_K`])`. Trié parce
/// que la sortie du rerank ne dépend que du set de documents et de la query — deux
/// récupérations au même top-50 partagent l'ordre LLM.
fn rerank_cache_key(query: &str, top_ids: &[i64]) -> String {
    let mut ids = top_ids.to_vec();
    ids.sort_unstable();
    let joined = ids
        .iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join(",");
    format!("{query}|{joined}")
}

fn score_key(query: &str, decision_id: i64) -> String {
    format!("{decision_id} {query}")
}

/// Applies the LLM order `new_ids` to the head (top-[`RERANK_K`]) of `base` and
/// returns every id in reranked relevance order: the reranked ids take back, in LLM
/// order, the ranks they held; a head id the reranker did not judge (no summary)
/// keeps its RRF rank; then the tail untouched.
fn rerank_id_order(base: &[(i64, f64, LegHit)], new_ids: &[i64]) -> Vec<i64> {
    let head_len = RERANK_K.min(base.len());
    let head = &base[..head_len];
    let mut in_head: HashSet<i64> = head.iter().map(|(d, _, _)| *d).collect();
    let reranked: Vec<i64> = new_ids
        .iter()
        .copied()
        .filter(|d| in_head.remove(d))
        .collect();
    let judged: HashSet<i64> = reranked.iter().copied().collect();
    let mut reranked = reranked.into_iter();
    let mut out: Vec<i64> = head
        .iter()
        .map(|(d, _, _)| {
            if judged.contains(d) {
                reranked.next().expect("one reranked id per judged slot")
            } else {
                *d
            }
        })
        .collect();
    out.extend(base[head_len..].iter().map(|(d, _, _)| *d));
    out
}

/// Reference ranks by identity, so an LLM asked to judge « 2503146 » against its
/// carriers can only demote the exact match. Lexical ranks by the operators the
/// caller typed, and browse asks nothing.
pub(crate) fn reranks(mode: QueryMode) -> bool {
    matches!(mode, QueryMode::Hybrid)
}

/// Ordre LLM du top-[`RERANK_K`] si le mode IA s'applique, sinon `None` (ordre RRF
/// naturel). No-op silencieux si : `ai_mode` désactivé, mode servi hors hybride
/// ([`reranks`]), sort ≠ relevance, clé Mistral absente, shortlist < 3, budget de
/// rerank épuisé, ou échec du rerank.
///
/// Deux caches en série : l'ordre complet, keyé `(query, top-ids)` ; puis les
/// notes, keyées `(query, document)`, indépendantes de la shortlist. Seuls les
/// documents sous [`TARGET_SCORES`] repartent au LLM.
///
/// Body-source = `bundle.meta`, donc zéro requête DB ici.
async fn reranked_order(
    state: &AppState,
    req: &SearchRequest,
    bundle: &RankedResults,
    budget_key: Option<&crate::ratelimit::CallerKey>,
    budget: std::time::Duration,
) -> Result<Option<std::sync::Arc<Vec<i64>>>> {
    if !req.ai_mode
        || !reranks(bundle.query_mode)
        || req.sort != SortOrder::Relevance
        || bundle.fused.len() < 3
    {
        return Ok(None);
    }

    let head_len = RERANK_K.min(bundle.fused.len());
    let top_ids: Vec<i64> = bundle.fused[..head_len]
        .iter()
        .map(|(d, _, _)| *d)
        .collect();

    let refs = referential(state).await?;
    let items: Vec<RerankItem> = top_ids
        .iter()
        .filter_map(|d| {
            let m = bundle.meta.get(d)?;
            let summary = m.summary.as_deref().filter(|s| !s.is_empty())?;
            Some(RerankItem {
                decision_id: *d,
                title: display_title(m, &refs),
                summary: lj_summary::summary_head(summary).to_string(),
            })
        })
        .collect();
    if items.len() < 3 {
        return Ok(None);
    }

    // Single-flight : `optionally_get_with` ne laisse qu'un appelant exécuter le
    // rerank pour une clé donnée. Le `None` — mode IA renoncé, échec, budget épuisé —
    // n'est pas mis en cache.
    let key = rerank_cache_key(&req.query, &top_ids);
    let order = state
        .rerank_cache
        .optionally_get_with(key, llm_order(state, req, items, budget_key, budget))
        .await;
    Ok(order)
}

/// Ordre LLM pour une shortlist qui n'est pas déjà dans le cache d'ordre : notes
/// en cache d'abord, appels Mistral pour le reste. `None` = renoncement, et
/// l'appelant sert l'ordre RRF.
async fn llm_order(
    state: &AppState,
    req: &SearchRequest,
    items: Vec<RerankItem>,
    budget_key: Option<&crate::ratelimit::CallerKey>,
    budget: std::time::Duration,
) -> Option<std::sync::Arc<Vec<i64>>> {
    let qkey: std::sync::Arc<str> = std::sync::Arc::from(req.query.as_str());
    let mut scores: HashMap<i64, DocScores> = HashMap::new();
    for it in &items {
        if let Some(v) = state
            .rerank_scores_cache
            .get(&score_key(&qkey, it.decision_id))
            .await
        {
            scores.insert(it.decision_id, v);
        }
    }
    if let Some(order) = cached_order(&items, &scores) {
        return Some(std::sync::Arc::new(order));
    }

    let client = {
        let conn = match state.pool.get().await {
            Ok(conn) => conn,
            Err(err) => {
                tracing::warn!(error = %err, "connection checkout for the Mistral pool, rerank dropped");
                return None;
            }
        };
        state
            .mistral
            .client(&DecisionRepository::new(&conn))
            .await?
    };

    // One token is one rerank, whatever number of calls the score cache leaves it
    // to make. The token is set on the worst case.
    if state
        .budgets
        .take(crate::ratelimit::Cost::Rerank, budget_key)
        .await
        == crate::ratelimit::Verdict::Exhausted
    {
        return None;
    }

    let sink: crate::rerank::ScoreSink = {
        let cache = state.rerank_scores_cache.clone();
        let qkey = std::sync::Arc::clone(&qkey);
        std::sync::Arc::new(move |batch: HashMap<i64, f64>| {
            let cache = cache.clone();
            let qkey = std::sync::Arc::clone(&qkey);
            tokio::spawn(async move {
                let now = tokio::time::Instant::now();
                for (decision_id, score) in batch {
                    cache
                        .upsert_with(score_key(&qkey, decision_id), |old| {
                            DocScores::merged(old, &[score], now)
                        })
                        .await;
                }
            });
        })
    };

    let result = rerank_shortlist(
        items,
        &req.query,
        std::sync::Arc::clone(&client),
        Some(tokio::time::Instant::now() + budget),
        &scores,
        sink,
    )
    .await;

    let spent = client.drain_spent_fingerprints();
    if !spent.is_empty() {
        match state.pool.get().await {
            Ok(conn) => {
                let repo = DecisionRepository::new(&conn);
                for (fp, status) in &spent {
                    let disable = repo.disable_credential(fp, None, *status as i16, "rerank");
                    if let Err(err) = disable.await {
                        tracing::warn!(fingerprint = %fp, error = %err, "disable_credential failed");
                    }
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "connection checkout for disable_credential")
            }
        }
    }

    match result {
        Ok(new_ids) => Some(std::sync::Arc::new(new_ids)),
        Err(err) => {
            tracing::warn!(error = %err, "rerank_shortlist failed — falling back to retrieval order");
            None
        }
    }
}

/// Bundle de recherche mis en cache : récupération (BM25/ANN, RRF), facettes et
/// hydratation complète des hits (metadata + summary), AVANT rerank, tri par date
/// et pagination. Partagé via `Arc` dans le cache moka de l'[`AppState`], clé
/// ai_mode-indépendante. Le `full_text` n'est pas caché : il est fetché par-page
/// par [`assemble_page`].
pub struct RankedResults {
    query_mode: QueryMode,
    /// Liste fused en ordre RRF (pré-rerank, pré-tri). Porte le `chunk_id` du chunk
    /// gagnant ANN et les scores.
    fused: Vec<(i64, f64, LegHit)>,
    /// Metadata hydratée de tous les hits (≤100), `summary` inclus. Sert au tri par
    /// date, à l'assemblage de page et de texte-source au rerank. Pas de highlight.
    meta: HashMap<i64, DecisionMeta>,
    /// `decision_id → public_id` de tous les hits.
    decision_paths: HashMap<i64, String>,
    /// Servies que la question n'apparie pas, portées ici pour que le décompte
    /// décrive la récupération qui a servi CETTE liste.
    hors_question: Vec<i64>,
    comptage: tokio::sync::OnceCell<(i64, SearchFacets)>,
    /// Decisions the query designates, hydrated once with the bundle.
    pinned: Vec<PinnedDecision>,
    /// The card holds a sample: the reference retrieval carries more than it shows.
    pinned_more: bool,
    /// Tier 2 — pages déjà highlightées, mémoïsées par `(ai_mode, sort, offset,
    /// limit)`. `ai_mode` est dans la clé car le rerank change l'ordre, donc le contenu
    /// de la page. `std::sync::Mutex` : verrou jamais tenu à travers un `await`.
    page_memo: std::sync::Mutex<HashMap<String, std::sync::Arc<Vec<SearchHit>>>>,
}

impl RankedResults {
    /// Poids approximatif de l'entrée en octets, pour le `weigher` du cache moka.
    /// Dominé par la metadata hydratée. Ni le `page_memo` ni le décompte ne sont
    /// pesés, tous deux remplis après insertion : moka ne ré-évalue pas le poids.
    pub fn approx_weight(&self) -> u32 {
        let fused: usize = self
            .fused
            .iter()
            .map(|(_, _, c)| {
                std::mem::size_of::<LegHit>() + c.snippet.as_ref().map_or(0, String::len)
            })
            .sum();
        let meta: usize = self.meta.values().map(meta_weight).sum();
        let ids: usize = self
            .decision_paths
            .values()
            .map(|s| std::mem::size_of::<i64>() + std::mem::size_of::<String>() + s.len())
            .sum();
        (fused + meta + ids).min(u32::MAX as usize) as u32
    }
}

/// Octets approximatifs d'une `DecisionMeta` (size_of inline + champs texte sur le tas).
fn meta_weight(m: &DecisionMeta) -> usize {
    fn opt(o: &Option<String>) -> usize {
        o.as_ref().map_or(0, String::len)
    }
    fn vec_s(v: &[String]) -> usize {
        v.iter()
            .map(|x| std::mem::size_of::<String>() + x.len())
            .sum()
    }
    std::mem::size_of::<DecisionMeta>()
        + m.public_id.len()
        + m.jurisdiction_type.len()
        + opt(&m.jurisdiction_code)
        + opt(&m.date_lecture)
        + opt(&m.solution_uid)
        + vec_s(&m.formation_paths)
        + opt(&m.office_uid)
        + opt(&m.legal_domain_uid)
        + vec_s(&m.publication_codes)
        + m.docket_numbers.as_deref().map_or(0, vec_s)
        + opt(&m.summary)
}

/// Octets approximatifs d'un bloc de facettes (libellés + overhead des buckets).
pub(crate) fn facets_weight(f: &SearchFacets) -> usize {
    fn choices(v: &[FacetChoice]) -> usize {
        v.iter()
            .map(|c| {
                std::mem::size_of::<FacetChoice>()
                    + c.value.len()
                    + c.label.len()
                    + c.parent.as_ref().map_or(0, String::len)
            })
            .sum()
    }
    choices(&f.seat)
        + choices(&f.formation)
        + choices(&f.legal_domain)
        + choices(&f.solution)
        + choices(&f.publication)
        + choices(&f.date_lecture_year)
        + f.legal_instrument
            .iter()
            .map(|l| {
                std::mem::size_of::<LegalInstrumentFacet>()
                    + l.value.len()
                    + l.label.len()
                    + choices(&l.articles)
            })
            .sum::<usize>()
}

/// Clé de cache du bundle retrieval : query, filtres, mode. Exclut
/// `offset`/`limit`, `sort` et `ai_mode` — une seule entrée sert les deux modes,
/// toutes les pages et tous les tris.
fn ranked_cache_key(req: &SearchRequest) -> String {
    let mut norm = req.clone();
    norm.offset = 0;
    norm.limit = 0;
    norm.sort = lj_dtos::SortOrder::Relevance;
    norm.ai_mode = false;
    serde_json::to_string(&norm).unwrap_or_default()
}

/// Récupération (RRF) + hydratation → bundle [`RankedResults`], la partie
/// coûteuse mise en cache et ai_mode-indépendante. Le décompte n'en fait pas
/// partie : [`comptage`] le calcule à la demande dans le bundle.
async fn rank_results(
    state: &AppState,
    retrieval: Retrieval,
    query_mode: QueryMode,
) -> Result<RankedResults> {
    let Retrieval {
        fused,
        lexicales,
        pinned,
        pinned_more,
    } = retrieval;
    let all_ids: Vec<i64> = fused.iter().map(|(d, _, _)| *d).collect();
    let hors_question: Vec<i64> = all_ids
        .iter()
        .copied()
        .filter(|d| !lexicales.contains(d))
        .collect();

    // Les deux sont indépendants : lancés en parallèle. Pic de connexions
    // 1 + 1 = 2 ≤ `PEAK_CONNS_PER_SEARCH`.
    let cite_conn = client(state).await?;
    let (decision_paths, meta) = tokio::try_join!(
        async {
            fetch_decision_paths(&cite_conn, &all_ids)
                .await
                .map_err(ApiError::Store)
        },
        hydrate_all(state, &fused),
    )?;
    let pinned = pinned_decisions(state, &pinned).await?;

    Ok(RankedResults {
        query_mode,
        fused,
        meta,
        decision_paths,
        hors_question,
        comptage: tokio::sync::OnceCell::new(),
        pinned,
        pinned_more,
        page_memo: std::sync::Mutex::new(HashMap::new()),
    })
}

/// Title and public id of the designated decisions, in the leg's order.
async fn pinned_decisions(state: &AppState, ids: &[i64]) -> Result<Vec<PinnedDecision>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let conn = client(state).await?;
    let meta = hydrate_decisions(&conn, ids)
        .await
        .map_err(ApiError::Store)?;
    let refs = referential(state).await?;
    Ok(ids
        .iter()
        .filter_map(|d| meta.get(d))
        .map(|m| PinnedDecision {
            id: m.public_id.clone(),
            jurisdiction_type: lj_dtos::JurisdictionType::from_code(&m.jurisdiction_type)
                .expect("a stored jurisdiction_type is a known variant"),
            jurisdiction_code: m.jurisdiction_code.clone(),
            title: display_title(m, &refs),
            seat: display_seat(m, &refs),
            solution: opt_tag(&m.solution_uid, &refs),
            formation: crate::titles::formation_tags(&m.formation_paths),
        })
        .collect())
}

/// Total exact et facettes du bundle, calculés une seule fois : les demandeurs
/// suivants attendent le même résultat, et aucun ne relance la récupération.
/// A list served lexically is counted as the lexical mode, auto's included; a
/// constrained hybrid counts the decisions holding its quoted phrases.
fn counted(req: &SearchRequest, served: QueryMode) -> SearchRequest {
    let constraint = match served {
        QueryMode::Hybrid => quoted_constraint(&req.query),
        _ => None,
    };
    let lexical = served == QueryMode::Lexical || constraint.is_some();
    SearchRequest {
        query: constraint.unwrap_or_else(|| req.query.clone()),
        mode: if lexical {
            SearchMode::Lexical
        } else {
            req.mode
        },
        ..req.clone()
    }
}

async fn comptage<'a>(
    state: &AppState,
    req: &SearchRequest,
    ranked: &'a RankedResults,
) -> Result<&'a (i64, SearchFacets)> {
    ranked
        .comptage
        .get_or_try_init(|| async {
            let conn = client(state).await?;
            let refs = referential(state).await?;
            let served: Vec<i64> = ranked.fused.iter().map(|(d, _, _)| *d).collect();
            let scope = match ranked.query_mode {
                QueryMode::Reference => Scope::Served(&served),
                _ => Scope::Question(&ranked.hors_question),
            };
            let req = &counted(req, ranked.query_mode);
            let (total, counters) = aggregate_facets(&conn, req, scope, Aggs::All).await?;
            Ok((total, facets_from_counters(&counters, &refs)))
        })
        .await
}

/// Tier 2: applies the effective relevance order (reranked in AI mode, else the
/// bundle's RRF), then sort + slice of the page (zero DB) and highlight of the
/// ≤ `limit` hits, memoised by `(ai_mode, sort, offset, limit)`. `all_hit_paths` is
/// recomposed in the effective relevance order.
async fn paginate(
    state: &AppState,
    req: &SearchRequest,
    ranked: &RankedResults,
    rerank_ids: Option<&[i64]>,
    snippet_chars: usize,
) -> Result<SearchResponse> {
    let relevance_ids: Vec<i64> = match rerank_ids {
        Some(ids) => rerank_id_order(&ranked.fused, ids),
        None => ranked.fused.iter().map(|(d, _, _)| *d).collect(),
    };
    let all_hit_paths: Vec<String> = relevance_ids
        .iter()
        .filter_map(|d| ranked.decision_paths.get(d).cloned())
        .collect();

    let page_key = format!(
        "{}|{:?}|{}|{}|{}",
        req.ai_mode, req.sort, req.offset, req.limit, snippet_chars
    );

    let cached_page = ranked.page_memo.lock().unwrap().get(&page_key).cloned();
    let page = if let Some(hits) = cached_page {
        (*hits).clone()
    } else {
        let by_id: HashMap<i64, &(i64, f64, LegHit)> =
            ranked.fused.iter().map(|row| (row.0, row)).collect();
        let mut order: Vec<&(i64, f64, LegHit)> = relevance_ids
            .iter()
            .filter_map(|d| by_id.get(d).copied())
            .filter(|(d, _, _)| ranked.meta.contains_key(d))
            .collect();
        sort_fused(&mut order, &ranked.meta, req.sort);

        let offset = req.offset as usize;
        let slice: Vec<(i64, f64, LegHit)> = if offset >= order.len() {
            Vec::new()
        } else {
            let end = (offset + req.limit as usize).min(order.len());
            order[offset..end].iter().map(|x| (**x).clone()).collect()
        };

        let conn = client(state).await?;
        let refs = referential(state).await?;
        let hits = assemble_page(
            &conn,
            &slice,
            &ranked.meta,
            &req.query,
            &refs,
            snippet_chars,
        )
        .await?;
        ranked
            .page_memo
            .lock()
            .unwrap()
            .insert(page_key, std::sync::Arc::new(hits.clone()));
        hits
    };

    let comptage = ranked.comptage.get();
    Ok(SearchResponse {
        query: req.query.clone(),
        total: comptage.map(|(total, _)| *total),
        hits: page,
        query_mode: ranked.query_mode,
        facets: comptage.map(|(_, facets)| facets.clone()),
        all_hit_paths,
        reranked: rerank_ids.is_some(),
        pinned: ranked.pinned.clone(),
        pinned_more: ranked.pinned_more,
    })
}

/// Bundle retrieval depuis le cache moka, sinon calculé (récupération + RRF +
/// facettes + hydratation, sans rerank) puis caché. Clé ai_mode-indépendante.
///
/// Single-flight sur le chemin qui réussit : `optionally_get_with` ne laisse qu'un
/// appelant calculer. L'échec n'est pas partagé — un suiveur tombé sur un échec
/// recalcule pour obtenir son erreur typée.
async fn ranked_results_cached(
    state: &AppState,
    req: &SearchRequest,
    leg_limit: i64,
    vchord_probes: u32,
    budget_key: Option<&crate::ratelimit::CallerKey>,
) -> Result<std::sync::Arc<RankedResults>> {
    let key = ranked_cache_key(req);
    let mut own_err: Option<ApiError> = None;
    let mut computed = false;
    let hit = state
        .search_cache
        .optionally_get_with(key, async {
            computed = true;
            match rank_uncached(state, req, leg_limit, vchord_probes, budget_key).await {
                Ok(ranked) => Some(ranked),
                Err(err) => {
                    own_err = Some(err);
                    None
                }
            }
        })
        .await;
    crate::cache_metrics::record_access("recherche", !computed);
    match (hit, own_err) {
        (Some(ranked), _) => Ok(ranked),
        (None, Some(err)) => Err(err),
        (None, None) => rank_uncached(state, req, leg_limit, vchord_probes, budget_key).await,
    }
}

/// Budget, DB permit, resolution, retrieval, ranking. Kept apart from
/// [`ranked_results_cached`] so the single flight covers the cache alone.
async fn rank_uncached(
    state: &AppState,
    req: &SearchRequest,
    leg_limit: i64,
    vchord_probes: u32,
    budget_key: Option<&crate::ratelimit::CallerKey>,
) -> Result<std::sync::Arc<RankedResults>> {
    if state.budgets.take(budget_cost(req), budget_key).await
        == crate::ratelimit::Verdict::Exhausted
        && state.settings.search_budget_enforce
    {
        return Err(ApiError::TooManyRequests);
    }
    let _db_permit = state
        .search_permits
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| ApiError::Internal(format!("search semaphore: {e}")))?;
    let resolution = resolve(state, req).await?;
    let (retrieval, mode) = retrieve(state, req, resolution, leg_limit, vchord_probes).await?;
    Ok(std::sync::Arc::new(
        rank_results(state, retrieval, mode).await?,
    ))
}

#[tracing::instrument(skip(state, req, caller, budget_key), fields(
    librejustice.search.engine = tracing::field::Empty,
    librejustice.search.source = tracing::field::Empty,
    librejustice.search.query = tracing::field::Empty,
    librejustice.search.mode = tracing::field::Empty,
    librejustice.search.resolved = tracing::field::Empty,
    librejustice.search.pinned = tracing::field::Empty,
    librejustice.search.identity = tracing::field::Empty,
    librejustice.search.context = tracing::field::Empty,
    librejustice.search.origin = tracing::field::Empty,
    librejustice.search.caller = tracing::field::Empty,
    user_agent.original = tracing::field::Empty,
    librejustice.request.headers = tracing::field::Empty,
    librejustice.request.worker = tracing::field::Empty,
    librejustice.search.visit = tracing::field::Empty,
    librejustice.search.page = tracing::field::Empty,
    librejustice.search.filters = tracing::field::Empty,
    librejustice.search.results_count = tracing::field::Empty,
    librejustice.search.hit_paths = tracing::field::Empty,
    librejustice.search.retrieval_ms = tracing::field::Empty,
    librejustice.search.rerank_ms = tracing::field::Empty,
    librejustice.search.paginate_ms = tracing::field::Empty,
))]
pub async fn search(
    state: &AppState,
    req: &SearchRequest,
    caller: crate::usage::Caller,
    budget_key: Option<&crate::ratelimit::CallerKey>,
) -> Result<SearchResponse> {
    const ENGINE: lj_dtos::SearchEngine = lj_dtos::SearchEngine::Decisions;
    let filters = crate::usage::facet_filters(req).to_string();
    let shape = crate::usage::Shape {
        page: crate::usage::page_number(i64::from(req.offset), i64::from(req.limit)).unwrap_or(1),
        filters: &filters,
    };
    caller.record_span(ENGINE, req.query.as_str(), shape);
    tracing::Span::current().record("librejustice.search.mode", tracing::field::debug(req.mode));
    let resolu = |resp: &SearchResponse| {
        let span = tracing::Span::current();
        span.record(
            "librejustice.search.resolved",
            tracing::field::debug(resp.query_mode),
        );
        span.record("librejustice.search.pinned", resp.pinned.len() as u64);
    };

    if query_lacks_searchable_terms(&req.query) || dated_listing(state, req).await? {
        let resp = browse::browse(state, req).await?;
        resolu(&resp);
        caller.record_outcome(
            ENGINE,
            req.query.as_str(),
            shape,
            resp.total.unwrap_or(resp.all_hit_paths.len() as i64),
        );
        return Ok(resp);
    }

    let leg_limit = state.settings.leg_limit as i64;
    let vchord_probes = state.settings.vchord_probes;
    let t = std::time::Instant::now();
    let ranked = ranked_results_cached(state, req, leg_limit, vchord_probes, budget_key).await?;
    let retrieval = t.elapsed();
    let t = std::time::Instant::now();
    // Channel-scoped, and the order cache is single-flight: two channels landing on
    // the same shortlist at once share the deadline of whichever one executes.
    let rerank_budget = match caller.source {
        lj_dtos::ActivitySource::Mcp => RERANK_BUDGET_MCP,
        lj_dtos::ActivitySource::Web | lj_dtos::ActivitySource::Api => RERANK_BUDGET,
    };
    let rerank_ids = reranked_order(state, req, &ranked, budget_key, rerank_budget).await?;
    let rerank = t.elapsed();
    let t = std::time::Instant::now();
    let snippet_chars = snippets::budget(caller.context, snippets::DEFAULT_MAX_CHARS);
    let resp = paginate(
        state,
        req,
        &ranked,
        rerank_ids.as_deref().map(Vec::as_slice),
        snippet_chars,
    )
    .await?;

    resolu(&resp);
    let span = tracing::Span::current();
    span.record(
        "librejustice.search.retrieval_ms",
        retrieval.as_millis() as u64,
    );
    span.record("librejustice.search.rerank_ms", rerank.as_millis() as u64);
    span.record(
        "librejustice.search.paginate_ms",
        t.elapsed().as_millis() as u64,
    );
    span.record(
        "librejustice.search.hit_paths",
        resp.all_hit_paths
            .iter()
            .take(crate::rerank::RERANK_K)
            .cloned()
            .collect::<Vec<_>>()
            .join(",")
            .as_str(),
    );
    // Le total exact ne se paie plus ici : `results_count` porte le nombre servi.
    caller.record_outcome(
        ENGINE,
        req.query.as_str(),
        shape,
        resp.all_hit_paths.len() as i64,
    );
    Ok(resp)
}

pub async fn count(
    state: &AppState,
    req: &SearchRequest,
    budget_key: Option<&crate::ratelimit::CallerKey>,
) -> Result<lj_dtos::SearchCountResponse> {
    if query_lacks_searchable_terms(&req.query) || dated_listing(state, req).await? {
        let refs = referential(state).await?;
        let counts = browse::totals_and_facets(state, req, &refs).await?;
        return Ok(lj_dtos::SearchCountResponse {
            total: counts.0,
            facets: counts.1.clone(),
        });
    }
    let ranked = ranked_results_cached(
        state,
        req,
        state.settings.leg_limit as i64,
        state.settings.vchord_probes,
        budget_key,
    )
    .await?;
    let (total, facets) = comptage(state, req, &ranked).await?;
    Ok(lj_dtos::SearchCountResponse {
        total: *total,
        facets: facets.clone(),
    })
}

/// Catalogue candidates for a text typed in the « Textes cités » panel, capped
/// so the aggregate stays one scan.
const SUGGEST_ARTICLES: i64 = 20;
const SUGGEST_TEXTS: i64 = 8;
/// Rows served back. Ten codes carrying the same number is the shape of the
/// answer to « l'article L761-2 », not a list to shorten further.
const SUGGEST_ROWS: usize = 10;

/// The `include` regex of the aggregate compiles to a bounded number of states and
/// a key costs about one per escaped character: past the bound the whole call
/// fails. Trimming here rather than in the
/// aggregate is what keeps the keys counted and the rows served the same set.
const INCLUDE_BUDGET: usize = 900;

/// How many of `keys` fit the budget, in order.
fn fitting(keys: &[String]) -> usize {
    let mut spent = 0;
    keys.iter()
        .take_while(|k| {
            spent += k.chars().filter(|c| !c.is_ascii_alphanumeric()).count() + k.chars().count();
            spent <= INCLUDE_BUDGET
        })
        .count()
}

fn article_label(num_key: &str, num: &str) -> String {
    match lj_norm::article_key::display(num_key) {
        spelled if spelled != num_key => spelled,
        _ => num.to_string(),
    }
}

/// The catalogue texts and articles the typed text names, counted on the
/// current search. The facet serves the head of its vocabulary, 30 texts and 20
/// articles each, so a number cited by 84 decisions is unreachable through it:
/// what is named is counted by name, down to zero.
pub async fn suggest_instruments(
    state: &AppState,
    req: &SearchRequest,
    needle: &str,
    budget_key: Option<&crate::ratelimit::CallerKey>,
) -> Result<lj_dtos::LegalInstrumentSuggestions> {
    let needle = needle.trim();
    if needle.chars().count() < 2 {
        return Ok(lj_dtos::LegalInstrumentSuggestions::default());
    }
    let conn = client(state).await?;
    let repo = lj_store::repository::DecisionRepository::new(&conn);
    let mut articles = match lj_norm::article_key::filter_article_key(needle) {
        Some(key) => repo
            .suggest_articles(&key, needle, SUGGEST_ARTICLES)
            .await
            .map_err(|e| ApiError::Internal(format!("articles du catalogue : {e}")))?,
        None => Vec::new(),
    };
    let texts = repo
        .suggest_instruments(needle, SUGGEST_TEXTS)
        .await
        .map_err(|e| ApiError::Internal(format!("textes du catalogue : {e}")))?;

    let mut keys: Vec<String> = articles
        .iter()
        .map(|a| format!("{}|{}", a.text_uid, a.num_key))
        .collect();
    keys.truncate(fitting(&keys));
    articles.truncate(keys.len());

    let mut uids: Vec<String> = Vec::new();
    let mut titles: Vec<(String, Option<String>)> = Vec::new();
    for (uid, title, slug) in articles
        .iter()
        .map(|a| (a.text_uid.clone(), a.title.clone(), a.slug.clone()))
        .chain(
            texts
                .into_iter()
                .map(|t| (t.text_uid, t.title, Some(t.slug))),
        )
    {
        if !uids.contains(&uid) {
            uids.push(uid);
            titles.push((title, slug));
        }
    }
    uids.truncate(fitting(&uids));
    titles.truncate(uids.len());
    if uids.is_empty() {
        return Ok(lj_dtos::LegalInstrumentSuggestions::default());
    }

    let aggs = Aggs::Keys {
        instruments: &uids,
        articles: &keys,
    };
    let counters = if query_lacks_searchable_terms(&req.query) {
        aggregate_facets(&conn, req, Scope::Question(&[]), aggs).await
    } else {
        let ranked = ranked_results_cached(
            state,
            req,
            state.settings.leg_limit as i64,
            state.settings.vchord_probes,
            budget_key,
        )
        .await?;
        let served: Vec<i64> = ranked.fused.iter().map(|(d, _, _)| *d).collect();
        let scope = match ranked.query_mode {
            QueryMode::Reference => Scope::Served(&served),
            _ => Scope::Question(&ranked.hors_question),
        };
        aggregate_facets(&conn, &counted(req, ranked.query_mode), scope, aggs).await
    }?
    .1;

    let mut rows: Vec<LegalInstrumentFacet> = uids
        .into_iter()
        .zip(titles)
        .map(|(uid, (label, slug))| LegalInstrumentFacet {
            count: counters.instrument.counts.get(&uid).copied().unwrap_or(0),
            articles: articles
                .iter()
                .filter(|a| a.text_uid == uid)
                .map(|a| FacetChoice {
                    count: counters
                        .instrument
                        .articles
                        .get(&uid)
                        .and_then(|m| m.get(&a.num_key))
                        .copied()
                        .unwrap_or(0),
                    label: article_label(&a.num_key, &a.num),
                    value: a.num_key.clone(),
                    parent: None,
                    hint: None,
                })
                .collect(),
            value: uid,
            label,
            slug,
        })
        .collect();
    // The code spelled out comes first, then the one case law cites most. On
    // the count alone, « l'article L761-2 du code de la consommation » answers
    // with the labour code, which carries that number more often; on
    // the catalogue score alone, « 700 » answers with the Senegalese code of
    // civil procedure, whose short title outscores the French one it never
    // cites, and « L. 761-1 CJA » ranks the CJA low, an acronym being in no
    // title.
    let named = lj_fold::slug(needle, '-');
    rows.sort_by_key(|r| {
        let met = match r.articles.is_empty() {
            true => r.count,
            false => r.articles.iter().map(|a| a.count).max().unwrap_or(0),
        };
        (
            !named.contains(&lj_fold::slug(&r.label, '-')),
            std::cmp::Reverse(met),
        )
    });
    rows.truncate(SUGGEST_ROWS);
    Ok(lj_dtos::LegalInstrumentSuggestions {
        legal_instrument: rows,
    })
}

/// Recherche d'un appelant qui ne reviendra pas chercher son décompte : MCP,
/// dont le schéma de sortie exige `total` et `facets`.
pub async fn search_with_count(
    state: &AppState,
    req: &SearchRequest,
    caller: crate::usage::Caller,
    budget_key: Option<&crate::ratelimit::CallerKey>,
) -> Result<SearchResponse> {
    let mut resp = search(state, req, caller, budget_key).await?;
    if resp.total.is_none() {
        let compte = count(state, req, budget_key).await?;
        resp.total = Some(compte.total);
        resp.facets = Some(compte.facets);
    }
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(query: &str, mode: &str) -> SearchRequest {
        serde_json::from_value(serde_json::json!({ "query": query, "mode": mode })).unwrap()
    }

    #[test]
    fn a_decision_the_reranker_did_not_judge_keeps_its_rank() {
        let base: Vec<(i64, f64, LegHit)> = (1..=60)
            .map(|d| (d, 0.0, LegHit::synthetic_title_only(d)))
            .collect();
        let judged: Vec<i64> = (1..=50).rev().filter(|d| *d != 3).collect();
        let order = rerank_id_order(&base, &judged);
        assert_eq!(&order[..4], &[50, 49, 3, 48]);
        assert_eq!(&order[50..], &(51..=60).collect::<Vec<_>>()[..]);
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (1..=60).collect::<Vec<_>>());
    }

    #[test]
    fn la_requete_qui_designe_une_decision_part_en_reference() {
        for q in [
            "\"17-24.681\"",
            "n° 412345",
            "n° 1801234",
            "19PA01234",
            "41234/22",
            "2012-245 QPC",
            "88-123 DC conseil constitutionnel 5 mars 1988",
            "\"2025-901 DC\"",
            "ECLI:FR:CCASS:2015:C100123",
            "2401234",
            " 1801234 ",
            "412345",
        ] {
            assert_eq!(
                resolve_shape(&req(q, "auto")),
                Some(Resolution::Reference),
                "{q}"
            );
        }
        for q in [
            "\"13-14.567\" OU valeur des parts sociales",
            "\"14-12.345\" ET \"cour d'appel de Rennes\"",
            "19PA01234 SAUF référé",
            "412345 Durand délai raisonnable",
            "69003",
            "44100",
            "Danthony",
        ] {
            assert_eq!(resolve_shape(&req(q, "auto")), None, "{q}");
        }
    }

    #[test]
    fn le_mode_demande_est_servi_tel_quel_sans_carte() {
        for q in ["\"17-24.681\"", "Danthony", "69003", "expulsion locative"] {
            assert_eq!(
                resolve_shape(&req(q, "lexical")),
                Some(Resolution::Lexical),
                "{q}"
            );
            assert_eq!(
                resolve_shape(&req(q, "semantic")),
                Some(Resolution::Semantic),
                "{q}"
            );
        }
    }

    #[test]
    fn le_mode_reference_demande_tient_sous_un_operateur_booleen() {
        for q in [
            "\"17-24.681\"",
            "\"24-10.101\" OU \"24-10.102\"",
            "\"14-12.345\" ET \"cour d'appel de Rennes\"",
            "expulsion locative",
        ] {
            assert_eq!(
                resolve_shape(&req(q, "reference")),
                Some(Resolution::Reference),
                "{q}"
            );
        }
    }

    #[test]
    fn la_designation_dans_une_requete_de_contenu_pose_une_carte() {
        let seats: std::collections::HashSet<String> = ["toulouse", "paris", "marseille", "lyon"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let number = || Some(Designation::Number { weak: false });
        let weak = || Some(Designation::Number { weak: true });
        let name = |covered: &str| {
            Some(Designation::Name {
                bearers: vec![1],
                covered: covered.split(' ').map(str::to_string).collect(),
            })
        };
        let danthony = || name("danthony");
        let beziers = || name("commune beziers");
        let beziers_court = || name("beziers");
        for (q, designation, attendu) in [
            ("expulsion locative", None, Resolution::Semantic),
            (
                "Conseil d'État 2023 expropriation",
                None,
                Resolution::Semantic,
            ),
            ("conseil d'état 1935", None, Resolution::Semantic),
            (
                "cour d'appel rennes 3 mars 2021 heures supplémentaires",
                None,
                Resolution::Semantic,
            ),
            (
                "cour d'appel de toulouse 14 mai 2003",
                None,
                Resolution::Reference,
            ),
            ("caa paris 9 juin 2023", None, Resolution::Reference),
            (
                "caa paris 9 juin 2023 OU 10 juin 2023",
                None,
                Resolution::Lexical,
            ),
            ("congés ET payés", None, Resolution::Lexical),
            ("\"force majeure\"", None, Resolution::Lexical),
            (
                "4823 conseil d'etat 1976",
                weak(),
                Resolution::SemanticWithCard,
            ),
            (
                "TA Lyon 15 mars 2021 2101234",
                number(),
                Resolution::Reference,
            ),
            (
                "104567 durand 5 mai 1993",
                number(),
                Resolution::SemanticWithCard,
            ),
            ("Béziers CAA Lyon", beziers_court(), Resolution::Reference),
            ("69003", number(), Resolution::LexicalWithCard),
            (
                "412345 Durand délai raisonnable",
                number(),
                Resolution::SemanticWithCard,
            ),
            (
                "barème 300000 indemnisation du préjudice",
                number(),
                Resolution::SemanticWithCard,
            ),
            (
                "\"13-14.567\" OU valeur des parts",
                number(),
                Resolution::LexicalWithCard,
            ),
            ("Danthony", danthony(), Resolution::LexicalWithCard),
            ("arrêt Nicolo", name("nicolo"), Resolution::LexicalWithCard),
            (
                "l'arrêt dit Nicolo",
                name("nicolo"),
                Resolution::LexicalWithCard,
            ),
            (
                "Marckx c. Belgique",
                name("marckx belgique"),
                Resolution::LexicalWithCard,
            ),
            ("\"Danthony\"", danthony(), Resolution::LexicalWithCard),
            (
                "Danthony consultation irrégulière",
                danthony(),
                Resolution::SemanticWithCard,
            ),
            ("Commune de Béziers", beziers(), Resolution::LexicalWithCard),
            (
                "Commune de Béziers résiliation unilatérale",
                beziers(),
                Resolution::SemanticWithCard,
            ),
            ("Béziers", beziers_court(), Resolution::LexicalWithCard),
            (
                "Béziers 2011",
                beziers_court(),
                Resolution::SemanticWithCard,
            ),
            (
                "doctrine Béziers II",
                beziers_court(),
                Resolution::SemanticWithCard,
            ),
            (
                "Béziers conseil d'État",
                beziers_court(),
                Resolution::Reference,
            ),
            ("Béziers CE 2011", beziers_court(), Resolution::Reference),
            ("Danthony CE", danthony(), Resolution::Reference),
            ("412345 conseil d'État", number(), Resolution::Reference),
            (
                "Béziers OU Danthony CE",
                beziers_court(),
                Resolution::LexicalWithCard,
            ),
            (
                "Béziers conseil d'État résiliation unilatérale",
                beziers_court(),
                Resolution::SemanticWithCard,
            ),
        ] {
            assert_eq!(resolve_content(q, designation, &seats), attendu, "{q}");
        }
    }

    #[test]
    fn la_jambe_titre_d_une_reference_filtre_sur_la_cour_et_le_jour_ecrits() {
        let row = |code: &str, kind: &str, city: Option<&str>, label: &str| {
            lj_store::repository::JurisdictionRow {
                code: code.to_string(),
                source_code: code.to_string(),
                jurisdiction_type: kind.to_string(),
                city: city.map(str::to_string),
                label: label.to_string(),
                place: None,
            }
        };
        let refs = crate::referential::Referential::new(
            vec![],
            vec![
                row("ce", "CE", None, "Conseil d'État"),
                row(
                    "caa_paris",
                    "CAA",
                    Some("Paris"),
                    "Cour administrative d'appel de Paris",
                ),
                row(
                    "caa_lyon",
                    "CAA",
                    Some("Lyon"),
                    "Cour administrative d'appel de Lyon",
                ),
                row("ca_paris", "CA", Some("Paris"), "Cour d'appel de Paris"),
            ],
            Vec::new(),
        );
        let filters = |q: &str| {
            let t = title_request(&refs, &req(q, "auto"));
            (
                t.jurisdiction_code,
                t.jurisdiction_type.map(|v| v.len()),
                t.date_from,
                t.date_to,
            )
        };
        assert_eq!(
            filters("caa paris 9 juin 2023"),
            (
                Some(vec!["caa_paris".to_string()]),
                None,
                Some("2023-06-09".to_string()),
                Some("2023-06-09".to_string())
            )
        );
        assert_eq!(
            filters("cour d'appel de toulouse 14 mai 2003"),
            (
                None,
                Some(1),
                Some("2003-05-14".to_string()),
                Some("2003-05-14".to_string())
            )
        );
        assert_eq!(
            filters("Béziers conseil d'État"),
            (Some(vec!["ce".to_string()]), None, None, None)
        );
        assert_eq!(filters("Béziers"), (None, None, None, None));
        let mut own = req("caa paris 9 juin 2023", "auto");
        own.date_from = Some("2020-01-01".to_string());
        own.jurisdiction_code = Some(vec!["caa_lyon".to_string()]);
        let kept = title_request(&refs, &own);
        assert_eq!(kept.date_from.as_deref(), Some("2020-01-01"));
        assert_eq!(kept.date_to, None);
        assert_eq!(kept.jurisdiction_code, Some(vec!["caa_lyon".to_string()]));
    }
}
