use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use lj_dtos::{
    ActivitySource, EntitySearchReport, LandingReport, ReadKind, ReadReport, SearchContext,
    SearchEngine, UiAction,
};
use opentelemetry::KeyValue;

use crate::auth::OptionalUser;
use crate::ratelimit::Cost;
use crate::state::AppState;
use crate::usage::Caller;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Page,
    App,
    Api,
    Mcp,
}

impl Surface {
    fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::App => "app",
            Self::Api => "api",
            Self::Mcp => "mcp",
        }
    }
}

fn count(kind: ReadKind, surface: Surface, provenance: &'static str) {
    lj_telemetry::meter()
        .u64_counter("librejustice.reads")
        .with_description("Lectures de décisions et de textes, par forme, surface et provenance.")
        .build()
        .add(
            1,
            &[
                KeyValue::new("kind", kind.as_str()),
                KeyValue::new("surface", surface.as_str()),
                KeyValue::new("provenance", provenance),
            ],
        );
}

pub fn served(kind: ReadKind, surface: Surface, caller: &Caller, reference: &str, document: &str) {
    if !caller.first_party {
        count(kind, surface, "direct");
    }
    caller.record_read(kind, reference, document_path(document));
}

fn browser(state: &AppState, user: Option<&str>, headers: &HeaderMap) -> Caller {
    let identity = lj_identity::Identity::resolve(user, headers);
    Caller::http(
        ActivitySource::Web,
        user.is_some(),
        SearchContext::User,
        headers,
        &state.settings,
    )
    .pseudonymise(&identity, state.pseudonymiser.as_ref(), headers)
}

async fn within_budget(state: &AppState, headers: &HeaderMap, cost: Cost) -> bool {
    let budget_key = crate::ratelimit::caller_key(None, headers);
    state.budgets.take(cost, budget_key.as_ref()).await != crate::ratelimit::Verdict::Exhausted
}

pub async fn report(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
    headers: HeaderMap,
    Json(report): Json<ReadReport>,
) -> StatusCode {
    if !within_budget(&state, &headers, Cost::ReadReport).await {
        return StatusCode::TOO_MANY_REQUESTS;
    }
    let caller = browser(&state, user.as_deref(), &headers);
    caller.record_read(
        report.kind,
        &report.reference,
        document_path(&report.document),
    );
    if caller.first_party {
        return StatusCode::NO_CONTENT;
    }
    match report.landing {
        true => {
            let own = own_host(&state.settings.web_base_url);
            let from = provenance(
                report.referrer.as_deref(),
                report.utm_source.as_deref(),
                &own,
            );
            count(report.kind, Surface::Page, from);
        }
        false => count(report.kind, Surface::App, "internal"),
    }
    StatusCode::NO_CONTENT
}

pub async fn report_entity_search(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
    headers: HeaderMap,
    Json(report): Json<EntitySearchReport>,
) -> StatusCode {
    if !within_budget(&state, &headers, Cost::ReadReport).await {
        return StatusCode::TOO_MANY_REQUESTS;
    }
    browser(&state, user.as_deref(), &headers).record_search(
        SearchEngine::Entites,
        report.query.trim(),
        crate::usage::Shape {
            page: 1,
            filters: "",
        },
        None,
    );
    StatusCode::NO_CONTENT
}

pub async fn report_landing(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
    headers: HeaderMap,
    Json(report): Json<LandingReport>,
) -> StatusCode {
    if !within_budget(&state, &headers, Cost::LandingReport).await {
        return StatusCode::TOO_MANY_REQUESTS;
    }
    let referrer = report.referrer.as_deref().and_then(host_of);
    browser(&state, user.as_deref(), &headers).record_landing(
        page_kind(&report.path),
        referrer.as_deref(),
        report.utm_source.as_deref().and_then(utm_value).as_deref(),
        report
            .utm_campaign
            .as_deref()
            .and_then(utm_value)
            .as_deref(),
    );
    StatusCode::NO_CONTENT
}

pub async fn report_action(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
    headers: HeaderMap,
    Path(action): Path<UiAction>,
) -> StatusCode {
    if !within_budget(&state, &headers, Cost::ActionReport).await {
        return StatusCode::TOO_MANY_REQUESTS;
    }
    browser(&state, user.as_deref(), &headers).record_action(action);
    StatusCode::NO_CONTENT
}

fn page_kind(path: &str) -> &'static str {
    match path
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or_default()
    {
        "" => "home",
        "decision" => "decision",
        "norm" => "norm",
        "entity" => "entity",
        "notice" => "notice",
        "decision-index" | "norm-index" | "notice-index" | "entity-index" => "index",
        "decision-search" | "norm-search" | "notice-search" | "entity-search" => "search",
        "login" | "reset-password" | "confirm" | "login-link" | "email-change"
        | "authorize-mcp" => "auth",
        "profile" | "bookmarks" => "account",
        "tools" | "redact" => "tool",
        "sources" | "feedback" | "api-guide" | "mcp-guide" | "legal-notice" | "privacy"
        | "terms" | "open-source" => "info",
        _ => "unknown",
    }
}

fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
}

fn utm_value(raw: &str) -> Option<String> {
    let value = raw.trim().to_ascii_lowercase();
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    (!value.is_empty() && value.len() <= 40 && value.chars().all(allowed)).then_some(value)
}

fn document_path(url: &str) -> &str {
    let path = match url.split_once("://") {
        Some((_, rest)) => rest.find('/').map_or("", |i| &rest[i..]),
        None => url,
    };
    path.split(['?', '#']).next().unwrap_or_default()
}

fn own_host(web_base_url: &str) -> String {
    url::Url::parse(web_base_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .unwrap_or_default()
}

fn provenance(referrer: Option<&str>, utm_source: Option<&str>, own_host: &str) -> &'static str {
    if let Some(utm) = utm_source.map(str::trim).filter(|u| !u.is_empty()) {
        return origin_of(&utm.to_ascii_lowercase()).unwrap_or("other");
    }
    let Some(host) = referrer.and_then(host_of) else {
        return "direct";
    };
    if host == own_host || host.ends_with(&format!(".{own_host}")) {
        return "internal";
    }
    origin_of(&host).unwrap_or("other")
}

const ASSISTANTS: [(&str, &str); 7] = [
    ("chatgpt.com", "chatgpt"),
    ("chat.openai.com", "chatgpt"),
    ("claude.ai", "claude"),
    ("perplexity.ai", "perplexity"),
    ("gemini.google.com", "gemini"),
    ("copilot.microsoft.com", "copilot"),
    ("chat.mistral.ai", "mistral"),
];

const ENGINES: [&str; 8] = [
    "google",
    "bing.com",
    "duckduckgo.com",
    "qwant.com",
    "ecosia.org",
    "search.yahoo.com",
    "search.brave.com",
    "yandex",
];

fn origin_of(name: &str) -> Option<&'static str> {
    let name = name.strip_prefix("www.").unwrap_or(name);
    let under = |domain: &str| name == domain || name.ends_with(&format!(".{domain}"));
    if let Some((_, label)) = ASSISTANTS
        .iter()
        .find(|(domain, label)| under(domain) || name == *label)
    {
        return Some(label);
    }
    let labels: Vec<&str> = name.split('.').collect();
    ENGINES
        .iter()
        .any(|engine| match engine.contains('.') {
            true => under(engine),
            false => labels.contains(engine),
        })
        .then_some("search_engine")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn de(referrer: &str) -> &'static str {
        provenance(Some(referrer), None, "librejustice.fr")
    }

    #[test]
    fn un_assistant_se_reconnait_a_son_domaine() {
        assert_eq!(de("https://chatgpt.com/"), "chatgpt");
        assert_eq!(de("https://chat.openai.com/c/abc"), "chatgpt");
        assert_eq!(de("https://claude.ai/chat/123"), "claude");
        assert_eq!(de("https://www.perplexity.ai/search?q=x"), "perplexity");
        assert_eq!(de("https://chat.mistral.ai/"), "mistral");
    }

    #[test]
    fn gemini_n_est_pas_le_moteur_de_google() {
        assert_eq!(de("https://gemini.google.com/app"), "gemini");
        assert_eq!(de("https://www.google.fr/"), "search_engine");
        assert_eq!(
            de("android-app://com.google.android.googlequicksearchbox/"),
            "search_engine"
        );
    }

    #[test]
    fn un_moteur_se_reconnait_a_son_domaine() {
        assert_eq!(de("https://www.bing.com/"), "search_engine");
        assert_eq!(de("https://duckduckgo.com/"), "search_engine");
        assert_eq!(de("https://www.qwant.com/?q=x"), "search_engine");
    }

    #[test]
    fn un_domaine_qui_contient_un_nom_ne_le_prend_pas() {
        assert_eq!(de("https://notclaude.ai/"), "other");
        assert_eq!(de("https://bing.com.example.org/"), "other");
    }

    #[test]
    fn le_site_lui_meme_est_interne() {
        assert_eq!(
            de("https://librejustice.fr/decision-search?q=bail"),
            "internal"
        );
        assert_eq!(de("https://www.librejustice.fr/"), "internal");
    }

    #[test]
    fn sans_referer_l_arrivee_est_directe() {
        assert_eq!(provenance(None, None, "librejustice.fr"), "direct");
        assert_eq!(
            provenance(Some("n'importe quoi"), None, "librejustice.fr"),
            "direct"
        );
    }

    #[test]
    fn le_type_de_page_se_lit_sur_le_premier_segment() {
        assert_eq!(page_kind("/"), "home");
        assert_eq!(page_kind("/decision/fr/W2MaOjgp4jgf"), "decision");
        assert_eq!(page_kind("/norm/fr/code-civil/1240"), "norm");
        assert_eq!(page_kind("/decision-index/fr/ca_rennes/2004"), "index");
        assert_eq!(page_kind("/norm-search"), "search");
        assert_eq!(page_kind("/authorize-mcp"), "auth");
        assert_eq!(page_kind("/login"), "auth");
        assert_eq!(
            page_kind("/legislation/article-222-7-du-code-penal"),
            "unknown"
        );
    }

    #[test]
    fn un_document_se_nomme_par_son_chemin() {
        assert_eq!(
            document_path("https://librejustice.fr/decision/fr/W2MaOjgp4jgf"),
            "/decision/fr/W2MaOjgp4jgf"
        );
        assert_eq!(
            document_path("https://librejustice.fr/norm/fr/code-civil/1240?date=2020-01-01#al2"),
            "/norm/fr/code-civil/1240"
        );
        assert_eq!(
            document_path("/norm/fr/code-civil/1240"),
            "/norm/fr/code-civil/1240"
        );
        assert_eq!(document_path(""), "");
    }

    #[test]
    fn un_utm_hors_de_son_alphabet_est_ecarte() {
        assert_eq!(utm_value(" ChatGPT.com "), Some("chatgpt.com".to_string()));
        assert_eq!(utm_value("campagne-2"), Some("campagne-2".to_string()));
        assert_eq!(utm_value("jean.dupont@exemple.fr"), None);
        assert_eq!(utm_value(&"a".repeat(41)), None);
        assert_eq!(utm_value("  "), None);
    }

    #[test]
    fn l_utm_prime_sur_le_referer() {
        let de_utm = |utm| {
            provenance(
                Some("https://www.google.com/"),
                Some(utm),
                "librejustice.fr",
            )
        };
        assert_eq!(de_utm("chatgpt.com"), "chatgpt");
        assert_eq!(de_utm("perplexity"), "perplexity");
        assert_eq!(de_utm("newsletter"), "other");
        assert_eq!(de_utm("  "), "search_engine");
    }
}
