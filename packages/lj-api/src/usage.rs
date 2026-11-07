use lj_dtos::{ActivitySource, ReadKind, SearchContext, SearchEngine, UiAction};

pub const LOG_TARGET: &str = "lj_api::usage";

#[derive(Debug, Clone, Copy)]
pub(crate) struct Shape<'a> {
    pub page: i64,
    pub filters: &'a str,
}

#[derive(Debug, Clone)]
pub struct Caller {
    pub source: ActivitySource,
    pub authenticated: bool,
    pub context: SearchContext,
    pub first_party: bool,
    pub caller: Option<String>,
    pub visit: Option<String>,
    pub user_agent: Option<String>,
    pub request: Request,
}

#[derive(Debug, Clone, Default)]
pub struct Request {
    pub headers: Option<String>,
    pub worker: Option<String>,
}

// Added by Cloudflare or Caddy on every request, so they say nothing of the client.
const PROXY_HEADERS: [&str; 9] = [
    "cdn-loop",
    "cf-connecting-ip",
    "cf-ipcountry",
    "cf-ray",
    "cf-visitor",
    "via",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
];

impl Request {
    pub fn read(headers: &axum::http::HeaderMap) -> Self {
        let mut names: Vec<&str> = headers
            .keys()
            .map(axum::http::HeaderName::as_str)
            .filter(|n| !PROXY_HEADERS.contains(n))
            .collect();
        names.sort_unstable();
        Self {
            headers: Some(names.join(",")),
            worker: headers
                .get("cf-worker")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned),
        }
    }
}

pub fn is_first_party(headers: &axum::http::HeaderMap, settings: &crate::config::Settings) -> bool {
    settings.traffic_is_first_party()
        || internal_origin(headers, settings.internal_origin_token.as_deref())
}

pub fn user_agent(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
}

pub fn internal_origin(headers: &axum::http::HeaderMap, expected: Option<&str>) -> bool {
    let Some(expected) = expected.filter(|e| !e.is_empty()) else {
        return false;
    };
    let entete = headers
        .get(lj_dtos::INTERNAL_ORIGIN_HEADER)
        .and_then(|v| v.to_str().ok());
    if entete.is_some_and(|v| v == expected) {
        return true;
    }
    headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| crate::session::read_cookie(v, lj_dtos::INTERNAL_ORIGIN_COOKIE))
        .is_some_and(|v| v == expected)
}

impl Caller {
    pub fn web_anonymous(context: SearchContext) -> Self {
        Self {
            source: ActivitySource::Web,
            authenticated: false,
            context,
            first_party: false,
            caller: None,
            visit: None,
            user_agent: None,
            request: Request::default(),
        }
    }

    pub fn http(
        source: ActivitySource,
        authenticated: bool,
        context: SearchContext,
        headers: &axum::http::HeaderMap,
        settings: &crate::config::Settings,
    ) -> Self {
        Self {
            source,
            authenticated,
            context,
            first_party: is_first_party(headers, settings),
            caller: None,
            visit: None,
            user_agent: user_agent(headers).map(str::to_owned),
            request: match source {
                ActivitySource::Web => Request::default(),
                ActivitySource::Api | ActivitySource::Mcp => Request::read(headers),
            },
        }
    }

    pub fn pseudonymise(
        mut self,
        id: &lj_identity::Identity,
        pseudo: Option<&lj_identity::Pseudonymiser>,
        headers: &axum::http::HeaderMap,
    ) -> Self {
        let Some(pseudo) = pseudo else {
            return self;
        };
        self.caller = pseudo.caller(id);
        self.visit = pseudo.visit(
            id,
            user_agent(headers),
            lj_identity::salt_period(unix_now()),
        );
        self
    }

    pub(crate) fn record_span(&self, engine: SearchEngine, query: &str, shape: Shape<'_>) {
        let span = tracing::Span::current();
        span.record("librejustice.search.engine", engine_value(engine));
        span.record("librejustice.search.source", source_value(self.source));
        span.record(
            "librejustice.search.identity",
            identity_value(self.authenticated),
        );
        span.record("librejustice.search.context", context_value(self.context));
        span.record("librejustice.search.origin", origin_value(self.first_party));
        span.record("librejustice.search.caller", self.caller.as_deref());
        span.record("librejustice.search.visit", self.visit.as_deref());
        span.record("user_agent.original", self.user_agent.as_deref());
        span.record(
            "librejustice.request.headers",
            self.request.headers.as_deref(),
        );
        span.record(
            "librejustice.request.worker",
            self.request.worker.as_deref(),
        );
        span.record("librejustice.search.query", query);
        span.record("librejustice.search.page", shape.page);
        span.record("librejustice.search.filters", shape.filters);
    }

    pub(crate) fn record_outcome(
        &self,
        engine: SearchEngine,
        query: &str,
        shape: Shape<'_>,
        results_count: i64,
    ) {
        tracing::Span::current().record("librejustice.search.results_count", results_count);
        self.log(engine, query, shape, Some(results_count));
    }

    fn log(&self, engine: SearchEngine, query: &str, shape: Shape<'_>, results_count: Option<i64>) {
        tracing::info!(
            target: LOG_TARGET,
            engine = engine_value(engine),
            source = source_value(self.source),
            identity = identity_value(self.authenticated),
            context = context_value(self.context),
            origin = origin_value(self.first_party),
            caller = self.caller.as_deref(),
            visit = self.visit.as_deref(),
            query = query,
            page = shape.page,
            filters = shape.filters,
            results_count = results_count,
            "recherche"
        );
    }
}

impl Caller {
    pub fn record_read(&self, kind: ReadKind, reference: &str, document: &str) {
        let _span = tracing::info_span!(
            "read",
            librejustice.read.kind = kind.as_str(),
            librejustice.read.reference = reference,
            librejustice.read.document = document,
            librejustice.search.source = source_value(self.source),
            librejustice.search.identity = identity_value(self.authenticated),
            librejustice.search.origin = origin_value(self.first_party),
            librejustice.search.caller = self.caller.as_deref(),
            librejustice.search.visit = self.visit.as_deref(),
            user_agent.original = self.user_agent.as_deref(),
            librejustice.request.headers = self.request.headers.as_deref(),
            librejustice.request.worker = self.request.worker.as_deref(),
        );
    }

    pub fn record_landing(
        &self,
        page: &str,
        referrer: Option<&str>,
        utm_source: Option<&str>,
        utm_campaign: Option<&str>,
    ) {
        let _span = tracing::info_span!(
            "landing",
            librejustice.landing.page = page,
            librejustice.landing.referrer = referrer,
            librejustice.landing.utm_source = utm_source,
            librejustice.landing.utm_campaign = utm_campaign,
            librejustice.search.source = source_value(self.source),
            librejustice.search.identity = identity_value(self.authenticated),
            librejustice.search.origin = origin_value(self.first_party),
            librejustice.search.caller = self.caller.as_deref(),
            librejustice.search.visit = self.visit.as_deref(),
            user_agent.original = self.user_agent.as_deref(),
        );
    }

    pub fn record_action(&self, action: UiAction) {
        let _span = tracing::info_span!(
            "ui_action",
            librejustice.ui.action = action.as_str(),
            librejustice.search.source = source_value(self.source),
            librejustice.search.identity = identity_value(self.authenticated),
            librejustice.search.origin = origin_value(self.first_party),
            librejustice.search.caller = self.caller.as_deref(),
            librejustice.search.visit = self.visit.as_deref(),
            user_agent.original = self.user_agent.as_deref(),
        );
    }

    pub(crate) fn record_search(
        &self,
        engine: SearchEngine,
        query: &str,
        shape: Shape<'_>,
        results_count: Option<i64>,
    ) {
        let span = tracing::info_span!(
            "search",
            librejustice.search.engine = tracing::field::Empty,
            librejustice.search.source = tracing::field::Empty,
            librejustice.search.query = tracing::field::Empty,
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
        );
        let _guard = span.enter();
        self.record_span(engine, query, shape);
        if let Some(count) = results_count {
            span.record("librejustice.search.results_count", count);
        }
        self.log(engine, query, shape, results_count);
    }
}

fn unix_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

pub(crate) fn source_value(source: ActivitySource) -> &'static str {
    match source {
        ActivitySource::Web => "web",
        ActivitySource::Api => "api",
        ActivitySource::Mcp => "mcp",
    }
}

pub(crate) fn engine_value(engine: SearchEngine) -> &'static str {
    match engine {
        SearchEngine::Decisions => "decisions",
        SearchEngine::Textes => "textes",
        SearchEngine::Annonces => "annonces",
        SearchEngine::Entites => "entites",
    }
}

pub(crate) fn page_number(offset: i64, limit: i64) -> Option<i64> {
    if offset == 0 {
        return None;
    }
    Some(offset / limit + 1)
}

pub(crate) fn facet_filters(req: &lj_dtos::SearchRequest) -> serde_json::Value {
    let mut value = serde_json::to_value(req).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(obj) = value.as_object_mut() {
        obj.remove("query");
        obj.remove("limit");
        obj.remove("offset");
    }
    value
}

pub(crate) fn identity_value(authenticated: bool) -> &'static str {
    if authenticated {
        "authenticated"
    } else {
        "anonymous"
    }
}

pub(crate) fn origin_value(internal: bool) -> &'static str {
    if internal {
        "internal"
    } else {
        "external"
    }
}

pub(crate) fn context_value(context: SearchContext) -> &'static str {
    match context {
        SearchContext::User => "user",
        SearchContext::Teaser => "teaser",
    }
}

#[cfg(test)]
mod tests {
    use super::internal_origin;
    use axum::http::{HeaderMap, HeaderName, HeaderValue};

    fn entetes(nom: &str, valeur: &str) -> HeaderMap {
        let mut map = HeaderMap::new();
        map.insert(
            HeaderName::from_bytes(nom.as_bytes()).unwrap(),
            HeaderValue::from_str(valeur).unwrap(),
        );
        map
    }

    #[test]
    fn les_deux_voies_declarent() {
        let entete = entetes(lj_dtos::INTERNAL_ORIGIN_HEADER, "s3cret");
        assert!(internal_origin(&entete, Some("s3cret")));

        let cookie = entetes(
            "cookie",
            &format!("a=1; {}=s3cret; b=2", lj_dtos::INTERNAL_ORIGIN_COOKIE),
        );
        assert!(internal_origin(&cookie, Some("s3cret")));
    }

    #[test]
    fn une_mauvaise_valeur_ne_declare_pas() {
        let entete = entetes(lj_dtos::INTERNAL_ORIGIN_HEADER, "devine");
        assert!(!internal_origin(&entete, Some("s3cret")));

        let cookie = entetes(
            "cookie",
            &format!("{}=devine", lj_dtos::INTERNAL_ORIGIN_COOKIE),
        );
        assert!(!internal_origin(&cookie, Some("s3cret")));
    }

    #[test]
    fn sans_secret_attendu_rien_ne_declare() {
        let cookie = entetes("cookie", &format!("{}=", lj_dtos::INTERNAL_ORIGIN_COOKIE));
        assert!(!internal_origin(&cookie, None));
        assert!(!internal_origin(&cookie, Some("")));
    }
}
