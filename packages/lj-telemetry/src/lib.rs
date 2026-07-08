//! Telemetrie partagee : subscriber tracing + export DIRECT vers la gateway
//! OTLP Grafana Cloud (traces + logs + metriques), HTTP/protobuf + Basic Auth.
//!
//! Crate consommee par les binaires `lj-api` (serveur axum, long-vivant) et
//! `lj-ingest` (cron, court-vivant). Plus de collector local : c'etait lui qui
//! scrubait la PII et faisait le fan-out vers les backends. Les deux roles sont
//! rapatries cote app :
//!
//! - export OTLP/HTTP des 3 signaux vers un endpoint unique (`/otlp`), avec
//!   l'en-tete `Authorization: Basic base64(<otlp_user>:<api_key>)` ;
//! - scrub PII des spans avant export via un [`ScrubbingSpanProcessor`]
//!   (RGPD / ADR 0039) : suppression IP / `db.statement`, troncature des
//!   query-strings d'URL ; `librejustice.search.query` est conserve.
//!
//! L'activation OTel = presence des credentials Grafana ([`InitOpts::otlp`]).
//! Sans eux, on reste sur un simple subscriber fmt (dev local).
//!
//! Court-vivant (cron) : [`init`] renvoie un [`TelemetryGuard`] dont le `Drop`
//! flush les batch processors. Sans ce flush avant exit, les spans/logs
//! bufferises ne partiraient jamais.

use anyhow::Result;
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{global, KeyValue, Value};
use opentelemetry_otlp::{
    LogExporter, MetricExporter, SpanExporter, WithExportConfig, WithHttpConfig,
};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::{
    BatchSpanProcessor, SdkTracerProvider, Span, SpanData, SpanProcessor,
};
use opentelemetry_sdk::Resource;
use std::collections::HashMap;
use std::time::Duration;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Nom de scope OTel commun (tracer + meter). Les instruments (ex. scraper
/// Postgres cote `lj-api`) passent par [`meter`].
const SCOPE: &str = "librejustice";

/// Credentials de la gateway OTLP Grafana Cloud. Presents = export actif.
pub struct OtlpCreds {
    /// Base URL de la gateway (ex. `https://otlp-gateway-...grafana.net/otlp`).
    /// On y ajoute soi-même `/v1/{traces,logs,metrics}` par signal (cf.
    /// [`OtlpExport::signal_endpoint`]) : `with_endpoint` sur un builder
    /// par-signal prend l'URL telle quelle (seule la variable *générale*
    /// `OTEL_EXPORTER_OTLP_ENDPOINT` déclenche l'append automatique du SDK).
    pub endpoint: String,
    /// Utilisateur OTLP (numero d'instance Grafana Cloud).
    pub user: String,
    /// Cle d'API Grafana Cloud.
    pub api_key: String,
}

/// Parametres d'initialisation de la telemetrie.
pub struct InitOpts {
    /// Filtre de niveaux du subscriber (chaque binaire le construit a sa facon :
    /// `RUST_LOG` cote api, `LIBREJUSTICE_LOG_LEVEL` cote ingest).
    pub filter: EnvFilter,
    /// Format du layer fmt stdout : `true` = JSON, `false` = texte.
    pub json: bool,
    /// `service.name` du `Resource` (ex. `librejustice-api`, `librejustice-ingest`).
    pub service_name: String,
    /// `deployment.environment` du `Resource` (ex. `prod`), si pose.
    pub deployment_environment: Option<String>,
    /// Niveau plancher du bridge logs -> LogRecords OTLP (Loki). L'API reste
    /// sur `WARN` (volume), l'ingest passe a `INFO` pour exporter les
    /// breadcrumbs de cycle de vie (debut/fin de phase) : un « phase debut »
    /// sans « phase fin » localise un hang invisible autrement.
    pub otlp_log_level: LevelFilter,
    /// Credentials OTLP. `None` = subscriber fmt seul (pas d'export).
    pub otlp: Option<OtlpCreds>,
}

/// Garde de vie de la telemetrie : flush des batch processors a la destruction.
///
/// Indispensable pour les process courts (cron `lj-ingest`) : sans flush avant
/// exit, les spans/logs bufferises ne sont jamais exportes. A lier dans `main`
/// (`let _guard = lj_telemetry::init(...)?;`).
#[must_use = "garder le guard vivant jusqu'a la fin du process, sinon les signaux bufferises ne partent pas"]
#[derive(Default)]
pub struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
    logger_provider: Option<SdkLoggerProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(p) = &self.tracer_provider {
            let _ = p.shutdown();
        }
        if let Some(p) = &self.meter_provider {
            let _ = p.shutdown();
        }
        if let Some(p) = &self.logger_provider {
            let _ = p.shutdown();
        }
    }
}

/// Config OTLP resolue (endpoint normalise + en-tete d'auth + resource).
struct OtlpExport {
    endpoint: String,
    auth: String,
    resource: Resource,
}

impl OtlpExport {
    fn new(creds: OtlpCreds, service_name: &str, deployment_environment: Option<&str>) -> Self {
        let token = STANDARD.encode(format!("{}:{}", creds.user, creds.api_key));
        let mut attrs = vec![KeyValue::new("host.name", hostname())];
        if let Some(env) = deployment_environment {
            attrs.push(KeyValue::new("deployment.environment", env.to_string()));
        }
        let resource = Resource::builder()
            .with_service_name(service_name.to_string())
            .with_attributes(attrs)
            .build();
        Self {
            endpoint: creds.endpoint.trim_end_matches('/').to_string(),
            auth: format!("Basic {token}"),
            resource,
        }
    }

    fn headers(&self) -> HashMap<String, String> {
        HashMap::from([("Authorization".to_string(), self.auth.clone())])
    }

    /// URL complète d'un signal : `<endpoint>/v1/<signal>` (ex.
    /// `.../otlp/v1/traces`). Indispensable car `with_endpoint` par-signal est
    /// utilisé verbatim — sans ce suffixe la gateway répond 404.
    fn signal_endpoint(&self, signal: &str) -> String {
        format!("{}/v1/{signal}", self.endpoint)
    }
}

/// Installe le subscriber tracing global et, si [`InitOpts::otlp`] est fourni,
/// branche l'export OTLP/HTTP des traces + logs + metriques. Renvoie un guard de
/// flush a garder vivant jusqu'a la fin du process.
pub fn init(opts: InitOpts) -> Result<TelemetryGuard> {
    let fmt_layer = if opts.json {
        tracing_subscriber::fmt::layer()
            .json()
            .with_target(true)
            .boxed()
    } else {
        tracing_subscriber::fmt::layer().with_target(true).boxed()
    };

    let Some(creds) = opts.otlp else {
        tracing_subscriber::registry()
            .with(opts.filter)
            .with(fmt_layer)
            .init();
        return Ok(TelemetryGuard::default());
    };

    let export = OtlpExport::new(
        creds,
        &opts.service_name,
        opts.deployment_environment.as_deref(),
    );

    let tracer_provider = build_tracer_provider(&export)?;
    let meter_provider = build_meter_provider(&export)?;
    let logger_provider = build_logger_provider(&export)?;

    let traces_layer = tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer(SCOPE));
    // Bridge logs : `otlp_log_level`+ partent en LogRecords OTLP (les events
    // sous ce plancher restent visibles via le layer fmt et les spans dans Tempo).
    let logs_layer =
        opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(&logger_provider)
            .with_filter(opts.otlp_log_level);

    tracing_subscriber::registry()
        .with(opts.filter)
        .with(fmt_layer)
        .with(traces_layer)
        .with(logs_layer)
        .init();

    tracing::info!(
        endpoint = %export.endpoint,
        service = %opts.service_name,
        "opentelemetry : traces + logs + metriques -> Grafana Cloud OTLP"
    );

    Ok(TelemetryGuard {
        tracer_provider: Some(tracer_provider),
        meter_provider: Some(meter_provider),
        logger_provider: Some(logger_provider),
    })
}

/// `Meter` du scope `librejustice` : point d'acces unique pour creer des
/// instruments (ex. observables du scraper Postgres). No-op si l'export OTel
/// n'est pas actif (provider global par defaut).
pub fn meter() -> opentelemetry::metrics::Meter {
    global::meter(SCOPE)
}

/// Construit le `TracerProvider` OTLP/HTTP et le pose en global. Le
/// [`ScrubbingSpanProcessor`] nettoie la PII avant de deleguer au
/// `BatchSpanProcessor` (serialisation + transmission sur thread dedie).
fn build_tracer_provider(export: &OtlpExport) -> Result<SdkTracerProvider> {
    let exporter = SpanExporter::builder()
        .with_http()
        .with_endpoint(export.signal_endpoint("traces"))
        .with_headers(export.headers())
        .build()?;

    let processor = ScrubbingSpanProcessor {
        inner: BatchSpanProcessor::builder(exporter).build(),
    };

    let provider = SdkTracerProvider::builder()
        .with_span_processor(processor)
        .with_resource(export.resource.clone())
        .build();

    global::set_tracer_provider(provider.clone());
    Ok(provider)
}

/// Construit le `MeterProvider` OTLP/HTTP (PeriodicReader, export ~60s) et le
/// pose en global.
fn build_meter_provider(export: &OtlpExport) -> Result<SdkMeterProvider> {
    let exporter = MetricExporter::builder()
        .with_http()
        .with_endpoint(export.signal_endpoint("metrics"))
        .with_headers(export.headers())
        .build()?;

    let reader = PeriodicReader::builder(exporter).build();

    let provider = SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(export.resource.clone())
        .build();

    global::set_meter_provider(provider.clone());
    Ok(provider)
}

/// Construit le `LoggerProvider` OTLP/HTTP (BatchLogProcessor) consomme par le
/// bridge `tracing`.
fn build_logger_provider(export: &OtlpExport) -> Result<SdkLoggerProvider> {
    let exporter = LogExporter::builder()
        .with_http()
        .with_endpoint(export.signal_endpoint("logs"))
        .with_headers(export.headers())
        .build()?;

    let provider = SdkLoggerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(export.resource.clone())
        .build();
    Ok(provider)
}

/// Cles d'attributs supprimees avant export (PII directe).
const PII_DROP_KEYS: &[&str] = &[
    "client.address",
    "client.socket.address",
    "net.peer.ip",
    "net.sock.peer.addr",
    "http.client_ip",
    "db.statement",
];

/// Cles d'URL dont on tronque la query-string (`?...` retire).
const URL_TRUNCATE_KEYS: &[&str] = &["url.full", "http.url", "http.target"];

/// `SpanProcessor` qui scrub la PII puis delegue au `BatchSpanProcessor`.
///
/// Sans collector, c'est le seul endroit ou la PII est retiree avant qu'elle ne
/// quitte le process (RGPD / ADR 0039). `librejustice.search.query` n'est pas
/// dans les listes : il part intact (attendu par le span de recherche).
#[derive(Debug)]
struct ScrubbingSpanProcessor {
    inner: BatchSpanProcessor,
}

impl SpanProcessor for ScrubbingSpanProcessor {
    fn on_start(&self, span: &mut Span, cx: &opentelemetry::Context) {
        self.inner.on_start(span, cx);
    }

    fn on_end(&self, mut span: SpanData) {
        span.attributes
            .retain(|kv| !PII_DROP_KEYS.contains(&kv.key.as_str()));
        for kv in &mut span.attributes {
            if URL_TRUNCATE_KEYS.contains(&kv.key.as_str()) {
                if let Value::String(url) = &kv.value {
                    if let Some(q) = url.as_str().find('?') {
                        let truncated = url.as_str()[..q].to_string();
                        kv.value = Value::String(truncated.into());
                    }
                }
            }
        }
        self.inner.on_end(span);
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.inner.force_flush()
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}

/// Nom d'hote (equivalent `socket.gethostname()`), best-effort.
fn hostname() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string())
}
