//! Recherche hybride (BM25 ParadeDB `@@@` + ANN `<=>`) — port de `search.py`.
//!
//! 1. `embed_query` produit le vecteur requête (chemin hybride).
//! 2. En parallèle :
//!    - **BM25 body** : `decisions_bm25` (`full_text`) au grain décision
//!      (ADR 0084) — phrase-combo S5 (phrase entière + runs content ≥2 + tokens
//!      OR) ou la traduction booléenne ; `LIMIT leg_limit` direct, plus de
//!      sur-récupération ni de pooling (supersede la jambe chunk de l'ADR 0080).
//!    - **BM25 titre** : `decisions_bm25` (`search_title`) au grain décision,
//!      mêmes filtres que la jambe body.
//!    - **ANN** VectorChord : `ORDER BY embedding <=> quantize_to_rabitq8(...)`
//!      sur `decision_chunks` — la SEULE jambe encore au grain chunk
//!      (max-pool ×1,1 par décision, ADR 0080 conservé : on ne somme jamais des
//!      embeddings).
//! 3. Fusion RRF au grain décision pondérée adaptative (signals), cap + seuil
//!    relatif.
//! 4. Hydratation métadonnées + facettes + snippets (`<mark>` depuis
//!    `full_text` ; fenêtre ancrée au chunk gagnant pour un hit ANN).
//!
//! Le tri stable tie-break par `source_uid`/id est assuré par l'ordre déterministe
//! des `ORDER BY` SQL et le tri final sur le score puis l'id.
//!
//! NB scope : ce port couvre le retrieval + fusion + facettes + hydratation +
//! snippets + rerank LLM (ai_mode, top-50 avant tri par date). Le résultat ranké
//! (post-rerank) est mis en cache moka in-process par requête (cf.
//! [`RankedResults`]), donc le rerank n'est pas rejoué par page ni sur requête
//! répétée tant que l'entrée est chaude (TTL 1 h).

mod adaptive;
mod dates;
mod filters;
mod hydrate;
mod legs;
mod query;
pub mod suggest;

use std::collections::HashMap;

use deadpool_postgres::Client;
use lj_llm::backend::Embedder;
use lj_llm::mistral::{key_fingerprint, MistralClient};
use lj_store::repository::DecisionRepository;

use crate::error::{ApiError, Result};
use crate::referential::referential;
use crate::rerank::{rerank_shortlist, RerankItem, RERANK_K};
use crate::signals;
use crate::state::AppState;

use lj_dtos::{
    FacetChoice, LegalInstrumentFacet, QueryMode, SearchFacets, SearchHit, SearchRequest,
    SearchResponse, SortOrder,
};

use adaptive::compute_adaptive_weights;
use hydrate::{
    assemble_page, display_title, fetch_pub_ids_and_facets, hydrate_decisions, DecisionMeta,
};
use legs::{
    ann_leg, bm25_leg, bm25_parse_leg, fuse_ranks, pool_max_per_decision, title_leg,
    truncate_fused, LegHit,
};
use query::{detect_query_mode, is_boolean_query, query_lacks_searchable_terms, translate_boolean};

pub(crate) use dates::{parse_search_date, DateError, DATE_GE, DATE_LE};
pub use query::{body_query_for_arm, BodyArm};

// ── API bancs offline (lj-bench rank-arms / arm-latency / rank-bsweep) ────────

/// Récupération LEXICALE pure pour l'A/B de ranking offline (`lj-bench
/// rank-arms`) : jambe BM25 body (corps construit selon `arm`) + jambe titre,
/// fusion RRF aux poids de base, troncature relative — exactement le chemin
/// [`retrieve_boolean`] non-booléen du prod, le corps mis à part. Les deux jambes
/// tournent en séquence sur la même connexion (le résultat est déterministe ;
/// seule la latence diffère du `try_join` prod à deux connexions). Renvoie les
/// `decision_id` en ordre de pertinence (= `all_hit_ids` lexical).
pub async fn lexical_rank_for_arm(
    conn: &Client,
    query: &str,
    arm: BodyArm,
    leg_limit: i64,
) -> anyhow::Result<Vec<i64>> {
    let req: SearchRequest = serde_json::from_value(serde_json::json!({ "query": query }))?;
    let body_query = body_query_for_arm(query, arm);
    let bm25_scores = bm25_parse_leg(conn, &req, &body_query, leg_limit).await?;
    let title_scores = title_leg(conn, &req, leg_limit).await?;
    let empty: HashMap<i64, LegHit> = HashMap::new();
    let fused = truncate_fused(fuse_ranks(
        &bm25_scores,
        &empty,
        &title_scores,
        signals::BASE_WEIGHTS.bm25,
        signals::BASE_WEIGHTS.ann,
        signals::BASE_WEIGHTS.title,
    ));
    Ok(fused.into_iter().map(|(d, _, _)| d).collect())
}

/// Récupération HYBRIDE pour l'A/B de ranking offline (`lj-bench rank-arms
/// --mode hybrid`) : embedding requête + jambes BM25 body (corps selon `arm`) /
/// ANN / titre, poids RRF adaptatifs (signals), troncature relative — exactement
/// le chemin [`retrieve_hybrid`] du prod (mode `auto`, défaut utilisateur), le
/// corps mis à part. Les jambes ANN et titre sont identiques pour tous les bras
/// (le corps ne les touche pas) ; seules la jambe BM25 et, par ricochet, les
/// poids adaptatifs varient. Renvoie les `decision_id` en ordre de pertinence RRF
/// (pré-rerank ; le rerank LLM ne s'applique pas hors mode IA).
pub async fn hybrid_rank_for_arm(
    state: &AppState,
    query: &str,
    arm: BodyArm,
) -> anyhow::Result<Vec<i64>> {
    let req: SearchRequest = serde_json::from_value(serde_json::json!({ "query": query }))?;
    let leg_limit = state.settings.leg_limit as i64;
    let vchord_probes = state.settings.vchord_probes;

    // Les 3 bras d'une même requête partagent l'embedding (cache in-process).
    let query_vec = embed_query_cached(state, &req.query).await?;

    let body_query = body_query_for_arm(query, arm);
    let bm25_conn = client(state).await?;
    let mut ann_conn = client(state).await?;
    let title_conn = client(state).await?;
    let (bm25_scores, ann_hits, title_scores) = tokio::try_join!(
        bm25_parse_leg(&bm25_conn, &req, &body_query, leg_limit),
        ann_leg(&mut ann_conn, &req, &query_vec, leg_limit, vchord_probes),
        title_leg(&title_conn, &req, leg_limit),
    )?;
    let ann_best = pool_max_per_decision(&ann_hits, leg_limit as usize);
    let weights = compute_adaptive_weights(state, &req.query, &bm25_scores, &ann_hits).await?;
    let fused = truncate_fused(fuse_ranks(
        &bm25_scores,
        &ann_best,
        &title_scores,
        weights.bm25,
        weights.ann,
        weights.title,
    ));
    Ok(fused.into_iter().map(|(d, _, _)| d).collect())
}

/// Recherche complète SANS le cache résultats moka (banc de charge `lj-bench
/// load`) : exécute TOUJOURS récupération + RRF + facettes + hydratation +
/// assemblage de page, mais conserve le permit d'admission (`search_permits`) et
/// le cache embeddings. Mesure la capacité DB « tous-miss » sans cache-buster la
/// requête — la cache-buster invaliderait aussi le cache embeddings (→ un appel
/// réseau d'embedding par requête, qui masquerait le coût DB). Le permit est tenu
/// exactement comme en prod : sur récupération + ranking, relâché avant le
/// highlight de page ([`search`] via [`ranked_results_cached`]). `ai_mode` ignoré
/// (ordre RRF) — le rerank LLM est hors périmètre charge.
pub async fn search_nocache(state: &AppState, req: &SearchRequest) -> Result<SearchResponse> {
    let leg_limit = state.settings.leg_limit as i64;
    let vchord_probes = state.settings.vchord_probes;
    let query_mode = effective_query_mode(req);
    let t0 = std::time::Instant::now();
    let (ranked, retrieval_ms) = {
        let _db_permit = state
            .search_permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| ApiError::Internal(format!("search semaphore: {e}")))?;
        let fused = if query_lacks_searchable_terms(&req.query) {
            Vec::new()
        } else {
            match query_mode {
                QueryMode::Lexical => retrieve_boolean(state, req, leg_limit).await?,
                QueryMode::Hybrid => retrieve_hybrid(state, req, leg_limit, vchord_probes).await?,
            }
        };
        let retrieval_ms = t0.elapsed().as_secs_f64() * 1000.0;
        (
            std::sync::Arc::new(rank_results(state, fused, query_mode).await?),
            retrieval_ms,
        )
    };
    let hydrate_ms = t0.elapsed().as_secs_f64() * 1000.0 - retrieval_ms;
    let t1 = std::time::Instant::now();
    let resp = paginate(state, req, &ranked, None).await?;
    // Décompose le coût d'une requête réelle pour le banc (`lj-bench load`) :
    // récupération (3 jambes) / hydratation DB (facettes + metadata) / page
    // (snippet : TOAST + highlight CPU). Cible `lj_bench_timing` (off par défaut).
    tracing::info!(
        target: "lj_bench_timing",
        retrieval_ms,
        hydrate_ms,
        paginate_ms = t1.elapsed().as_secs_f64() * 1000.0,
        "search_nocache phases"
    );
    Ok(resp)
}

/// Embedding requête via le cache in-process (parité prod : un texte → un
/// vecteur stable, partagé entre les bras et les jambes d'une même requête).
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

/// Latence isolée de la jambe BM25 body pour un bras (banc `lj-bench
/// arm-latency`). La jambe body est la SEULE qui varie entre bras : la
/// chronométrer seule donne le coût propre de la machinerie phrase, hors jambes
/// titre/ANN (identiques d'un bras à l'autre) et hors fusion. Renvoie
/// `(nb_hits, durée d'UN appel)` ; l'appelant boucle pour la statistique.
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

/// Ranking de la SEULE jambe body BM25 (sans jambe titre ni fusion RRF), pour un
/// bras de construction de requête. Oracle de fidélité du re-scorer BM25 offline
/// (`lj-bench rank-bsweep`) : la réimplémentation pure de BM25 doit reproduire ce
/// classement de l'index `decisions_bm25` au `b` par défaut avant qu'on ne balaye
/// `b`. Renvoie `(decision_id, score)` trié score décroissant puis id croissant
/// (même tie-break que les jambes).
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

/// Latence isolée de la jambe titre (banc `arm-latency`) : identique d'un bras à
/// l'autre, sert de point de comparaison au coût body. Renvoie (nb_hits, durée).
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

/// Latence isolée de la jambe ANN (banc `arm-latency`) : embedding HORS chrono
/// (cache in-process, parité prod), on ne chronomètre que la requête VectorChord.
/// Souvent la jambe la plus lente du `try_join` prod — la situer face au coût
/// body dit si le surcoût `split` est sur le chemin critique ou masqué derrière
/// elle. Renvoie (nb_hits, durée de la requête ANN seule).
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

/// Hydratation DB de TOUS les hits rankés (≤100) : metadata seule. **Aucun
/// highlight** — c'est le Tier 1 du cache (cf. [`RankedResults`]) : la partie
/// partagée par toutes les pages/tris, calculée une fois. Le highlight (depuis
/// `full_text`) est par-page ([`assemble_page`]).
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

/// Hydrate des `SearchHit` complets pour une liste **ordonnée** d'ids de
/// décisions, SANS requête de recherche (score 0, snippet = tête du texte) —
/// chemin partagé des listes de décisions hors recherche (fiche entité ADR
/// 0189…), même assemblage que les pages de résultats ([`assemble_page`]).
/// Renvoie `(decision_id, hit)` dans l'ordre d'entrée ; les ids sans metadata
/// affichable (public_id absent) sont silencieusement omis.
pub(crate) async fn hits_for_decision_ids(
    conn: &Client,
    decision_ids: &[i64],
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
    let hits = assemble_page(conn, &page, &meta, "", refs).await?;
    // `assemble_page` préserve l'ordre de `page` (déjà filtrée sur la metadata
    // présente) : le zip id ↔ hit est positionnel.
    Ok(page.iter().map(|(id, _, _)| *id).zip(hits).collect())
}

/// Ordre relevance → ordre d'affichage : trie une vue de la liste fused par date
/// (date_desc/asc) via la metadata hydratée, sans tie-break. `sort_by` stable +
/// `order` déjà en pertinence ⇒ égalité de date = ordre de pertinence préservé
/// (parité du `sorted(..., reverse=...)` stable de Python). Date nulle → ""
/// (comme le `else ""` Python ; même format "YYYY-MM-DD" que `date_lecture`).
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

#[tracing::instrument(skip(state, req), fields(leg_limit))]
async fn retrieve_boolean(
    state: &AppState,
    req: &SearchRequest,
    leg_limit: i64,
) -> Result<Vec<(i64, f64, LegHit)>> {
    let bm25_conn = client(state).await?;
    let title_conn = client(state).await?;
    let body_query = if is_boolean_query(&req.query) {
        translate_boolean(&req.query)
    } else {
        req.query.clone()
    };
    let (bm25_scores, title_scores) = if is_boolean_query(&req.query) {
        tokio::try_join!(
            bm25_parse_leg(&bm25_conn, req, &body_query, leg_limit),
            title_leg(&title_conn, req, leg_limit),
        )
    } else {
        // Détecté lexical via un autre signal (rare) : on retombe sur le
        // phrase-combo standard de la jambe body.
        tokio::try_join!(
            bm25_leg(&bm25_conn, req, leg_limit),
            title_leg(&title_conn, req, leg_limit),
        )
    }
    .map_err(ApiError::Store)?;

    let empty: HashMap<i64, LegHit> = HashMap::new();
    let fused = truncate_fused(fuse_ranks(
        &bm25_scores,
        &empty,
        &title_scores,
        signals::BASE_WEIGHTS.bm25,
        signals::BASE_WEIGHTS.ann,
        signals::BASE_WEIGHTS.title,
    ));
    Ok(fused)
}

#[tracing::instrument(skip(state, req), fields(leg_limit, vchord_probes))]
async fn retrieve_hybrid(
    state: &AppState,
    req: &SearchRequest,
    leg_limit: i64,
    vchord_probes: u32,
) -> Result<Vec<(i64, f64, LegHit)>> {
    // Embedder partagé (state) + cache in-process des vecteurs (moka, TTL 7 j) :
    // un texte donné → un vecteur stable, on évite l'aller-retour backend
    // d'embedding sur une requête répétée (parité du cache embeddings Redis
    // Python). `embed_query` applique l'instruction (format_query) en interne —
    // passer la requête brute (pré-formater ici doublait le préfixe
    // « Instruct…Query: Instruct…Query: q » et changeait le vecteur).
    let query_arr = match state.embedding_cache.get(&req.query).await {
        Some(vec) => vec,
        None => {
            let arr = state
                .embedder
                .embed_query(std::slice::from_ref(&req.query))
                .await
                .map_err(|e| ApiError::Internal(format!("embed: {e}")))?;
            let vec = std::sync::Arc::new(arr.row(0).to_owned());
            state
                .embedding_cache
                .set(&req.query, std::sync::Arc::clone(&vec))
                .await;
            vec
        }
    };
    let query_vec: Vec<f32> = query_arr.iter().copied().collect();

    // Bloc dédié : les 3 conns des jambes sont relâchées dès le `try_join` fini
    // (les résultats sont possédés), AVANT `compute_adaptive_weights` qui en
    // reprend 2 en parallèle. Sans ce drop, le pic serait 3 + 2 = 5 > PEAK_CONNS.
    let (bm25_scores, ann_hits, title_scores) = {
        let bm25_conn = client(state).await?;
        let mut ann_conn = client(state).await?;
        let title_conn = client(state).await?;
        tokio::try_join!(
            bm25_leg(&bm25_conn, req, leg_limit),
            ann_leg(&mut ann_conn, req, &query_vec, leg_limit, vchord_probes),
            title_leg(&title_conn, req, leg_limit),
        )
        .map_err(ApiError::Store)?
    };

    let ann_best = pool_max_per_decision(&ann_hits, leg_limit as usize);
    let weights = compute_adaptive_weights(state, &req.query, &bm25_scores, &ann_hits).await?;
    let fused = truncate_fused(fuse_ranks(
        &bm25_scores,
        &ann_best,
        &title_scores,
        weights.bm25,
        weights.ann,
        weights.title,
    ));
    Ok(fused)
}

/// Clé du cache rerank : `(query, ensemble TRIÉ du top-[`RERANK_K`])`. Trié car
/// la sortie du rerank ne dépend que du SET de docs + de la query (le reranker
/// shuffle son entrée en interne) — deux récupérations donnant le même top-50,
/// même dans un ordre RRF différent, partagent ainsi l'ordre LLM. Indépendant de
/// `sort`/`offset`/`limit` (le rerank ne s'applique qu'au tri relevance).
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

/// Applique l'ordre LLM `new_ids` sur le head (top-[`RERANK_K`]) de `base` (ordre
/// RRF) et renvoie la séquence COMPLÈTE d'ids en ordre relevance reranké : head
/// réordonné par `new_ids`, ids du head non couverts (hors `new_ids`, ex. sans
/// résumé) remis en queue de head dans l'ordre RRF, puis la tail intacte.
/// Conserve TOUS les ids de `base` — sinon `all_hit_ids` perdrait des entrées et
/// `total` divergerait.
fn rerank_id_order(base: &[(i64, f64, LegHit)], new_ids: &[i64]) -> Vec<i64> {
    let head_len = RERANK_K.min(base.len());
    let head_ids: Vec<i64> = base[..head_len].iter().map(|(d, _, _)| *d).collect();
    let mut remaining: std::collections::HashSet<i64> = head_ids.iter().copied().collect();
    let mut out: Vec<i64> = Vec::with_capacity(base.len());
    for d in new_ids {
        if remaining.remove(d) {
            out.push(*d);
        }
    }
    for d in &head_ids {
        if remaining.remove(d) {
            out.push(*d);
        }
    }
    out.extend(base[head_len..].iter().map(|(d, _, _)| *d));
    out
}

/// Ordre LLM du top-[`RERANK_K`] si le mode IA s'applique, sinon `None` (ordre
/// RRF naturel). No-op silencieux (→ `None`) si : `ai_mode` désactivé, retrieval
/// lexical, sort ≠ relevance, clé Mistral absente, shortlist < 3, ou échec du
/// rerank (fallback retrieval).
///
/// L'ordre LLM est mis en cache par `(query, top-ids)` (cf. [`rerank_cache_key`]) :
/// le quota Mistral n'est consommé qu'UNE fois par shortlist — un toggle
/// IA on↔off, ou une reconstruction du bundle au même top-50, sert le cache sans
/// rappeler le LLM. Body-source = `bundle.meta` (title + summary déjà hydratés au
/// retrieval) : ZÉRO requête DB ici. Renvoie l'`Arc<Vec<i64>>` cachable tel quel
/// ([`rerank_id_order`] le déplie par-page en ordre complet).
async fn reranked_order(
    state: &AppState,
    req: &SearchRequest,
    bundle: &RankedResults,
) -> Result<Option<std::sync::Arc<Vec<i64>>>> {
    if !req.ai_mode
        || bundle.query_mode == QueryMode::Lexical
        || req.sort != SortOrder::Relevance
        || bundle.fused.len() < 3
    {
        return Ok(None);
    }
    let mistral_api_keys = &state.settings.mistral_api_keys;
    if mistral_api_keys.is_empty() {
        tracing::warn!(
            "ai_mode requested but LIBREJUSTICE_MISTRAL_API_KEYS missing — skipping rerank"
        );
        return Ok(None);
    }
    // Écarte les clés désactivées (`mistral_key_status`). Connexion courte,
    // relâchée avant les appels LLM.
    let live_keys: Vec<String> = {
        let conn = state
            .pool
            .get()
            .await
            .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
        let disabled = DecisionRepository::new(&conn)
            .disabled_mistral_key_fingerprints()
            .await
            .map_err(ApiError::Store)?;
        mistral_api_keys
            .iter()
            .filter(|k| !disabled.contains(&key_fingerprint(k)))
            .cloned()
            .collect()
    };
    if live_keys.is_empty() {
        tracing::warn!(
            "toutes les clés Mistral sont désactivées (mistral_key_status) — skipping rerank"
        );
        return Ok(None);
    }

    let head_len = RERANK_K.min(bundle.fused.len());
    let top_ids: Vec<i64> = bundle.fused[..head_len]
        .iter()
        .map(|(d, _, _)| *d)
        .collect();

    let key = rerank_cache_key(&req.query, &top_ids);
    if let Some(cached) = state.rerank_cache.get(&key).await {
        return Ok(Some(cached));
    }

    // Body-source depuis la metadata déjà hydratée — pas de DB. Un id sans résumé
    // est non couvert : il restera en queue de head ([`rerank_id_order`]), parité
    // de l'ex-`fetch_rerank_payloads` qui ne gardait que les ids à résumé.
    let refs = referential(state).await?;
    let items: Vec<RerankItem> = top_ids
        .iter()
        .filter_map(|d| {
            let m = bundle.meta.get(d)?;
            let summary = m.summary.clone().filter(|s| !s.is_empty())?;
            Some(RerankItem {
                decision_id: *d,
                title: display_title(m, &refs),
                summary,
            })
        })
        .collect();
    if items.len() < 3 {
        return Ok(None);
    }

    // Client par requête ; un span HTTP par appel (TracingMiddleware) pour Tempo.
    let client = std::sync::Arc::new(
        MistralClient::new(live_keys, state.settings.mistral_model.clone())
            .map_err(|e| ApiError::Internal(format!("rerank mistral client: {e}")))?,
    );
    let result = rerank_shortlist(items, &req.query, std::sync::Arc::clone(&client)).await;

    // Persiste les clés mortes découvertes — best-effort, un échec de marquage
    // ne casse pas la recherche.
    let spent = client.spent_fingerprints();
    if !spent.is_empty() {
        match state.pool.get().await {
            Ok(conn) => {
                let repo = DecisionRepository::new(&conn);
                for fp in &spent {
                    if let Err(err) = repo.mark_mistral_key_disabled(fp, 401, "rerank").await {
                        tracing::warn!(fingerprint = %fp, error = %err, "mark_mistral_key_disabled failed");
                    }
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "checkout connexion pour mark_mistral_key_disabled")
            }
        }
    }

    let new_ids = match result {
        Ok(ids) => ids,
        Err(err) => {
            tracing::warn!(error = %err, "rerank_shortlist failed — falling back to retrieval order");
            return Ok(None);
        }
    };

    let order = std::sync::Arc::new(new_ids);
    state.rerank_cache.insert(key, order.clone()).await;
    Ok(Some(order))
}

/// Bundle de recherche mis en cache : récupération (BM25/ANN, **RRF**), facettes
/// et **hydratation COMPLÈTE des hits** (metadata + summary), AVANT rerank, tri
/// par date et pagination. Partagé via `Arc` dans le cache moka de l'[`AppState`] :
/// une même `(query, filtres, mode)` — **ai_mode EXCLU** — réutilise ce calcul
/// coûteux pour les deux modes, toutes les pages et les requêtes répétées. Le
/// rerank LLM (ai_mode) ne touche PLUS ce bundle : il vit hors-cache dans
/// [`reranked_order`] (cache rerank dédié) et n'est qu'un réordonnancement
/// appliqué par-page. La pagination ne fait que trier puis slicer en mémoire ;
/// le `full_text` n'est PAS caché — il est fetché par-page par
/// [`assemble_page`] (≤`limit` lectures TOAST sur miss `page_memo`).
pub struct RankedResults {
    query_mode: QueryMode,
    /// Liste fused en ordre RRF (PRÉ-rerank, PRÉ-tri). Porte le `chunk_id` du
    /// chunk gagnant ANN (fenêtre snippet) + les scores ; le `full_text` est
    /// fetché par-page ([`assemble_page`]).
    fused: Vec<(i64, f64, LegHit)>,
    /// Metadata hydratée de TOUS les hits (≤100), `summary` inclus. Sert au tri
    /// par date ([`sort_fused`]), à l'assemblage de page ([`assemble_page`]) ET
    /// de texte-source au rerank ([`reranked_order`] lit le `summary`, zéro DB).
    /// PAS de highlight ici.
    meta: HashMap<i64, DecisionMeta>,
    facets: SearchFacets,
    /// `decision_id → public_id` de tous les hits. [`paginate`] compose
    /// `all_hit_ids` de la réponse dans l'ordre relevance EFFECTIF (reranké ou
    /// non) — l'ordre n'est plus figé ici puisque le rerank est hors-bundle.
    pub_ids: HashMap<i64, String>,
    total: i64,
    /// Tier 2 — pages déjà highlightées, mémoïsées par `(ai_mode, sort, offset,
    /// limit)`. Le highlight tantivy (~0,2 s/page) ne tourne qu'une fois par page
    /// et par mode ; une page répétée (re-render, pagination retour, retour au
    /// même mode) est servie instantanément. `ai_mode` est dans la clé car le
    /// rerank change l'ordre, donc le contenu de la page. `std::sync::Mutex` :
    /// verrou jamais tenu à travers un `await`.
    page_memo: std::sync::Mutex<HashMap<String, std::sync::Arc<Vec<SearchHit>>>>,
}

impl RankedResults {
    /// Poids approximatif de l'entrée en octets, pour le `weigher` du cache moka
    /// (plafond mémoire, pas par nombre d'entrées). Dominé par la metadata
    /// hydratée (`meta`, summary inclus) ; `fused` ne porte plus les bodies (fetch
    /// par-page), donc son poids est négligeable. Borné à `u32`. Le `page_memo`
    /// (rempli paresseusement, borné aux pages réellement demandées) n'est pas
    /// pesé — moka ne ré-évalue pas le poids après insertion de toute façon.
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
            .pub_ids
            .values()
            .map(|s| std::mem::size_of::<i64>() + std::mem::size_of::<String>() + s.len())
            .sum();
        (fused + meta + ids + facets_weight(&self.facets)).min(u32::MAX as usize) as u32
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
        + opt(&m.procedure_uid)
        + opt(&m.office_uid)
        + opt(&m.legal_domain_uid)
        + vec_s(&m.publication_codes)
        + m.docket_numbers.as_deref().map_or(0, vec_s)
        + opt(&m.summary)
}

/// Octets approximatifs d'un bloc de facettes (libellés + overhead des buckets).
fn facets_weight(f: &SearchFacets) -> usize {
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
    choices(&f.jurisdiction)
        + choices(&f.legal_domain)
        + choices(&f.solution)
        + choices(&f.significance)
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

/// Clé de cache du bundle retrieval : query, filtres, mode — tout ce qui
/// détermine la récupération + RRF + hydratation. EXCLUT `offset`/`limit`
/// (pagination), `sort` (ré-appliqué par page dans [`paginate`]) ET `ai_mode` :
/// retrieval/RRF/facettes/hydratation sont identiques dans les deux modes (le
/// rerank n'est qu'un réordonnancement, mis en cache à part). Une seule entrée
/// sert donc les deux modes, toutes les pages et tous les tris.
fn ranked_cache_key(req: &SearchRequest, query_mode: QueryMode) -> String {
    let mut norm = req.clone();
    norm.offset = 0;
    norm.limit = 0;
    norm.sort = lj_dtos::SortOrder::Relevance;
    norm.ai_mode = false;
    format!(
        "{query_mode:?}|{}",
        serde_json::to_string(&norm).unwrap_or_default()
    )
}

/// Récupération (RRF) + facettes + hydratation → bundle [`RankedResults`] (la
/// partie coûteuse, mise en cache, **ai_mode-indépendante**). Le rerank LLM n'est
/// PLUS fait ici : il est appliqué hors-bundle par [`reranked_order`] / [`paginate`].
async fn rank_results(
    state: &AppState,
    fused: Vec<(i64, f64, LegHit)>,
    query_mode: QueryMode,
) -> Result<RankedResults> {
    let all_ids: Vec<i64> = fused.iter().map(|(d, _, _)| *d).collect();
    let total = fused.len() as i64;

    // `fetch_pub_ids_and_facets` (public_ids + facettes) et `hydrate_all`
    // (metadata des ≤100 hits, SANS highlight) sont indépendants — `hydrate_all`
    // n'utilise ni `pub_ids` ni `facets`. On les lance en parallèle pour épargner
    // un aller-retour DB sur le chemin froid (gain p95 sur DB distante). Pics de
    // connexions : 1 (facettes) + 1 (meta de hydrate_all) = 2 ≤
    // PEAK_CONNS_PER_SEARCH (3) → le sémaphore couvre, pas de hold-and-wait.
    // Le highlight n'est PLUS fait ici (tier 1) : il est par-page dans
    // `assemble_page`/`paginate` (tier 2, mémoïsé). C'est le levier p95 froid :
    // ~20 docs highlightés au lieu de 100.
    let refs = referential(state).await?;
    let facets_conn = client(state).await?;
    let ((pub_ids, facets), meta) = tokio::try_join!(
        async {
            fetch_pub_ids_and_facets(&facets_conn, &all_ids, &refs)
                .await
                .map_err(ApiError::Store)
        },
        hydrate_all(state, &fused),
    )?;

    Ok(RankedResults {
        query_mode,
        fused,
        meta,
        facets,
        pub_ids,
        total,
        page_memo: std::sync::Mutex::new(HashMap::new()),
    })
}

/// Tier 2 : applique l'ordre relevance EFFECTIF (reranké `rerank_ids` si mode IA,
/// sinon RRF du bundle), puis tri + slice de la page (zéro DB) et highlight des
/// ≤ `limit` hits de cette page, mémoïsés par `(ai_mode, sort, offset, limit)`.
/// Première vue d'une page → highlight (~0,2 s) ; vue répétée (même mode) → mémo
/// instantané. Le réseau ne porte que la page. `all_hit_ids` est recomposé dans
/// l'ordre relevance effectif (le rerank n'étant plus figé dans le bundle).
async fn paginate(
    state: &AppState,
    req: &SearchRequest,
    ranked: &RankedResults,
    rerank_ids: Option<&[i64]>,
) -> Result<SearchResponse> {
    // Ordre relevance effectif (ids seuls — pas de clone de bodies). `rerank_ids`
    // n'est `Some` que sous tri relevance (cf. `reranked_order`).
    let relevance_ids: Vec<i64> = match rerank_ids {
        Some(ids) => rerank_id_order(&ranked.fused, ids),
        None => ranked.fused.iter().map(|(d, _, _)| *d).collect(),
    };
    // Posé sur la réponse dans l'ordre relevance (avant re-tri par date), comme
    // l'ex-`_serve_from_cache`. Indépendant de la page → recalculé à chaque appel.
    let all_hit_ids: Vec<String> = relevance_ids
        .iter()
        .filter_map(|d| ranked.pub_ids.get(d).cloned())
        .collect();

    // `ai_mode` dans la clé : l'ordre (donc le contenu de page) diffère entre modes.
    let page_key = format!(
        "{}|{:?}|{}|{}",
        req.ai_mode, req.sort, req.offset, req.limit
    );

    // Verrou court, jamais tenu à travers l'`await` du highlight ci-dessous.
    let cached_page = ranked.page_memo.lock().unwrap().get(&page_key).cloned();
    let page = if let Some(hits) = cached_page {
        (*hits).clone()
    } else {
        // Vue de la liste dans l'ordre relevance effectif, filtrée des ids sans
        // meta (absents du référentiel — jamais en pratique) AVANT pagination :
        // aligne le comptage de page sur l'ex-`build_page_hits`.
        let by_id: HashMap<i64, &(i64, f64, LegHit)> =
            ranked.fused.iter().map(|row| (row.0, row)).collect();
        let mut order: Vec<&(i64, f64, LegHit)> = relevance_ids
            .iter()
            .filter_map(|d| by_id.get(d).copied())
            .filter(|(d, _, _)| ranked.meta.contains_key(d))
            .collect();
        // No-op sous relevance (l'ordre reranké est déjà posé) ; re-trie par date.
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
        let hits = assemble_page(&conn, &slice, &ranked.meta, &req.query, &refs).await?;
        ranked
            .page_memo
            .lock()
            .unwrap()
            .insert(page_key, std::sync::Arc::new(hits.clone()));
        hits
    };

    Ok(SearchResponse {
        query: req.query.clone(),
        total: ranked.total,
        hits: page,
        query_mode: ranked.query_mode,
        facets: Some(ranked.facets.clone()),
        all_hit_ids,
    })
}

/// Mode de recherche effectif (parité du `match` de dispatch) : `Lexical` si la
/// requête est explicitement lexicale, ou détectée booléenne en `auto` ; sinon
/// `Hybrid`.
fn effective_query_mode(req: &SearchRequest) -> QueryMode {
    match req.mode {
        lj_dtos::SearchMode::Lexical => QueryMode::Lexical,
        lj_dtos::SearchMode::Auto if detect_query_mode(&req.query) == QueryMode::Lexical => {
            QueryMode::Lexical
        }
        _ => QueryMode::Hybrid,
    }
}

/// Bundle retrieval depuis le cache moka, sinon calculé (récupération + RRF +
/// facettes + hydratation, **sans rerank**) puis caché (TTL 1 h, aligné sur le
/// cache CDN `/search`). Clé ai_mode-indépendante : les deux modes partagent ce
/// calcul. Pas de single-flight (`ApiError` n'est pas `Clone`) : deux misses
/// concurrents identiques recalculent — acceptable en mono-serveur.
async fn ranked_results_cached(
    state: &AppState,
    req: &SearchRequest,
    query_mode: QueryMode,
    leg_limit: i64,
    vchord_probes: u32,
) -> Result<std::sync::Arc<RankedResults>> {
    let key = ranked_cache_key(req, query_mode);
    if let Some(cached) = state.search_cache.get(&key).await {
        return Ok(cached);
    }
    // Permit DB tenu pendant toute la phase récupération + ranking (cf.
    // `AppState.search_permits`) : borne les recherches concurrentes pour que
    // chacune puisse acquérir son pic de connexions sans hold-and-wait → pas de
    // deadlock du pool, parallélisme intra-recherche (`try_join`) préservé.
    let _db_permit = state
        .search_permits
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| ApiError::Internal(format!("search semaphore: {e}")))?;
    // Requête sans token indexable (vide / 100 % stopwords) : les jambes BM25
    // produiraient une query tantivy sans clause → parse error `paradedb.parse`.
    // Court-circuit sur un bundle vide (`fused` vide ⇒ facettes vides, total 0),
    // caché comme les autres ; aucune jambe (ni embedding ANN) n'est exécutée.
    let fused = if query_lacks_searchable_terms(&req.query) {
        Vec::new()
    } else {
        match query_mode {
            QueryMode::Lexical => retrieve_boolean(state, req, leg_limit).await?,
            QueryMode::Hybrid => retrieve_hybrid(state, req, leg_limit, vchord_probes).await?,
        }
    };
    let ranked = std::sync::Arc::new(rank_results(state, fused, query_mode).await?);
    state.search_cache.insert(key, ranked.clone()).await;
    Ok(ranked)
}

/// Exécute une recherche (lexicale ou hybride selon `mode`) et assemble les
/// hits + facettes. Opérateurs `@@@` (BM25) / `<=>` (cosine) en SQL brut. Le
/// bundle retrieval (récupération + RRF + facettes + metadata hydratée, sans
/// rerank ni highlight) passe par le cache moka, ai_mode-indépendant ; le rerank
/// LLM ([`reranked_order`], cache dédié) n'est qu'un réordonnancement appliqué
/// par-dessus. Chaque appel ne fait plus que trier + slicer puis highlighter la
/// page (≤ `limit` docs, mémoïsée) — zéro DB sur hit, zéro Mistral sur hit rerank.
///
/// Pose `librejustice.search.{source,authenticated,query,mode,context}` sur le
/// span courant AVANT l'exécution, puis `.results_count` APRÈS. `source`
/// (web/mcp) et `authenticated` viennent du handler appelant : on ne met
/// **jamais** l'identité de l'utilisateur dans les traces (RGPD / ADR 0039),
/// seulement un booléen connecté/anonyme — assez pour séparer l'anonyme du
/// connecté côté Tempo. `context` (`user`/`teaser`) distingue les recherches
/// posées par l'utilisateur des fetchs machine des ponts croisés (ADR 0251) —
/// exclure `teaser` de tout comptage d'usage. Ces noms littéraux
/// `librejustice.search.*` survivent au scrub des attributs.
#[tracing::instrument(skip(state, req), fields(
    librejustice.search.source = tracing::field::Empty,
    librejustice.search.query = tracing::field::Empty,
    librejustice.search.mode = tracing::field::Empty,
    librejustice.search.authenticated = tracing::field::Empty,
    librejustice.search.context = tracing::field::Empty,
    librejustice.search.results_count = tracing::field::Empty,
))]
pub async fn search(
    state: &AppState,
    req: &SearchRequest,
    source: lj_dtos::ActivitySource,
    authenticated: bool,
    context: lj_dtos::SearchContext,
) -> Result<SearchResponse> {
    let span = tracing::Span::current();
    span.record(
        "librejustice.search.source",
        crate::search_history::source_value(source),
    );
    span.record("librejustice.search.authenticated", authenticated);
    span.record("librejustice.search.query", req.query.as_str());
    span.record("librejustice.search.mode", tracing::field::debug(req.mode));
    span.record(
        "librejustice.search.context",
        match context {
            lj_dtos::SearchContext::User => "user",
            lj_dtos::SearchContext::Teaser => "teaser",
        },
    );

    let leg_limit = state.settings.leg_limit as i64;
    let vchord_probes = state.settings.vchord_probes;
    let query_mode = effective_query_mode(req);
    let ranked = ranked_results_cached(state, req, query_mode, leg_limit, vchord_probes).await?;
    // Rerank LLM hors-bundle (cache dédié, lit la meta hydratée) : 0 DB, 0 Mistral
    // sur hit. `None` (mode IA off / no-op / échec) ⇒ ordre RRF naturel.
    let rerank_ids = reranked_order(state, req, &ranked).await?;
    let resp = paginate(
        state,
        req,
        &ranked,
        rerank_ids.as_deref().map(Vec::as_slice),
    )
    .await?;

    span.record("librejustice.search.results_count", resp.total);
    Ok(resp)
}
