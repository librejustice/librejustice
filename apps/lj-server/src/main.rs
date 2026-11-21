//! Binaire de déploiement unique : API REST + MCP/OAuth + SSR Leptos + assets
//! statiques, dans un seul process axum partageant un `AppState` construit **une
//! fois**. Écoute en HTTP clair : le TLS d'origine est terminé par le front `caddy`.
//!
//! The SSR render calls the `lj-api` service layer in-process: the
//! `lj_web::api::ApiClient` provided to the Leptos context delegates to
//! `web_backend`. The browser calls `/internal` over same-origin HTTP.

use anyhow::Result;
use axum::body::{Body, Bytes, HttpBody};
use axum::Router;
use axum_tracing_opentelemetry::middleware::OtelAxumLayer;
use leptos::prelude::*;
use leptos_axum::{generate_route_list, LeptosRoutes};
use lj_api::{config::Settings, host_metrics, pg_metrics, state::AppState, telemetry};
use lj_pg::db;
use lj_web::app::{shell, App};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower_http::compression::predicate::{DefaultPredicate, NotForContentType, Predicate as _};
use tower_http::compression::CompressionLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeader;
use tracing::Instrument as _;

mod pinned_render;
mod tracing_executor;
mod web_backend;

// Heavy CPU (snippet highlight, PDF/DOCX render) goes to the blocking pool, capped
// at four threads: `#[tokio::main]` does not set `max_blocking_threads`.
fn main() -> Result<()> {
    let blocking_cap = cores().min(4);
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .max_blocking_threads(blocking_cap)
        .build()?
        .block_on(run())
}

fn cores() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get())
}

async fn run() -> Result<()> {
    let settings = Settings::from_env()?;
    let _telemetry_guard = telemetry::init_telemetry(&settings)?;
    install_panic_hook();
    tracing_executor::install(tokio::runtime::Handle::current())?;

    let conf = get_configuration(None).unwrap();
    let leptos_options = conf.leptos_options;
    let origin = settings.public_base_url.trim_end_matches('/');
    let redact = lj_api::redact::router(origin, lj_web::pages::redact::mcp_app::document(origin));
    let workbench_entry = lj_web::pages::redact::mcp_app::entry(&leptos_options);
    let pool = db::build_pool(&settings.db_url, settings.pool_max)?;
    let state = AppState::build(Arc::new(settings.clone()), pool);

    tokio::spawn(lj_api::cache_metrics::run(state.clone()));
    if settings.scrapes_shared_resources() {
        tokio::spawn(pg_metrics::run(state.pool.clone()));
        tokio::spawn(host_metrics::run());
    } else {
        tracing::info!(
            env = settings
                .deployment_environment
                .as_deref()
                .unwrap_or("(absent)"),
            "shared-resource scrapers off"
        );
    }

    {
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = lj_api::jurisdiction_hubs::catalogue(&state).await {
                tracing::warn!("jurisdiction catalogue warm-up failed: {e}");
            }
            if let Err(e) = lj_api::norm_hubs::catalogue(&state).await {
                tracing::warn!("norm catalogue warm-up failed: {e}");
            }
        });
    }
    tokio::spawn(lj_api::journals_jo::keep_hidden(state.clone()));
    // The beta and a dev server read the same base: one warmer counts for all.
    if settings.deployment_environment.as_deref() == Some("prod") {
        tokio::spawn(lj_api::search::keep_homes_warm(state.clone()));
    }

    let leptos_addr = leptos_options.site_addr;
    let routes = generate_route_list(App);

    // Bundle `/pkg` (wasm/js/css) : sert les variantes **précompressées** `.br`/`.gz`
    // générées au build quand le client les accepte.
    let pkg_dir = std::path::Path::new(&leptos_options.site_root.to_string())
        .join(leptos_options.site_pkg_dir.to_string());
    // Les noms de `/pkg` sont content-hashés : un fichier donné ne change jamais de
    // contenu, donc cachable un an `immutable`, et le HTML SSR (via `hash.txt` lu à
    // côté du binaire) référence toujours le bundle exact de son build.
    // `Vary: accept-encoding` : la même URL rend un corps brotli, gzip ou clair selon
    // la requête, et `ServeDir` ne le déclare pas.
    // Bundles des déploiements précédents : `<site_root>-keep/<pkg>`, monté depuis le
    // `state_dir`. Un document HTML antérieur au déploiement courant y trouve le
    // bundle exact de son build et s'hydrate ; les noms étant content-hashés, les deux
    // répertoires fusionnent sans collision possible.
    let keep_dir = std::path::Path::new(&format!("{}-keep", leptos_options.site_root))
        .join(leptos_options.site_pkg_dir.to_string());
    let pkg_service = SetResponseHeader::overriding(
        SetResponseHeader::overriding(
            ServeDir::new(pkg_dir)
                .precompressed_br()
                .precompressed_gzip()
                .fallback(
                    ServeDir::new(keep_dir)
                        .precompressed_br()
                        .precompressed_gzip(),
                ),
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("public, max-age=31536000, immutable"),
        ),
        axum::http::header::VARY,
        axum::http::HeaderValue::from_static("accept-encoding"),
    );
    // Y compris `/pkg/snippets/**` : leur chemin sur disque n'est pas content-hashé
    // (c'est la clé d'import que le wasm déclare), mais le glue les charge sous
    // `…/auth.js?v=<empreinte>`, posée au build par `build-app`.
    //
    // Fonts immuables : un `.woff2` n'est jamais réécrit en place (changer de police =
    // nouveau fichier), donc cachable un an `immutable` sans hash.
    let fonts_dir = std::path::Path::new(&leptos_options.site_root.to_string()).join("fonts");
    let fonts_service = SetResponseHeader::overriding(
        ServeDir::new(fonts_dir),
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("public, max-age=31536000, immutable"),
    );

    // These names carry no fingerprint: they revalidate on every load, and an
    // unchanged module answers 304.
    let site_root = std::path::PathBuf::from(leptos_options.site_root.to_string());
    let redactor_service = SetResponseHeader::overriding(
        SetResponseHeader::overriding(
            ServeDir::new(site_root.join("redactor"))
                .precompressed_br()
                .precompressed_gzip(),
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("public, no-cache"),
        ),
        axum::http::header::VARY,
        axum::http::HeaderValue::from_static("accept-encoding"),
    );

    // The front's SSR client is provided to the context of every request. The
    // fallback gets the same context: the Leptos router matches trailing-slash
    // paths (`/codes/`) to their routes (`/codes`) where axum does not, so its
    // render goes through real pages, which read that client. Those paths are
    // first redirected to their canonical form without the slash.
    let web_client = lj_web::api::ApiClient::new(web_backend::ServiceBackend {
        state: state.clone(),
    });
    let leptos_fallback = leptos_axum::file_and_error_handler_with_context(
        {
            let web_client = web_client.clone();
            move || provide_context(web_client.clone())
        },
        shell,
    );
    let site_fallback = move |uri: axum::http::Uri,
                              st: axum::extract::State<LeptosOptions>,
                              req: axum::extract::Request| {
        let leptos_fallback = leptos_fallback.clone();
        async move {
            let path = uri.path();
            if path.len() > 1 && path.ends_with('/') {
                let canonical = path.trim_end_matches('/');
                let target = match uri.query() {
                    Some(q) => format!("{canonical}?{q}"),
                    None => canonical.to_string(),
                };
                return axum::response::IntoResponse::into_response(
                    axum::response::Redirect::permanent(&target),
                );
            }
            // leptos_axum 0.8 `get_static_file` forwards `Accept-Encoding` alone: a
            // byte range came back whole, and Safari plays no video served that way.
            if path.ends_with(".mp4") || path.ends_with(".webm") {
                return match ServeDir::new(&*st.site_root).try_call(req).await {
                    Ok(res) => axum::response::IntoResponse::into_response(res),
                    Err(e) => axum::response::IntoResponse::into_response((
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        e.to_string(),
                    )),
                };
            }
            leptos_fallback(uri, st, req).await
        }
    };
    let leptos_router = Router::new()
        .leptos_routes_with_context(
            &leptos_options,
            routes,
            {
                let web_client = web_client.clone();
                move || provide_context(web_client.clone())
            },
            {
                let leptos_options = leptos_options.clone();
                move || shell(leptos_options.clone())
            },
        )
        // Cloudflare's challenge form leaves a POST document, which a reload repeats.
        .method_not_allowed_fallback(|uri: axum::http::Uri| async move {
            let target = match uri.query() {
                Some(q) => format!("{}?{q}", uri.path()),
                None => uri.path().to_string(),
            };
            axum::response::Redirect::to(&target)
        })
        .fallback(site_fallback.clone())
        .with_state(leptos_options.clone())
        .layer(axum::middleware::from_fn(
            move |req: axum::extract::Request, next: axum::middleware::Next| {
                let (site_fallback, options) = (site_fallback.clone(), leptos_options.clone());
                async move {
                    if !names_a_root_file(&req) {
                        return next.run(req).await;
                    }
                    site_fallback(req.uri().clone(), axum::extract::State(options), req).await
                }
            },
        ))
        .layer(axum::middleware::from_fn_with_state(
            pinned_render::Renders::new(cores()),
            pinned_render::render,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            lj_api::page_gates::session_gates,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            reader_home,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            ancienne_adresse,
        ))
        .layer(axum::middleware::from_fn(body_in_request_span))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            lj_api::admission::admit,
        ))
        .layer(OtelAxumLayer::default());

    // Fusion : routes données/MCP/OAuth (`lj-api`, sans fallback — c'est le
    // file-handler Leptos qui catch-all) + redirection `/mcp` → `/mcp/`.
    // `CompressionLayer` (br + gzip, négociés via `Accept-Encoding`) pour les réponses
    // dynamiques (HTML SSR, JSON API) ; elle saute les réponses déjà encodées.
    // Identité du bundle servi, posée sur les réponses `/api` que rien ne peut
    // resservir depuis un cache : un onglet ouvert avant un déploiement voit la
    // valeur changer et propose de recharger (`lj_web::app::BANDEAU`).
    let mut api = lj_api::routes::build_router(state.clone(), true);
    if let Some(build) = front_build_id() {
        let nom = axum::http::HeaderName::from_static("x-lj-front-build");
        api = api.layer(axum::middleware::map_response(
            move |mut resp: axum::response::Response| {
                let (nom, build) = (nom.clone(), build.clone());
                async move {
                    let cachable = resp
                        .headers()
                        .get(axum::http::header::CACHE_CONTROL)
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|v| v.contains("public"));
                    if !cachable {
                        resp.headers_mut().insert(nom, build);
                    }
                    resp
                }
            },
        ));
    }

    let app = match lj_api::redact::PUBLISHED {
        true => leptos_router.merge(api).merge(redact),
        false => leptos_router.merge(api),
    };
    let app = app
        .route(
            "/mcp",
            axum::routing::get(|| async { axum::response::Redirect::temporary("/mcp/") }),
        )
        .route(
            "/mentions-legales",
            redirect_preserving_query("/legal-notice"),
        )
        .route("/confidentialite", redirect_preserving_query("/privacy"))
        // French auth links still sit in mailboxes and bookmarks: their query
        // (token, next) survives the redirect.
        .route("/confirmer", redirect_preserving_query("/confirm"))
        .route(
            "/reinitialiser-mot-de-passe",
            redirect_preserving_query("/reset-password"),
        )
        .route("/profil", redirect_preserving_query("/profile"))
        .route("/connexion", redirect_preserving_query("/login-link"))
        .route(
            lj_vocab::files::ENTRY,
            axum::routing::get(move || async move {
                (
                    [
                        (
                            axum::http::header::CONTENT_TYPE,
                            "text/javascript; charset=utf-8",
                        ),
                        (axum::http::header::CACHE_CONTROL, "no-cache"),
                    ],
                    workbench_entry,
                )
            }),
        )
        .nest_service("/pkg", pkg_service)
        .nest_service("/fonts", fonts_service)
        .nest_service("/redactor", redactor_service)
        .layer(axum::middleware::from_fn(place_prefix_moved))
        // A compressed video goes out chunked, without a length: the CDN caches it
        // that way and then answers a byte range with the whole file.
        .layer(
            CompressionLayer::new()
                .compress_when(DefaultPredicate::new().and(NotForContentType::const_new("video/"))),
        )
        .layer(axum::middleware::from_fn(html_cache_policy))
        .layer(axum::middleware::from_fn(security_headers))
        .layer(axum::middleware::from_fn(workbench_files))
        .layer(axum::middleware::from_fn(pageview_span))
        .layer(axum::middleware::from_fn(crawler_trace_policy));

    let handle = axum_server::Handle::new();
    let arret = handle.clone();
    tokio::spawn(async move {
        signal_arret().await;
        arret.graceful_shutdown(Some(DRAINAGE));
    });
    tracing::info!("lj-server listening on {leptos_addr}");
    axum_server::bind(leptos_addr)
        .handle(handle)
        .serve(app.into_make_service())
        .await?;
    Ok(())
}

fn install_panic_hook() {
    let defaut = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(
            panic = %info,
            backtrace = %std::backtrace::Backtrace::force_capture(),
            "panic lj-server"
        );
        defaut(info);
    }));
}

const DRAINAGE: std::time::Duration = std::time::Duration::from_secs(5);

/// Attend SIGTERM (`podman stop`) ou Ctrl-C (dev).
///
/// Le binaire est **PID 1** du conteneur : le noyau n'applique pas l'action par
/// défaut d'un signal à PID 1 tant qu'aucun handler n'est installé, SIGTERM serait
/// donc ignoré.
async fn signal_arret() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("handler SIGTERM");
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    tracing::info!("shutdown signal received, draining in-flight connections");
}

async fn pageview_span(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let is_document = req.method() == axum::http::Method::GET
        && req
            .headers()
            .get(axum::http::header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("text/html"));
    let host = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| req.uri().host())
        .map(normalized_host);
    let external_referer = req
        .headers()
        .get(axum::http::header::REFERER)
        .and_then(|v| v.to_str().ok())
        .filter(|r| referer_is_external(r, host.as_deref()))
        .map(str::to_owned);
    let Some(referer) = external_referer.filter(|_| is_document) else {
        return next.run(req).await;
    };
    let path = req.uri().path().to_owned();
    let user_agent = req
        .headers()
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let client = client_kind(&user_agent, req.headers());
    let browser = browser_family(&user_agent);
    // Posé par Cloudflare, digne de foi pour la même raison que `CF-Connecting-IP` :
    // l'origine n'accepte que les plages Cloudflare en :443. `XX` est sa valeur pour
    // une IP non localisée.
    let country = req
        .headers()
        .get("cf-ipcountry")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("XX")
        .to_owned();
    let response = next.run(req).await;
    if !response.status().is_redirection() {
        let _pageview = tracing::info_span!(
            "pageview",
            librejustice.pageview.path = %path,
            librejustice.pageview.referer = %referer,
            librejustice.pageview.client = client,
            librejustice.pageview.browser = browser,
            librejustice.pageview.country = %country,
        )
        .entered();
    }
    response
}

async fn crawler_trace_policy(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let unattended = req
        .headers()
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .and_then(lj_crawlers::category_of)
        .is_some_and(lj_crawlers::Category::is_unattended);
    if unattended {
        next.run(req)
            .instrument(telemetry::unattended_request_span())
            .await
    } else {
        next.run(req).await
    }
}

async fn body_in_request_span(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let span = tracing::Span::current();
    let (parts, body) = next.run(req).await.into_parts();
    axum::response::Response::from_parts(parts, Body::new(SpannedBody { inner: body, span }))
}

struct SpannedBody {
    inner: Body,
    span: tracing::Span,
}

impl HttpBody for SpannedBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let corps = self.get_mut();
        let _entree = corps.span.enter();
        Pin::new(&mut corps.inner).poll_frame(cx)
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }
}

/// Classe l'appelant d'une arrivée : `browser` | `bot` | `unknown`.
///
/// Deux signaux, du plus falsifiable au moins :
///
/// 1. le UA **quand il se déclare**. Ne remontent ici que les déplieurs de lien
///    (Slack, Facebook, Discord), qui posent un `Referer` ; la table complète de la
///    politique d'accès est [`lj_crawlers::CRAWLERS`].
/// 2. la cohérence de `Sec-Fetch-*`, qu'un moteur de rendu seul produit : une
///    navigation vers un document pose `Dest: document`, un client HTTP n'en pose
///    aucun, et un `fetch()` d'une autre origine en pose un autre.
///
/// `unknown` est la réponse honnête pour un client qui se présente en navigateur
/// sans le prouver — un scraper à UA plausible y tombe, mais Safari < 16.4 aussi.
fn client_kind(user_agent: &str, headers: &axum::http::HeaderMap) -> &'static str {
    if ["bot", "crawler", "spider", "facebookexternalhit"]
        .iter()
        .any(|marker| user_agent.contains(marker))
    {
        return "bot";
    }
    match headers.get("sec-fetch-dest").and_then(|v| v.to_str().ok()) {
        Some("document") => "browser",
        Some(_) => "bot",
        None => "unknown",
    }
}

fn normalized_host(authority: &str) -> String {
    let without_port = match authority.strip_prefix('[') {
        Some(rest) => rest.split_once(']').map_or(rest, |(host, _)| host),
        None => authority
            .split_once(':')
            .map_or(authority, |(host, _)| host),
    };
    let bare = without_port.strip_prefix("www.").unwrap_or(without_port);
    bare.to_ascii_lowercase()
}

fn referer_is_external(referer: &str, host: Option<&str>) -> bool {
    let Some(authority) = referer
        .split_once("://")
        .and_then(|(_, rest)| rest.split(['/', '?', '#']).next())
        .filter(|a| !a.is_empty())
    else {
        return false;
    };
    let referrer_host = normalized_host(authority.rsplit_once('@').map_or(authority, |(_, h)| h));
    if referrer_host == "localhost" || referrer_host == "::1" || referrer_host.starts_with("127.") {
        return false;
    }
    host != Some(referrer_host.as_str())
}

/// Famille du navigateur — `edge` | `chrome` | `firefox` | `safari` | `other`.
/// L'ordre des tests est imposé par l'imbrication des UA : Edge se déclare aussi
/// `chrome` et `safari`, Chrome se déclare aussi `safari`.
fn browser_family(user_agent: &str) -> &'static str {
    if user_agent.contains("edg/") {
        "edge"
    } else if user_agent.contains("chrome/") || user_agent.contains("chromium") {
        "chrome"
    } else if user_agent.contains("firefox/") {
        "firefox"
    } else if user_agent.contains("safari/") {
        "safari"
    } else {
        "other"
    }
}

/// En-têtes de sécurité posés sur **toute** réponse (documents HTML, JSON de
/// l'API, exports binaires, assets statiques).
///
/// La CSP complète du document est posée par le shell SSR, qui seul connaît le
/// `nonce` de la requête (`lj_web::app::content_security_policy`) ; on ne pose ici
/// qu'un socle pour les réponses qui n'en ont pas — API, `/pkg`, `/fonts`,
/// exports —, d'où le test `contains_key`.
///
/// `Strict-Transport-Security` posé sans condition : un UA **doit** ignorer
/// l'en-tête reçu hors transport sécurisé (RFC 6797 §8.1), donc le dev en clair et
/// le hop `caddy → app` en clair ne peuvent pas l'armer par accident. Sans
/// `includeSubDomains` ni `preload`, qui s'étendent à des sous-domaines futurs pour
/// un an sans possibilité de rétractation.
async fn security_headers(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::header;
    use axum::http::HeaderValue;

    let worker = worker_policy(&req);
    let mut resp = next.run(req).await;
    let headers = resp.headers_mut();
    if let Some(policy) = worker {
        headers.insert(header::CONTENT_SECURITY_POLICY, policy);
    }
    for (name, value) in [
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::X_FRAME_OPTIONS, "DENY"),
        (header::REFERRER_POLICY, "strict-origin-when-cross-origin"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    headers.insert(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=(), payment=(), usb=()"),
    );
    headers.insert(
        header::STRICT_TRANSPORT_SECURITY,
        HeaderValue::from_static("max-age=31536000"),
    );
    if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "frame-ancestors 'none'; base-uri 'self'; form-action 'self'; object-src 'none'",
            ),
        );
    }
    resp
}

/// The workbench's view, framed by an MCP host, loads these files from another origin.
async fn workbench_files(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let shared = lj_vocab::files::shared(req.uri().path());
    let mut resp = next.run(req).await;
    if shared {
        resp.headers_mut().insert(
            axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
            axum::http::HeaderValue::from_static("*"),
        );
    }
    resp
}

/// The text readers' workers answer to the policy their own script is served
/// with, not the page's: the handwriting one reaches nothing, the OCR one its files.
fn worker_policy(req: &axum::extract::Request) -> Option<axum::http::HeaderValue> {
    let connect = match req.uri().path() {
        "/htr-worker.js" => "'none'".to_string(),
        "/ocr/worker.min.js" => {
            let host = req.headers().get(axum::http::header::HOST)?.to_str().ok()?;
            format!("{host}/ocr/")
        }
        _ => return None,
    };
    axum::http::HeaderValue::from_str(&format!("connect-src {connect}; object-src 'none'")).ok()
}

/// Politique de cache **par défaut** des documents HTML : `private, no-store`
/// pour toute page qui ne déclare pas la sienne. Une page publique pose son
/// `Cache-Control` via `ResponseOptions` (décision / texte / entité : 7 j de
/// CDN ; accueil, recherche, pages statiques : 5 min).
///
/// Le défaut protège, il n'optimise pas : la session est un cookie `HttpOnly`
/// lisible au rendu, donc une page peut personnaliser son HTML. L'oubli d'un
/// en-tête doit produire une page non cachée, jamais une identité servie à un
/// autre visiteur depuis un cache partagé.
async fn html_cache_policy(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut resp = next.run(req).await;
    let is_html = resp
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    if is_html
        && !resp
            .headers()
            .contains_key(axum::http::header::CACHE_CONTROL)
    {
        resp.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("private, no-store"),
        );
    }
    resp
}

/// Identité du bundle front servi, lue dans `hash.txt` à côté du binaire — là où
/// leptos le lit. `None` en dev, où le fichier n'existe pas : aucun en-tête posé,
/// mécanisme inerte.
fn front_build_id() -> Option<axum::http::HeaderValue> {
    let fichier = std::env::current_exe().ok()?.parent()?.join("hash.txt");
    let id = build_id(&std::fs::read_to_string(fichier).ok()?)?;
    axum::http::HeaderValue::from_str(&id).ok()
}

/// Empreintes `wasm` et `css` de `hash.txt`. Le `js` n'identifie pas un build :
/// deux compilations au Rust différent rendent la même empreinte de glue.
fn build_id(hash_txt: &str) -> Option<String> {
    let champ = |cle: &str| {
        hash_txt.lines().find_map(|ligne| {
            let (k, v) = ligne.split_once(':')?;
            (k.trim() == cle).then(|| v.trim())
        })
    };
    Some(format!("{}.{}", champ("wasm")?, champ("css")?))
}

/// Today's address of a path in an older shape or under a wrong jurisdiction, for
/// the shapes that need no lookup. The caller drops a result equal to the path;
/// `None` for an entity namespace no page serves.
fn renamed_path(path: &str) -> Option<String> {
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    Some(match segs.as_slice() {
        ["recherche" | "recherche-decisions" | "decisions"] => "/decision-search".to_string(),
        ["recherche-textes" | "textes"] => "/norm-search".to_string(),
        ["annonces" | "notice-search"] => "/journal-search".to_string(),
        ["annonces.rss" | "notice-search.rss"] => "/journal-search.rss".to_string(),
        ["annonces.ics" | "notice-search.ics"] => "/journal-search.ics".to_string(),
        ["annonces", "departement"] | ["notice-index" | "journal-index"] => {
            "/journal-index/fr".to_string()
        }
        ["annonces", "departement", code] => format!("/journal-index/fr/{code}"),
        ["marche", uid] => lj_vocab::notice_path("FR", uid),
        ["entite", ns, id] => {
            lj_vocab::entity_segment(ns)?;
            lj_vocab::entity_path(ns, id)
        }
        ["annuaire"] => "/entity-search".to_string(),
        ["annuaire", kind] => format!("/entity-index/fr/{kind}"),
        ["juridictions"] | ["decision-index"] => "/decision-index/fr".to_string(),
        ["corpus"] => "/norm-index/fr/in-force".to_string(),
        ["corpus", date] => format!("/norm-index/fr/in-force/{date}"),
        ["normes"] | ["norm-index"] => "/norm-index/fr".to_string(),
        ["normes", "codes"] | ["codes"] => "/norm-index/fr/codes".to_string(),
        ["normes", fond] => format!("/norm-index/{}/{fond}", fond_jurisdiction(fond)),
        ["normes", fond, year] => {
            format!("/norm-index/{}/{fond}/{year}", fond_jurisdiction(fond))
        }
        ["entity", _, ns, id] => {
            lj_vocab::entity_segment(ns)?;
            lj_vocab::entity_path(ns, id)
        }
        ["notice", _, uid @ ..] if !uid.is_empty() => lj_api::notices::path(&uid.join("/")),
        ["notice-index" | "journal-index", _, rest @ ..] if rest.len() <= 1 => {
            prefixed_fr("/journal-index/fr", rest)
        }
        ["entity-index", _, kind] => format!("/entity-index/fr/{kind}"),
        ["norm-index", _, "in-force", rest @ ..] if rest.len() <= 1 => {
            prefixed_fr("/norm-index/fr/in-force", rest)
        }
        _ => return None,
    })
}

fn prefixed_fr(base: &str, rest: &[&str]) -> String {
    match rest.first() {
        Some(seg) => format!("{base}/{seg}"),
        None => base.to_string(),
    }
}

/// The jurisdiction an old catalogue fond, which mixed them, is now browsed under.
fn fond_jurisdiction(fond: &str) -> &'static str {
    match fond {
        "textes-ue" => "eu",
        "traites" => "intl",
        _ => "fr",
    }
}

/// Every older address answers with a permanent redirect to today's: the renamed
/// prefixes, a decision, a court, an entity or a notice without its jurisdiction
/// or under another one, and a text under
/// `/texte/`, `/loi/` or without its jurisdiction segment. No text slug is two
/// letters nor equals `intl`, so a recognised first segment is already the new
/// shape.
async fn ancienne_adresse(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let uri = req.uri().clone();
    let segs: Vec<&str> = uri.path().trim_matches('/').split('/').collect();
    let cible = match segs.as_slice() {
        ["decision", id] | ["decision", _, id] => {
            lj_api::decisions::decision_address(&state, id).await
        }
        ["juridiction" | "decision-index", code, rest @ ..] if rest.len() <= 1 => {
            juridiction_address(&state, code, rest.first().copied()).await
        }
        ["decision-index", _, code, rest @ ..] if rest.len() <= 1 => {
            juridiction_address(&state, code, rest.first().copied()).await
        }
        [prefix @ ("texte" | "loi" | "norm"), premier, suite @ ..] => {
            text_address(&state, prefix, premier, suite).await
        }
        _ => renamed_path(uri.path()),
    };
    let Some(mut cible) = cible.filter(|c| c != uri.path()) else {
        let resp = next.run(req).await;
        if resp.status() != axum::http::StatusCode::NOT_FOUND {
            return resp;
        }
        if let ["decision", .., id] = segs.as_slice() {
            return match lj_api::decisions::conclusions_ruling_address(&state, id).await {
                Some(ruling) => axum::response::Redirect::permanent(&ruling).into_response(),
                None => resp,
            };
        }
        let Some(slug) = moved_norm_slug(&segs) else {
            return resp;
        };
        return match lj_api::notices::moved_text(&state, slug).await {
            Ok(Some(notice)) => axum::response::Redirect::permanent(&notice).into_response(),
            _ => resp,
        };
    };
    if let Some(q) = uri.query() {
        cible.push('?');
        cible.push_str(q);
    }
    axum::response::Redirect::permanent(&cible).into_response()
}

/// A file of the site root, which the `/{juris}` route captures with the
/// jurisdictions: its one segment is never two lower-case letters.
fn names_a_root_file(req: &axum::extract::Request) -> bool {
    let captured = req
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .is_some_and(|m| m.as_str() == "/{juris}");
    let segment = req.uri().path().trim_start_matches('/');
    captured && !(segment.len() == 2 && segment.bytes().all(|b| b.is_ascii_lowercase()))
}

/// The home page, the three searches and the API guide, reached without a
/// jurisdiction, answer with the reader's, which differs per reader and so is never cached.
async fn reader_home(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let unscoped = matches!(
        req.uri().path(),
        "/" | "/decision-search" | "/norm-search" | "/journal-search" | "/api-guide"
    );
    let read = matches!(
        *req.method(),
        axum::http::Method::GET | axum::http::Method::HEAD
    );
    if !(unscoped && read) {
        return next.run(req).await;
    }
    let known = match lj_api::referential::referential(&state).await {
        Ok(refs) => lj_api::jurisdictions::known(&refs),
        Err(e) => return e.into_response(),
    };
    let headers = req.headers();
    let cookie = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| lj_api::session::read_cookie(v, lj_dtos::READER_JURISDICTION_COOKIE));
    let country = headers.get("cf-ipcountry").and_then(|v| v.to_str().ok());
    let code = reader_jurisdiction(cookie.as_deref(), country, &known);
    let mut target = format!("/{code}{}", req.uri().path().trim_end_matches('/'));
    if let Some(q) = req.uri().query() {
        target.push('?');
        target.push_str(q);
    }
    let mut resp = axum::response::Redirect::temporary(&target).into_response();
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, no-store"),
    );
    resp
}

/// The jurisdiction the reader chose, else the country Cloudflare places their
/// address in, else France; a code the corpus does not hold is passed over.
fn reader_jurisdiction(
    cookie: Option<&str>,
    country: Option<&str>,
    known: &[lj_dtos::ReaderJurisdiction],
) -> String {
    let held = |code: &str| known.iter().any(|j| j.code == code);
    cookie
        .filter(|c| held(c))
        .map(str::to_string)
        .or_else(|| country.map(str::to_ascii_lowercase).filter(|c| held(c)))
        .unwrap_or_else(|| "fr".to_string())
}

/// The French text slug a norm address names, under its jurisdiction or an older shape.
fn moved_norm_slug<'a>(segs: &[&'a str]) -> Option<&'a str> {
    match segs {
        ["norm", "fr", slug, ..] => Some(slug),
        ["norm" | "texte" | "loi", first, ..]
            if lj_norm::jurisdictions::from_url_segment(first).is_none() =>
        {
            Some(first)
        }
        _ => None,
    }
}

/// A place written with its vocabulary prefix (`?place=geo:PF`), as old links
/// carry it, moves to the bare code, pages and API alike.
async fn place_prefix_moved(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let Some(query) = req.uri().query().and_then(place_without_prefix) else {
        return next.run(req).await;
    };
    axum::response::Redirect::permanent(&format!("{}?{query}", req.uri().path())).into_response()
}

fn place_without_prefix(query: &str) -> Option<String> {
    let mut moved = false;
    let pairs: Vec<String> = query
        .split('&')
        .map(|pair| {
            let bare = ["place=geo:", "place=geo%3A", "place=geo%3a"]
                .iter()
                .find_map(|prefix| pair.strip_prefix(prefix));
            match bare {
                Some(code) => {
                    moved = true;
                    format!("place={code}")
                }
                None => pair.to_string(),
            }
        })
        .collect();
    moved.then(|| pairs.join("&"))
}

async fn juridiction_address(state: &AppState, code: &str, year: Option<&str>) -> Option<String> {
    let referential = lj_api::referential::referential(state).await.ok()?;
    let entry = referential.jurisdiction(code)?;
    let jt = lj_dtos::JurisdictionType::from_code(&entry.jurisdiction_type)?;
    let hub = format!("/decision-index/{}/{code}", jt.url_segment(Some(code)));
    Some(match year {
        Some(y) => format!("{hub}/{y}"),
        None => hub,
    })
}

async fn text_address(
    state: &AppState,
    prefix: &str,
    premier: &str,
    suite: &[&str],
) -> Option<String> {
    let juris = lj_norm::jurisdictions::from_url_segment(premier);
    let current = juris.is_some();
    let (code, rest) = match juris {
        Some(_) => (*suite.first()?, &suite[1..]),
        None => (premier, suite),
    };
    if let ["section", cid] = rest {
        let found = lj_api::legi::section_address(state, code, juris, cid).await;
        if let Ok(Some(address)) = found {
            return Some(address);
        }
    }
    if prefix == "norm" && current {
        return None;
    }
    let mut cible = if current {
        format!("/norm/{premier}")
    } else {
        lj_api::legi::text_address(state, premier).await?
    };
    for seg in suite {
        cible.push('/');
        cible.push_str(if *seg == "comparer" { "compare" } else { seg });
    }
    Some(cible)
}

/// Route GET answering a 308 to `target`, query string kept as is.
fn redirect_preserving_query(target: &'static str) -> axum::routing::MethodRouter {
    axum::routing::get(move |raw: axum::extract::RawQuery| async move {
        let dest = match raw.0.filter(|q| !q.is_empty()) {
            Some(q) => format!("{target}?{q}"),
            None => target.to_string(),
        };
        axum::response::Redirect::permanent(&dest)
    })
}

#[cfg(test)]
mod tests {
    use super::{
        body_in_request_span, browser_family, build_id, client_kind, reader_jurisdiction,
        referer_is_external, renamed_path, SpannedBody,
    };
    use axum::body::{Body, Bytes, HttpBody};
    use axum::http::HeaderMap;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};
    use tracing::Instrument;

    const CHROME_MOBILE: &str = "mozilla/5.0 (linux; android 10; k) applewebkit/537.36 \
        (khtml, like gecko) chrome/132.0.0.0 mobile safari/537.36";
    const APPLEBOT: &str = "mozilla/5.0 (macintosh; intel mac os x 10_15_7) \
        applewebkit/605.1.15 (khtml, like gecko) version/17.4 safari/605.1.15 (applebot/0.1; \
        +http://www.apple.com/go/applebot)";
    const YANDEX: &str = "mozilla/5.0 (compatible; yandexbot/3.0; +http://yandex.com/bots)";

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn ua_declare_prime_sur_sec_fetch() {
        let h = headers(&[("sec-fetch-dest", "document")]);
        assert_eq!(client_kind(APPLEBOT, &h), "bot");
        assert_eq!(client_kind(YANDEX, &h), "bot");
    }

    #[test]
    fn navigation_vers_un_document_est_un_navigateur() {
        let h = headers(&[
            ("sec-fetch-dest", "document"),
            ("sec-fetch-mode", "navigate"),
        ]);
        assert_eq!(client_kind(CHROME_MOBILE, &h), "browser");
    }

    #[test]
    fn sec_fetch_incoherent_est_un_bot() {
        let h = headers(&[("sec-fetch-dest", "empty")]);
        assert_eq!(client_kind(CHROME_MOBILE, &h), "bot");
    }

    #[test]
    fn sans_sec_fetch_on_ne_sait_pas() {
        assert_eq!(client_kind(CHROME_MOBILE, &HeaderMap::new()), "unknown");
        assert_eq!(client_kind("", &HeaderMap::new()), "unknown");
    }

    #[test]
    fn une_arrivee_vient_d_un_autre_hote_et_jamais_d_une_boucle_locale() {
        let servi = Some("exemple.test");
        assert!(referer_is_external("https://www.google.com/", servi));
        assert!(referer_is_external(
            "android-app://com.linkedin.android/",
            servi
        ));
        assert!(!referer_is_external(
            "https://exemple.test/decisions?q=a",
            servi
        ));
        assert!(!referer_is_external("https://www.exemple.test/", servi));
        assert!(!referer_is_external(
            "http://127.0.0.1:3000/decisions?q=a",
            servi
        ));
        assert!(!referer_is_external("http://localhost:3000/", servi));
        assert!(!referer_is_external("http://[::1]:3000/", servi));
        assert!(!referer_is_external("pas une url", servi));
        assert!(referer_is_external("https://exemple.test/", None));
        assert!(!referer_is_external("http://127.0.0.1:3000/", None));
    }

    #[test]
    fn familles_dans_l_ordre_d_imbrication() {
        assert_eq!(
            browser_family(
                "mozilla/5.0 (windows nt 10.0; win64; x64) applewebkit/537.36 (khtml, like \
                 gecko) chrome/133.0.0.0 safari/537.36 edg/133.0.0.0"
            ),
            "edge"
        );
        assert_eq!(browser_family(CHROME_MOBILE), "chrome");
        assert_eq!(
            browser_family(
                "mozilla/5.0 (macintosh; intel mac os x 10_15_7) applewebkit/605.1.15 (khtml, \
                 like gecko) version/17.4 safari/605.1.15"
            ),
            "safari"
        );
        assert_eq!(
            browser_family(
                "mozilla/5.0 (x11; linux x86_64; rv:127.0) gecko/20100101 firefox/127.0"
            ),
            "firefox"
        );
        assert_eq!(browser_family("guzzlehttp/7"), "other");
    }

    /// Spec de l'identité de bundle : elle change dès qu'un des deux artefacts
    /// hydratés change, et ignore l'empreinte de la glue JS, que deux builds au
    /// Rust différent partagent.
    #[test]
    fn identite_du_bundle_ignore_la_glue_js() {
        let a = "js: GGGG\nwasm: WWWW\ncss: CCCC\n";
        assert_eq!(build_id(a).as_deref(), Some("WWWW.CCCC"));
        assert_ne!(build_id(a), build_id("js: GGGG\nwasm: XXXX\ncss: CCCC\n"));
        assert_ne!(build_id(a), build_id("js: GGGG\nwasm: WWWW\ncss: DDDD\n"));
        assert_eq!(build_id("js: GGGG"), None);
    }

    struct Fermetures(Arc<Mutex<Vec<tracing::Id>>>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Fermetures {
        fn on_close(&self, id: tracing::Id, _ctx: tracing_subscriber::layer::Context<'_, S>) {
            self.0.lock().unwrap().push(id);
        }
    }

    type Observations = Arc<Mutex<Vec<(Option<tracing::Id>, usize)>>>;

    #[tokio::test]
    async fn le_corps_streame_est_poll_dans_le_span_de_requete() {
        use axum::extract::Request;
        use axum::Router;
        use tower::ServiceExt;
        use tracing_subscriber::layer::SubscriberExt;

        let fermetures: Arc<Mutex<Vec<tracing::Id>>> = Arc::default();
        let _abonne = tracing::subscriber::set_default(
            tracing_subscriber::registry().with(Fermetures(fermetures.clone())),
        );

        let observe: Observations = Arc::default();
        let span_requete: Arc<Mutex<Option<tracing::Id>>> = Arc::default();

        let corps = {
            let observe = observe.clone();
            let fermetures = fermetures.clone();
            move || {
                let (observe, fermetures) = (observe.clone(), fermetures.clone());
                let mut restant = 3;
                async move {
                    Body::from_stream(futures::stream::poll_fn(move |_cx| {
                        observe.lock().unwrap().push((
                            tracing::Span::current().id(),
                            fermetures.lock().unwrap().len(),
                        ));
                        restant -= 1;
                        if restant > 0 {
                            Poll::Ready(Some(Ok::<_, std::io::Error>("<p>fragment</p>")))
                        } else {
                            Poll::Ready(None)
                        }
                    }))
                }
            }
        };

        let racine = {
            let span_requete = span_requete.clone();
            axum::middleware::from_fn(move |req: Request, next: axum::middleware::Next| {
                let span_requete = span_requete.clone();
                async move {
                    let span = tracing::info_span!("requete");
                    *span_requete.lock().unwrap() = span.id();
                    next.run(req).instrument(span).await
                }
            })
        };

        let app = Router::new()
            .route("/", axum::routing::get(corps))
            .layer(axum::middleware::from_fn(body_in_request_span))
            .layer(racine);

        let reponse = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let octets = axum::body::to_bytes(reponse.into_body(), usize::MAX)
            .await
            .unwrap();

        assert_eq!(&octets[..], b"<p>fragment</p><p>fragment</p>");

        let attendu = span_requete.lock().unwrap().clone();
        assert!(attendu.is_some(), "le span de requête doit avoir un id");
        let observe = observe.lock().unwrap().clone();
        assert_eq!(observe.len(), 3, "trois polls : deux fragments puis la fin");
        for (courant, deja_fermes) in observe {
            assert_eq!(courant, attendu, "poll du corps hors du span de requête");
            assert_eq!(deja_fermes, 0, "span refermé avant la fin du flux");
        }
        assert_eq!(*fermetures.lock().unwrap(), vec![attendu.unwrap()]);
    }

    /// Corps de test à deux frames : une donnée, puis des trailers.
    struct CorpsAvecTrailers(u8);

    impl HttpBody for CorpsAvecTrailers {
        type Data = Bytes;
        type Error = axum::Error;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
            self.0 += 1;
            match self.0 {
                1 => Poll::Ready(Some(Ok(http_body::Frame::data(Bytes::from_static(b"abc"))))),
                2 => {
                    let mut trailers = HeaderMap::new();
                    trailers.insert("x-somme", "42".parse().unwrap());
                    Poll::Ready(Some(Ok(http_body::Frame::trailers(trailers))))
                }
                _ => Poll::Ready(None),
            }
        }
    }

    /// Frame suivante d'un corps, poll à la main.
    async fn frame(
        corps: &mut SpannedBody,
    ) -> Option<Result<http_body::Frame<Bytes>, axum::Error>> {
        std::future::poll_fn(|cx| Pin::new(&mut *corps).poll_frame(cx)).await
    }

    /// Spec du passe-plat : envelopper le corps ne coûte ni une frame ni un en-tête de
    /// fin.
    #[tokio::test]
    async fn le_corps_enveloppe_reste_un_passe_plat() {
        let mut corps = SpannedBody {
            inner: Body::new(CorpsAvecTrailers(0)),
            span: tracing::Span::none(),
        };
        let donnee = frame(&mut corps).await.unwrap().unwrap();
        assert_eq!(donnee.into_data().unwrap(), Bytes::from_static(b"abc"));
        let fin = frame(&mut corps).await.unwrap().unwrap();
        assert_eq!(fin.into_trailers().unwrap()["x-somme"], "42");
        assert!(frame(&mut corps).await.is_none(), "fin de flux");

        let fixe = SpannedBody {
            inner: Body::from("abc"),
            span: tracing::Span::none(),
        };
        assert_eq!(fixe.size_hint().exact(), Some(3));
        assert!(!fixe.is_end_stream());
    }

    /// The cookie wins, then the country Cloudflare reads, then France; a code
    /// the corpus does not hold, `XX` and Tor's `T1` included, is passed over.
    #[test]
    fn a_reader_goes_to_the_jurisdiction_they_chose_else_their_country() {
        let known: Vec<lj_dtos::ReaderJurisdiction> = ["fr", "ch", "be"]
            .iter()
            .map(|c| lj_dtos::ReaderJurisdiction {
                code: c.to_string(),
                label: c.to_string(),
                seats: Vec::new(),
            })
            .collect();
        let cases = [
            (Some("ch"), Some("BE"), "ch"),
            (Some("zz"), Some("BE"), "be"),
            (Some("CH"), None, "fr"),
            (None, Some("CH"), "ch"),
            (None, Some("US"), "fr"),
            (None, Some("XX"), "fr"),
            (None, Some("T1"), "fr"),
            (Some(""), Some(""), "fr"),
            (None, None, "fr"),
        ];
        for (cookie, country, expected) in cases {
            assert_eq!(
                reader_jurisdiction(cookie, country, &known),
                expected,
                "{cookie:?} {country:?}"
            );
        }
    }

    #[test]
    fn une_ancienne_adresse_nomme_la_nouvelle() {
        let cases = [
            ("/decisions", Some("/decision-search")),
            ("/recherche-textes", Some("/norm-search")),
            ("/annonces.rss", Some("/journal-search.rss")),
            ("/annonces/departement/2A", Some("/journal-index/fr/2A")),
            ("/notice-search", Some("/journal-search")),
            ("/notice-search.ics", Some("/journal-search.ics")),
            ("/notice-index/fr/2A", Some("/journal-index/fr/2A")),
            ("/journal-index/fr/2A", Some("/journal-index/fr/2A")),
            ("/marche/boamp-25-1", Some("/notice/fr/boamp-25-1")),
            (
                "/entite/siren/552081317",
                Some("/entity/fr/siren/552081317"),
            ),
            ("/entite/xx/1", None),
            ("/annuaire/cabinets", Some("/entity-index/fr/cabinets")),
            ("/juridictions", Some("/decision-index/fr")),
            (
                "/corpus/2020-01-01",
                Some("/norm-index/fr/in-force/2020-01-01"),
            ),
            ("/codes", Some("/norm-index/fr/codes")),
            (
                "/normes/textes-ue/2019",
                Some("/norm-index/eu/textes-ue/2019"),
            ),
            ("/normes/lois", Some("/norm-index/fr/lois")),
            ("/norm-index", Some("/norm-index/fr")),
            ("/decision-search", None),
            ("/norm-index/fr", None),
            (
                "/entity/de/siren/552081317",
                Some("/entity/fr/siren/552081317"),
            ),
            (
                "/entity/fr/siren/552081317",
                Some("/entity/fr/siren/552081317"),
            ),
            ("/notice/eu/boamp-25-1", Some("/notice/fr/boamp-25-1")),
            (
                "/notice/fr/jo-dj:DJ/ACCORD-0",
                Some("/notice/dj/jo-dj:DJ/ACCORD-0"),
            ),
            (
                "/notice/ch/ro:RO/2003/260",
                Some("/notice/ch/ro:RO/2003/260"),
            ),
            ("/journal-index/be/75", Some("/journal-index/fr/75")),
            ("/norm-index/de/in-force", Some("/norm-index/fr/in-force")),
        ];
        for (old, new) in cases {
            assert_eq!(renamed_path(old).as_deref(), new, "{old}");
        }
    }
}
