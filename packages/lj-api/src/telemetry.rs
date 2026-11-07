use crate::config::Settings;
use anyhow::Result;
use lj_telemetry::{InitOpts, OtlpCreds, TelemetryGuard};
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::EnvFilter;

pub use lj_telemetry::{meter, unattended_request_span};

pub fn init_telemetry(settings: &Settings) -> Result<TelemetryGuard> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"))
        .add_directive(
            "axum_tracing_opentelemetry=error"
                .parse()
                .expect("directive de filtre valide"),
        )
        .add_directive(
            "otel::tracing=trace"
                .parse()
                .expect("directive de filtre valide"),
        );

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
        host_name: settings.host_name.clone(),
        container_name: settings.container_name.clone(),
        otlp_log_filter: Targets::new()
            .with_default(LevelFilter::WARN)
            .with_target(crate::usage::LOG_TARGET, LevelFilter::INFO)
            .with_target(crate::ratelimit::LOG_TARGET, LevelFilter::INFO)
            .with_target(crate::mail::LOG_TARGET, LevelFilter::INFO)
            .with_target(crate::disposable::LOG_TARGET, LevelFilter::INFO)
            .with_target(lj_llm::backend::LOG_TARGET, LevelFilter::INFO),
        otlp,
    })
}
