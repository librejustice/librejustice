//! État partagé axum : pool DB, settings, caches in-process, client Mistral.
//! Les ressources coûteuses sont construites une fois au démarrage ; `AppState`
//! est `Clone`, tous ses champs étant des handles partagés.

use crate::config::Settings;
use crate::embedder::build_query_embedder;
use crate::referential::Referential;
use crate::search::RankedResults;
use crate::typed_cache::TypedCache;
use deadpool_postgres::Pool;
use lj_llm::backend::AnyEmbedder;
use lj_llm::cache::EmbeddingCache;
use std::sync::Arc;
use std::time::Duration;

/// Plafond mémoire du cache résultats, en octets.
const SEARCH_CACHE_MAX_BYTES: u64 = 150 * 1024 * 1024;
/// Memory cap of the texts cache, in bytes.
const TEXTES_CACHE_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Overhead moka par entrée, ajouté au poids pesé.
const ENTRY_OVERHEAD: usize = 128;
/// Memory cap of the browse cache, in bytes. An entry is a total and an exhaustive
/// set of facets, the jurisdiction axis its largest part.
const BROWSE_CACHE_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Plafond du cache rerank, en nombre d'entrées.
const RERANK_CACHE_MAX_ENTRIES: u64 = 50_000;
/// Plafond du cache de notes de rerank : une entrée par (query, document).
const RERANK_SCORES_CACHE_MAX_ENTRIES: u64 = 100_000;

/// Plafond du cache des pages d'annonces, en nombre d'entrées.
const ANNONCES_CACHE_MAX_ENTRIES: u64 = 2_000;

/// Plafond du cache registre, en nombre d'entrées.
const REGISTRE_CACHE_MAX_ENTRIES: u64 = 20_000;

/// Plafond du cache d'identité de texte, en nombre d'entrées.
const TEXT_REF_CACHE_MAX_ENTRIES: u64 = 50_000;

/// Memory cap of the addressed tables of contents, in bytes.
const TOC_ADDRESS_CACHE_MAX_BYTES: u64 = 128 * 1024 * 1024;

/// Peak DB connections one search holds at once, whichever engine: the three
/// legs of `retrieve_hybrid` on decisions, hits and the two facet members on
/// norms. [`AppState::search_permits`] derives from it.
const PEAK_CONNS_PER_SEARCH: usize = 3;

/// État applicatif injecté dans les handlers (`State<AppState>`).
#[derive(Clone)]
pub struct AppState {
    pub settings: Arc<Settings>,
    pub pool: Pool,
    pub embedder: Arc<AnyEmbedder>,
    pub embedding_cache: EmbeddingCache,
    pub search_cache: TypedCache<Arc<RankedResults>>,
    /// Total and facets of browse mode, keyed on the filters alone: the costly
    /// part, and the only one that depends on neither the page nor the sort.
    pub browse_cache: TypedCache<Arc<(i64, lj_dtos::SearchFacets)>>,
    /// Réponses du moteur textes, clé = requête complète, fenêtre comprise. Accès
    /// via `legi::search_textes`.
    pub textes_cache: TypedCache<Arc<lj_dtos::ArticleSearchResponse>>,
    pub rerank_cache: TypedCache<Arc<Vec<i64>>>,
    /// Notes de rerank par `(query, decision_id)`. Elles survivent au changement de
    /// shortlist, là où l'ordre du `rerank_cache` ne survit pas.
    pub rerank_scores_cache: TypedCache<crate::rerank::DocScores>,
    /// `Search` only counts while `settings.search_budget_enforce` is false.
    pub budgets: crate::ratelimit::Budgets,
    /// Plafond journalier d'émission de signalements, qui protège le relais là
    /// où le budget par appelant ne protège que du répéteur.
    pub signalements_du_jour: crate::support::PlafondJournalier,
    /// Transport SMTP des signalements. `None` = relais non configuré,
    /// `/api/signalement` rebondit sur `envoi_impossible`.
    pub mailer: Option<lettre::AsyncSmtpTransport<lettre::Tokio1Executor>>,
    /// Cache mono-entrée du référentiel de facettes. Accès via
    /// `referential::referential`.
    pub referential_cache: TypedCache<Arc<Referential>>,
    /// Cache mono-entrée du catalogue des juridictions. Accès via
    /// `jurisdiction_hubs::catalogue`.
    pub jurisdiction_catalogue_cache: TypedCache<Arc<lj_dtos::JurisdictionCatalogueResponse>>,
    /// Cache mono-entrée du catalogue des normes. Accès via `norm_hubs::catalogue`.
    pub norm_catalogue_cache: TypedCache<Arc<lj_dtos::NormCatalogueResponse>>,
    /// Cache du catalogue des codes, clé = `head_only`. Accès via
    /// `legi::code_catalogue`.
    pub code_catalogue_cache: TypedCache<Arc<lj_dtos::CodeCatalogueResponse>>,
    /// Cache des pages de `/annonces`, clé = filtre sérialisé. Accès via
    /// `annonces::search`.
    pub annonces_cache: TypedCache<Arc<lj_dtos::AnnonceSearchResponse>>,
    /// Cache mono-entrée du catalogue des annonces.
    pub annonces_catalogue_cache: TypedCache<Arc<lj_dtos::AnnonceCatalogueResponse>>,
    /// Borne les recherches qui tapent la DB en même temps à
    /// `pool_max / PEAK_CONNS_PER_SEARCH`, pour que toute recherche admise puisse
    /// acquérir toutes ses connexions. Au-delà, les excédentaires attendent un
    /// permit. Partagé par les deux moteurs, comme le pool.
    pub search_permits: Arc<tokio::sync::Semaphore>,
    /// Cache du volet registre des fiches entité, une entrée par uid. Accès via
    /// `registre::entity_registre`.
    pub registre_cache: TypedCache<Arc<lj_dtos::EntityRegistreResponse>>,
    /// Cache mono-entrée du FST d'autocomplétion. Accès via `routes::suggest_index`.
    pub suggest_cache: TypedCache<Arc<crate::search::suggest::SuggestIndex>>,
    /// Cache de la résolution `slug` → identité de texte. L'entrée `None` est cachée
    /// aussi. Accès via `legi::resolve_text`.
    pub text_ref_cache: TypedCache<Option<Arc<crate::legi::TextRef>>>,
    /// A text's tree at a date with the address of each node, keyed
    /// `<text_uid>@<date>`. Access through `legi::toc_addresses`.
    pub toc_address_cache: TypedCache<Arc<crate::legi::TocAddresses>>,
    /// Cache de la fraîcheur d'ingest par source. Accès via `legi::ingest_freshness`.
    pub ingest_freshness_cache: TypedCache<Option<chrono::NaiveDate>>,
    /// A reader's place resolved to its ancestors; the vocabulary moves only by
    /// migration, so a restart is the only invalidation needed.
    pub place_path_cache: TypedCache<Arc<Vec<String>>>,
    /// What applies in a place at a date, keyed `<place>@<date>`. The
    /// metropolitan answer groups the whole fund; recompiling is what moves it.
    pub place_corpus_cache: TypedCache<Arc<lj_dtos::PlaceCorpusResponse>>,
    /// The corpora a reader placed somewhere searches, keyed by place. The
    /// default reader never reaches it, their answer being a constant.
    pub reader_home_cache: TypedCache<Arc<Vec<String>>>,
    /// The texts a place rules out per jurisdiction it belongs to, keyed
    /// `<place>@<date>`; recompiling is what moves it.
    pub place_texts_cache:
        TypedCache<Arc<std::collections::BTreeMap<String, lj_store::repository::PlaceFilter>>>,
    /// Single-entry cache of the nameable-code catalogue. Read by `legi::named_text`.
    pub named_code_catalog_cache: TypedCache<Arc<Vec<(String, String)>>>,
    /// Named-text resolution, keyed by target. `None` is cached too, an unknown
    /// name costing the same scan as a known one.
    pub named_text_cache: TypedCache<Option<Arc<str>>>,
    /// Client HTTP partagé des appels registre.
    pub registre_http: reqwest::Client,
    /// `None` without the INPI account or the bucket.
    pub inpi: Option<Arc<crate::inpi::Inpi>>,
    /// Client Mistral du rerank listwise, tenu sur le pool de la base.
    pub mistral: Arc<MistralPool>,
    pub pseudonymiser: Option<lj_identity::Pseudonymiser>,
    /// Signs the OIDC `id_token` and publishes its JWKS. `None` when
    /// `oidc_signing_key` is absent or unusable: no token is issued then.
    pub oidc_signer: Option<Arc<crate::oidc_keys::OidcSigner>>,
}

impl AppState {
    /// Construit l'état applicatif au démarrage.
    pub fn build(settings: Arc<Settings>, pool: Pool) -> Self {
        let embedder = Arc::new(build_query_embedder(&settings));
        let settings_search_budget = settings.search_budget_per_min;
        let settings_rerank_budget = settings.rerank_budget_per_min;
        let settings_signalements_par_jour = settings.signalements_par_jour;
        let mailer = crate::support::transport(&settings);
        let search_cache = TypedCache::builder(SEARCH_CACHE_MAX_BYTES)
            .weigher(|k: &str, v: &Arc<RankedResults>| {
                (k.len() + v.approx_weight() as usize + ENTRY_OVERHEAD).min(u32::MAX as usize)
                    as u32
            })
            .time_to_live(Duration::from_secs(3600))
            .build();
        let browse_cache = TypedCache::builder(BROWSE_CACHE_MAX_BYTES)
            .weigher(|k: &str, v: &Arc<(i64, lj_dtos::SearchFacets)>| {
                (k.len() + crate::search::facets_weight(&v.1) + ENTRY_OVERHEAD)
                    .min(u32::MAX as usize) as u32
            })
            .time_to_live(Duration::from_secs(3600))
            .build();
        let textes_cache = TypedCache::builder(TEXTES_CACHE_MAX_BYTES)
            .weigher(|k: &str, v: &Arc<lj_dtos::ArticleSearchResponse>| {
                (k.len() + crate::legi::response_weight(v) + ENTRY_OVERHEAD).min(u32::MAX as usize)
                    as u32
            })
            .time_to_live(Duration::from_secs(3600))
            .build();
        let rerank_cache = TypedCache::builder(RERANK_CACHE_MAX_ENTRIES)
            .time_to_live(Duration::from_secs(3600))
            .build();
        let rerank_scores_cache = TypedCache::builder(RERANK_SCORES_CACHE_MAX_ENTRIES)
            .time_to_live(Duration::from_secs(24 * 3600))
            .build();
        let referential_cache = TypedCache::builder(1)
            .time_to_live(Duration::from_secs(3600))
            .build();
        let jurisdiction_catalogue_cache = TypedCache::builder(1)
            .time_to_live(Duration::from_secs(12 * 3600))
            .build();
        let norm_catalogue_cache = TypedCache::builder(1)
            .time_to_live(Duration::from_secs(12 * 3600))
            .build();
        // Deux entrées : le catalogue de tête (`/norm-index/{juris}/codes`) et le catalogue
        // complet (`/api/codes`).
        let code_catalogue_cache = TypedCache::builder(2)
            .time_to_live(Duration::from_secs(12 * 3600))
            .build();
        let annonces_cache = TypedCache::builder(ANNONCES_CACHE_MAX_ENTRIES)
            .time_to_live(Duration::from_secs(15 * 60))
            .build();
        let annonces_catalogue_cache = TypedCache::builder(1)
            .time_to_live(Duration::from_secs(3600))
            .build();
        let search_permits = Arc::new(tokio::sync::Semaphore::new(
            (settings.pool_max / PEAK_CONNS_PER_SEARCH).max(1),
        ));
        // Volet registre : payloads de quelques Ko, plafond par nombre
        // d'entrées. TTL 24 h — chronologies et dirigeants bougent au rythme
        // des parutions, pas des requêtes.
        let registre_cache = TypedCache::builder(REGISTRE_CACHE_MAX_ENTRIES)
            .time_to_live(Duration::from_secs(24 * 3600))
            .build();
        // The autocomplete FST: one entry of tens of MB, reloaded from the DB on
        // expiry, its offline rebuild being at most daily.
        let suggest_cache = TypedCache::builder(1)
            .time_to_live(Duration::from_secs(24 * 3600))
            .build();
        // Identité de texte par slug : plafond par nombre d'entrées (payload
        // minuscule), TTL 1 h comme le référentiel de facettes — un texte
        // nouvellement ingéré devient adressable dans l'heure.
        let text_ref_cache = TypedCache::builder(TEXT_REF_CACHE_MAX_ENTRIES)
            .time_to_live(Duration::from_secs(3600))
            .build();
        let toc_address_cache = TypedCache::builder(TOC_ADDRESS_CACHE_MAX_BYTES)
            .weigher(|k: &str, v: &Arc<crate::legi::TocAddresses>| {
                (k.len() + v.weight() + ENTRY_OVERHEAD).min(u32::MAX as usize) as u32
            })
            .time_to_live(Duration::from_secs(3600))
            .build();
        // Fraîcheur d'ingest : une entrée par source (poignée de valeurs),
        // rafraîchie dans l'heure suivant le cron quotidien.
        let ingest_freshness_cache = TypedCache::builder(64)
            .time_to_live(Duration::from_secs(3600))
            .build();
        let place_path_cache = TypedCache::builder(4_096).build();
        let place_corpus_cache = TypedCache::builder(256)
            .time_to_live(Duration::from_secs(6 * 3600))
            .build();
        let reader_home_cache = TypedCache::builder(256).build();
        let place_texts_cache = TypedCache::builder(256)
            .time_to_live(Duration::from_secs(6 * 3600))
            .build();
        // Both read the whole of `legal_text`, which only ingest moves.
        let named_code_catalog_cache = TypedCache::builder(1)
            .time_to_live(Duration::from_secs(12 * 3600))
            .build();
        let named_text_cache = TypedCache::builder(512)
            .time_to_live(Duration::from_secs(12 * 3600))
            .build();
        let registre_http = reqwest::Client::builder()
            .timeout(crate::registre::UPSTREAM_TIMEOUT)
            // hyper divise ce plafond par le nombre d'adresses et les essaie en
            // série : sans lui, une IP morte mange le budget entier et la seconde
            // n'est jamais tentée — DILA en publie deux.
            .connect_timeout(Duration::from_secs(1))
            .user_agent("librejustice.fr")
            .build()
            .expect("client HTTP registre");
        let inpi = crate::inpi::Inpi::from_settings(&settings).map(Arc::new);
        let mistral = Arc::new(MistralPool::default());
        let pseudonymiser =
            lj_identity::Pseudonymiser::new(settings.usage_pseudonym_key.as_deref());
        let oidc_signer =
            crate::oidc_keys::OidcSigner::new(settings.oidc_signing_key.as_deref()).map(Arc::new);
        Self {
            settings,
            pool,
            embedder,
            embedding_cache: EmbeddingCache::default(),
            search_cache,
            browse_cache,
            textes_cache,
            rerank_cache,
            rerank_scores_cache,
            budgets: crate::ratelimit::Budgets::new(settings_search_budget, settings_rerank_budget),
            signalements_du_jour: crate::support::PlafondJournalier::new(
                settings_signalements_par_jour,
            ),
            mailer,
            referential_cache,
            jurisdiction_catalogue_cache,
            norm_catalogue_cache,
            code_catalogue_cache,
            annonces_cache,
            annonces_catalogue_cache,
            search_permits,
            registre_cache,
            registre_http,
            inpi,
            suggest_cache,
            text_ref_cache,
            toc_address_cache,
            ingest_freshness_cache,
            place_path_cache,
            place_corpus_cache,
            reader_home_cache,
            place_texts_cache,
            named_code_catalog_cache,
            named_text_cache,
            mistral,
            pseudonymiser,
            oidc_signer,
        }
    }
}

const RERANK_POOL: &str = "mistral";

/// Mistral client held on the live `llm_credential` pool, rebuilt when that pool
/// moves. Parked keys are already out of what the store returns, so a spent key
/// leaves the client and a freshly poured one joins it without a restart.
#[derive(Default)]
pub struct MistralPool {
    held: std::sync::Mutex<Option<(Vec<String>, Arc<lj_llm::mistral::MistralClient>)>>,
}

impl MistralPool {
    /// The up-to-date client, `None` when the pool is empty or unreadable.
    pub async fn client(
        &self,
        repo: &lj_store::repository::DecisionRepository<'_>,
    ) -> Option<Arc<lj_llm::mistral::MistralClient>> {
        let live = match repo.pool_fingerprints(RERANK_POOL).await {
            Ok(live) => live,
            Err(err) => {
                tracing::warn!(error = %err, "reading the Mistral pool");
                return None;
            }
        };
        if live.is_empty() {
            tracing::warn!("mistral pool empty, rerank dropped");
            return None;
        }
        if let Some(client) = self.current(&live) {
            return Some(client);
        }
        let keys = match repo.pool_api_keys(RERANK_POOL).await {
            Ok(keys) => keys,
            Err(err) => {
                tracing::warn!(error = %err, "reading the Mistral pool keys");
                return None;
            }
        };
        let mut built: Vec<String> = keys
            .iter()
            .map(|k| lj_llm::mistral::key_fingerprint(k))
            .collect();
        built.sort();
        let client =
            lj_llm::mistral::MistralClient::new(keys, crate::rerank::RERANK_MODEL.to_string())
                .map(Arc::new)
                .inspect_err(|err| tracing::warn!(error = %err, "Mistral client, rerank dropped"))
                .ok()?;
        tracing::info!(cles = built.len(), "rerank Mistral pool rebuilt");
        *self.held.lock().expect("mutex pool Mistral non empoisonné") =
            Some((built, Arc::clone(&client)));
        Some(client)
    }

    fn current(&self, live: &[String]) -> Option<Arc<lj_llm::mistral::MistralClient>> {
        let held = self.held.lock().expect("mutex pool Mistral non empoisonné");
        let (seen, client) = held.as_ref()?;
        (seen == live).then(|| Arc::clone(client))
    }
}
