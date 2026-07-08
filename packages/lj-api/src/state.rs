//! État partagé axum (pool DB, settings, client Mistral du lazy-fill summary).
//!
//! Parité avec le lifespan FastAPI (`main.create_app`) : les ressources
//! coûteuses (pool psycopg, client Mistral) sont construites **une fois** au
//! démarrage et partagées pour toute la durée du process — jamais ré-init par
//! requête. `AppState` est `Clone` (tous les champs sont `Arc`/`Pool`, donc le
//! clone est un partage de handles, pas une copie profonde).

use crate::config::Settings;
use crate::embedder::build_query_embedder;
use crate::referential::Referential;
use crate::search::RankedResults;
use deadpool_postgres::Pool;
use lj_dtos::CorpusStatsResponse;
use lj_llm::backend::AnyEmbedder;
use lj_llm::cache::EmbeddingCache;
use moka::future::Cache;
use std::sync::Arc;
use std::time::Duration;

/// Plafond **mémoire** du cache résultats (octets) : 150 Mio des 200 Mio totaux
/// du process (l'embedding cache prend les 50 Mio restants, cf. `lj-llm`).
const SEARCH_CACHE_MAX_BYTES: u64 = 150 * 1024 * 1024;
/// Overhead moka par entrée (nœud de la map + bookkeeping TinyLFU), ajouté au
/// poids pesé pour que le plafond couvre la mémoire réelle, pas que le payload.
const ENTRY_OVERHEAD: usize = 128;
/// Plafond du cache rerank en NOMBRE d'entrées (chaque ordre LLM ≤ 50 `i64`,
/// ~0,4 Kio + clé) : 50 000 entrées ≈ quelques dizaines de Mio, négligeable
/// devant les 150 Mio du `search_cache`. L'éviction primaire reste le TTL 1 h.
const RERANK_CACHE_MAX_ENTRIES: u64 = 50_000;

/// Pic de connexions DB qu'une recherche tient simultanément :
/// `retrieve_hybrid` garde 3 conns (bm25 + ann + title, en `try_join` parallèle)
/// puis les relâche AVANT `compute_adaptive_weights`, qui en reprend 2 en
/// parallèle (embeddings ANN + repr décision). `rank_results` en tient 2
/// (facettes + meta de `hydrate_all`), `paginate` 1. Pic = 3 (les jambes),
/// irréductible sans sérialiser le `try_join`. Sémaphore = pool_max / 3
/// (32 → 10) ; 10 × 3 = 30 ≤ pool, 2 conns de marge.
const PEAK_CONNS_PER_SEARCH: usize = 3;

/// État applicatif injecté dans les handlers (`State<AppState>`).
///
/// Champs portés depuis `app.state` (Python) :
/// - `settings` / `pool` : configuration + pool DB partagés ;
/// - `embedder` : embedder de requête construit **une fois** au démarrage et
///   partagé (parité lifespan Python). Crucial pour le mode `auto`, dont l'état
///   `BackendHealth` (EMA vLLM↔Cloudflare) doit persister entre requêtes ;
/// - `embedding_cache` : cache in-process des vecteurs de requête (`moka`, TTL
///   7 j), parité du cache embeddings Redis Python ;
/// - `search_cache` : cache in-process du bundle retrieval par recherche (`moka`,
///   TTL 1 h, aligné sur le cache CDN `/search`). Évite de rejouer récupération +
///   RRF + facettes + hydratation sur les pages 2+, les requêtes répétées ET les
///   deux modes IA (clé ai_mode-indépendante) ; la clé exclut aussi
///   `offset`/`limit`/`sort` (cf. `ranked_cache_key`) ;
/// - `rerank_cache` : cache in-process de l'ordre LLM du top-50 (`moka`, TTL 1 h),
///   keyé `(query, top-ids)`. Économise le quota Mistral : un toggle IA on↔off (ou
///   un bundle reconstruit au même shortlist) sert l'ordre caché sans rappeler le
///   reranker (cf. `search::reranked_order`).
#[derive(Clone)]
pub struct AppState {
    pub settings: Arc<Settings>,
    pub pool: Pool,
    pub embedder: Arc<AnyEmbedder>,
    pub embedding_cache: EmbeddingCache,
    pub search_cache: Cache<String, Arc<RankedResults>>,
    pub rerank_cache: Cache<String, Arc<Vec<i64>>>,
    /// Cache mono-entrée du référentiel de facettes (`facet_value` +
    /// `jurisdiction`, ADR 0146), TTL 1 h : labels résolus en mémoire, la DB
    /// n'est relue qu'à l'expiration. Accès via `referential::referential`.
    pub referential_cache: Cache<(), Arc<Referential>>,
    /// Cache mono-entrée des compteurs corpus de la page d'accueil
    /// (`corpus-stats`), TTL 12 h : les chiffres (comptes exacts) ne bougent qu'à
    /// l'ingest quotidien, donc un recalcul 2×/jour suffit. Accès via
    /// `stats::corpus_stats`.
    pub corpus_stats_cache: Cache<(), Arc<CorpusStatsResponse>>,
    /// Borne le nombre de recherches qui tapent la DB en même temps à
    /// `pool_max / PEAK_CONNS_PER_SEARCH`. Garantit que toute recherche admise
    /// peut acquérir TOUTES ses connexions (admises × pic ≤ pool) : le
    /// hold-and-wait ne peut plus former de cycle → pas de deadlock du pool, tout
    /// en gardant le `try_join` parallèle intra-recherche. Au-delà, les recherches
    /// excédentaires attendent un permit (backpressure) au lieu de figer le pool.
    pub search_permits: Arc<tokio::sync::Semaphore>,
}

impl AppState {
    /// Construit l'état applicatif au démarrage (parité lifespan FastAPI).
    pub fn build(settings: Arc<Settings>, pool: Pool) -> Self {
        // Embedder construit une fois (parité lifespan Python). `panic!` au boot
        // sur config invalide (creds manquantes) — voir `build_query_embedder`.
        let embedder = Arc::new(build_query_embedder(&settings));
        // Cache résultats : plafond MÉMOIRE (octets), pas un nombre d'entrées. Le
        // `weigher` pèse chaque liste rankée (clé + payload `approx_weight` +
        // overhead moka) ; moka évince (LRU/TTL) pour rester sous 150 Mio. TTL
        // 1 h (aligné sur le cache CDN `/search`) : le corpus ne bouge qu'une
        // fois/jour (ingest cron 3 h), donc 1 h ne sert jamais de périmé ; la
        // pression mémoire reste l'éviction primaire.
        let search_cache = Cache::builder()
            .weigher(|k: &String, v: &Arc<RankedResults>| {
                (k.len() + v.approx_weight() as usize + ENTRY_OVERHEAD).min(u32::MAX as usize)
                    as u32
            })
            .max_capacity(SEARCH_CACHE_MAX_BYTES)
            .time_to_live(Duration::from_secs(3600))
            .build();
        // Cache rerank : l'ordre LLM est minuscule (≤50 `i64`) → plafond par
        // NOMBRE d'entrées, pas en octets. TTL 1 h (aligné search_cache) : sur la
        // fenêtre, un toggle IA on↔off ne consomme le quota Mistral qu'une fois.
        let rerank_cache = Cache::builder()
            .max_capacity(RERANK_CACHE_MAX_ENTRIES)
            .time_to_live(Duration::from_secs(3600))
            .build();
        // Référentiel de facettes : une seule entrée (clé `()`), rechargée de la
        // DB toutes les heures. Quelques centaines de lignes → poids négligeable.
        let referential_cache = Cache::builder()
            .max_capacity(1)
            .time_to_live(Duration::from_secs(3600))
            .build();
        // Compteurs corpus (page d'accueil) : une seule entrée (clé `()`), TTL 12 h.
        // Le corpus ne grossit qu'à l'ingest quotidien → un recalcul (comptes exacts
        // décisions + catalogue codes) deux fois/jour suffit, jamais par requête.
        let corpus_stats_cache = Cache::builder()
            .max_capacity(1)
            .time_to_live(Duration::from_secs(12 * 3600))
            .build();
        // Au moins 1 permit même si pool_max < PEAK (config exotique) : la
        // recherche reste possible, juste sérialisée.
        let search_permits = Arc::new(tokio::sync::Semaphore::new(
            (settings.pool_max / PEAK_CONNS_PER_SEARCH).max(1),
        ));
        Self {
            settings,
            pool,
            embedder,
            embedding_cache: EmbeddingCache::default(),
            search_cache,
            rerank_cache,
            referential_cache,
            corpus_stats_cache,
            search_permits,
        }
    }
}
