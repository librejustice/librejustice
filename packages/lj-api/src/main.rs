//! Binaire `librejustice-api` (uvicorn → axum). Port de `apps/api/.../cli.py`.

use anyhow::Result;
use lj_api::{config::Settings, pg_metrics, routes, state::AppState, telemetry};
use lj_store::db;
use std::sync::Arc;

// Runtime mono-thread (ADR 0061) : le rendu SSR Leptos crée des valeurs `!Send`
// (`StoredValue::new_local`, `Rc<navigate>`) dans l'Owner réactif ; sur un runtime
// multi-thread, le drop de l'Owner peut tomber sur un autre worker →
// `SendWrapper::invalid_drop` → panic qui tue le stream Suspense. Mono-thread,
// création et drop sont sur le même thread. Coût throughput ≈ nul : le chemin chaud
// est de l'I/O async (Postgres + embedding HTTP) et tout le CPU lourd (parse XML,
// highlight, exports PDF/DOCX) est offloadé en `spawn_blocking` (pool dédié).
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let settings = Settings::from_env()?;
    // Guard a garder vivant : son `Drop` flush les batch processors OTLP a
    // l'arret (sinon les derniers spans/logs bufferises ne partent pas).
    let _telemetry_guard = telemetry::init_telemetry(&settings)?;

    let pool = db::build_pool(&settings.db_url, settings.pool_max)?;
    let state = AppState::build(Arc::new(settings.clone()), pool);

    // Scraper de métriques Postgres (remplace le `postgresqlreceiver` du
    // collector). Inutile sans export OTLP : on l'aligne sur la condition
    // d'activation de `init_telemetry` (les trois credentials Grafana Cloud
    // présents → MeterProvider installé).
    if settings.grafana_otlp_endpoint.is_some()
        && settings.grafana_otlp_user.is_some()
        && settings.grafana_cloud_api_key.is_some()
    {
        tokio::spawn(pg_metrics::run(state.pool.clone()));
    }

    // Parité `create_app(..., enable_mcp=True)` : l'endpoint MCP est monté par
    // défaut (le wrapper uvicorn Python n'expose pas de flag pour le couper).
    let app = routes::create_app(state, true);
    let addr = format!("{}:{}", settings.bind_host, settings.bind_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("librejustice-api listening on {addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
