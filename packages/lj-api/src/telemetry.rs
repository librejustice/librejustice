//! Adaptateur telemetrie du serveur API : construit les [`lj_telemetry::InitOpts`]
//! depuis [`Settings`] (regle #5 : toute config OTel passe par `Settings`) et
//! delegue l'installation a la crate partagee `lj-telemetry` (voir ADR 0062).
//!
//! L'instrumentation des spans naît des `#[instrument]` / `tracing::span!` cote
//! code metier ; les metriques (scraper Postgres) passent par [`meter`].

use crate::config::Settings;
use anyhow::Result;
use lj_telemetry::{InitOpts, OtlpCreds, TelemetryGuard};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::EnvFilter;

pub use lj_telemetry::meter;

/// Initialise la telemetrie de l'API. Renvoie un guard de flush a garder vivant
/// pour toute la duree du process (flush des batch processors a l'arret).
pub fn init_telemetry(settings: &Settings) -> Result<TelemetryGuard> {
    // Filtre : `RUST_LOG` si present, sinon `info`.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let otlp = match (
        settings.grafana_otlp_endpoint.as_ref(),
        settings.grafana_otlp_user.as_ref(),
        settings.grafana_cloud_api_key.as_ref(),
    ) {
        (Some(endpoint), Some(user), Some(api_key)) => Some(OtlpCreds {
            endpoint: endpoint.clone(),
            user: user.clone(),
            api_key: api_key.clone(),
        }),
        _ => None,
    };

    lj_telemetry::init(InitOpts {
        filter,
        json: false,
        service_name: settings.otel_service_name.clone(),
        deployment_environment: settings.deployment_environment.clone(),
        // API : haut volume, on ne ship que WARN+ en logs OTLP.
        otlp_log_level: LevelFilter::WARN,
        otlp,
    })
}
