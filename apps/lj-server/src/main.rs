//! Binaire de déploiement unique (ADR 0061) : API REST + MCP/OAuth + SSR Leptos +
//! assets statiques + TLS d'origine, dans un seul process axum partageant un
//! `AppState` construit **une fois**. Remplace les services `api` + `ssr` +
//! `caddy` (Caddy supprimé : TLS terminé in-process avec le cert Cloudflare
//! Origin CA statique).
//!
//! Frontière de données (ADR 0061 §2) : le rendu SSR appelle la couche service
//! `lj-api` **in-process** (l'`AppState` est fourni au contexte Leptos par
//! `leptos_routes_with_context`, lu par `lj_web::api::ApiClient`) ; le navigateur
//! tape `/api` en HTTP same-origin. Aucun hop HTTP-vers-soi sur le chemin chaud.

use anyhow::Result;
use axum::Router;
use leptos::prelude::*;
use leptos_axum::{generate_route_list, LeptosRoutes};
use lj_api::{config::Settings, pg_metrics, state::AppState, telemetry};
use lj_store::db;
use lj_web::app::{shell, App};
use std::sync::Arc;
use tower_http::compression::CompressionLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeader;

// Runtime **mono-thread** : le SSR Leptos crée des valeurs `_local`
// (`StoredValue::new_local` autour de closures JS / `Rc<navigate>` !Send dans
// TopBar/Pagination/FilterRail/Toc) stockées dans l'Owner réactif. Sur un runtime
// multi-thread le nettoyage de l'Owner peut tomber sur un autre worker →
// `SendWrapper::invalid_drop` → panic qui tue le stream Suspense. Création et drop
// ici sur le même thread. Le CPU lourd (highlight snippet tantivy) est offloadé
// via `spawn_blocking` (pool dédié) pour ne pas figer l'unique thread async ; le
// reste du hot-path est de l'I/O (`await` Postgres / embedding HTTP).
//
// Ne PAS repasser en multi-thread : un `LocalSet::run_until(run())` ne sauve rien
// — `axum::serve` (et `axum_server` côté TLS) spawnent chaque connexion via
// `tokio::spawn`, donc HORS du `LocalSet` ; le render SSR tourne sur les workers,
// se fait voler à un `await`, et droppe l'Owner cross-thread → panic. Mesuré :
// multi-thread + `LocalSet` meurt sous 60 requêtes concurrentes sur `/decision/{id}`
// (31 panics `SendWrapper`), là où `current_thread` encaisse 204 renders SSR
// concurrents sans une erreur. La seule voie multi-cœur serait un front Send-clean
// (`Arc` au lieu de `Rc`, plus de `_local`), pas un changement de runtime.
//
// Pool blocking **borné** : le défaut tokio (512 threads) explose sur la cible
// prod (ARM 2–4 cœurs) sous un burst de CPU offloadé (parse, rendu PDF/DOCX,
// highlight) → thrash + stacks OS. On le plafonne aux cœurs réels, borné à 4 ;
// `available_parallelism()` lit la cible (2–4 en prod, plafonné ici en dev). Le
// macro `#[tokio::main]` ne règle pas `max_blocking_threads` → builder manuel.
// `block_on` tourne sur le thread courant → même thread create+drop pour les
// `_local` Leptos (garantie identique au macro `current_thread`).
fn main() -> Result<()> {
    let blocking_cap = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(4);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(blocking_cap)
        .build()?
        .block_on(run())
}

async fn run() -> Result<()> {
    let settings = Settings::from_env()?;
    // Guard à garder vivant : son `Drop` flush les batch processors OTLP.
    let _telemetry_guard = telemetry::init_telemetry(&settings)?;

    let pool = db::build_pool(&settings.db_url, settings.pool_max)?;
    let state = AppState::build(Arc::new(settings.clone()), pool);

    // Scraper métriques Postgres : actif uniquement si l'export OTLP l'est (les
    // trois credentials Grafana Cloud présents), aligné sur `init_telemetry`.
    if settings.grafana_otlp_endpoint.is_some()
        && settings.grafana_otlp_user.is_some()
        && settings.grafana_cloud_api_key.is_some()
    {
        tokio::spawn(pg_metrics::run(state.pool.clone()));
    }

    // Config cargo-leptos (env `LEPTOS_*`) → options SSR (site_addr, site_root…).
    let conf = get_configuration(None).unwrap();
    let leptos_options = conf.leptos_options;
    let leptos_addr = leptos_options.site_addr;
    let routes = generate_route_list(App);

    // Service statique du bundle `/pkg` (wasm/js/css) servant les variantes
    // **précompressées** `.br`/`.gz` quand le client les accepte (générées au
    // build, cf. Dockerfile `precompress`). Le wasm release fait ~3,8 Mo brut,
    // ~1 Mo brotli : sans ça, chaque 1ʳᵉ visite télécharge 3,8 Mo — d'autant plus
    // sensible que `/recherche` est CSR (le 1ᵉʳ paint attend le bundle, ADR 0063).
    // Servir le `.br` pré-généré = zéro recompression par requête sur le nœud ARM.
    let pkg_dir = std::path::Path::new(&leptos_options.site_root.to_string())
        .join(leptos_options.site_pkg_dir.to_string());
    // Les noms de `/pkg` sont content-hashés (`hash-files = true`, ADR 0155) :
    // un fichier donné ne change jamais de contenu → cachable un an `immutable`,
    // et le HTML SSR (via `hash.txt` lu à côté du binaire) référence toujours le
    // bundle exact de son build — un wasm périmé ne peut plus hydrater un HTML
    // neuf (panic `failed_to_cast_element`, diagnostiqué tâche #34).
    let pkg_service = SetResponseHeader::overriding(
        ServeDir::new(pkg_dir)
            .precompressed_br()
            .precompressed_gzip(),
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("public, max-age=31536000, immutable"),
    );

    // Fonts immuables : un `.woff2` n'est jamais réécrit en place (changer de
    // police = nouveau fichier) → cachable un an `immutable` sans hash. Sans ce
    // header l'origine est muette et Cloudflare retombe sur son TTL navigateur
    // par défaut (4 h), refaisant télécharger ~44 Ko sur le chemin de rendu aux
    // visites répétées.
    let fonts_dir = std::path::Path::new(&leptos_options.site_root.to_string()).join("fonts");
    let fonts_service = SetResponseHeader::overriding(
        ServeDir::new(fonts_dir),
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("public, max-age=31536000, immutable"),
    );

    // Routeur SSR Leptos : l'`AppState` est fourni au contexte de chaque requête
    // (le client SSR `lj-web` l'y lit pour appeler la couche service in-process).
    let leptos_router = Router::new()
        .leptos_routes_with_context(
            &leptos_options,
            routes,
            {
                let state = state.clone();
                move || provide_context(state.clone())
            },
            {
                let leptos_options = leptos_options.clone();
                move || shell(leptos_options.clone())
            },
        )
        .fallback(leptos_axum::file_and_error_handler(shell))
        .with_state(leptos_options);

    // Fusion : routes données/MCP/OAuth (`lj-api`, sans fallback — c'est le
    // file-handler Leptos qui catch-all) + redirection `/mcp` → `/mcp/` (parité
    // du slash-redirect, préservée explicitement, ADR §5).
    // `CompressionLayer` (br + gzip, négociés via `Accept-Encoding`) pour les
    // réponses dynamiques (HTML SSR, JSON API) : compression à la volée bon marché
    // (≤ 65 Ko). Elle saute les réponses déjà encodées — donc le `/pkg`
    // précompressé ci-dessus n'est jamais recompressé (pas de double travail CPU).
    let app = leptos_router
        .merge(lj_api::routes::build_router(state.clone(), true))
        .route(
            "/mcp",
            axum::routing::get(|| async { axum::response::Redirect::temporary("/mcp/") }),
        )
        // Anciennes routes de la recherche (ADR 0114) → pages actuelles
        // (`/recherche` décisions, `/textes` lois et règlements), query
        // préservée. Redirects 308 côté axum (un alias est une redirection
        // HTTP, pas une page rendue) ; `lj-web` n'a plus ces routes. Côté
        // textes, le filtre provenance a changé de clé (`source` → `origine`).
        .route(
            "/recherche-decisions",
            axum::routing::get(|raw: axum::extract::RawQuery| async move {
                let target = match raw.0.filter(|q| !q.is_empty()) {
                    Some(q) => format!("/recherche?{q}"),
                    None => "/recherche".to_string(),
                };
                axum::response::Redirect::permanent(&target)
            }),
        )
        .route(
            "/recherche-textes",
            axum::routing::get(|raw: axum::extract::RawQuery| async move {
                let parts: Vec<String> = raw
                    .0
                    .as_deref()
                    .unwrap_or("")
                    .split('&')
                    .filter(|p| !p.is_empty())
                    .map(|p| match p.strip_prefix("source=") {
                        Some(v) => format!("origine={v}"),
                        None => p.to_string(),
                    })
                    .collect();
                let target = if parts.is_empty() {
                    "/textes".to_string()
                } else {
                    format!("/textes?{}", parts.join("&"))
                };
                axum::response::Redirect::permanent(&target)
            }),
        )
        .nest_service("/pkg", pkg_service)
        .nest_service("/fonts", fonts_service)
        .layer(CompressionLayer::new());

    // TLS in-process si le cert/clé Origin CA est configuré (prod, ADR §3) ;
    // sinon HTTP clair sur l'adresse cargo-leptos (dev).
    match (
        settings.tls_cert_path.as_deref(),
        settings.tls_key_path.as_deref(),
    ) {
        (Some(cert), Some(key)) => {
            let addr: std::net::SocketAddr =
                format!("{}:{}", settings.bind_host, settings.bind_port).parse()?;
            let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
            tracing::info!("lj-server (TLS) écoute sur {addr}");
            axum_server::bind_rustls(addr, tls)
                .serve(app.into_make_service())
                .await?;
        }
        _ => {
            let listener = tokio::net::TcpListener::bind(&leptos_addr).await?;
            tracing::info!("lj-server (HTTP) écoute sur {leptos_addr}");
            axum::serve(listener, app.into_make_service()).await?;
        }
    }
    Ok(())
}
