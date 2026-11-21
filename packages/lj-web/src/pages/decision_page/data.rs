//! Chargement de données de la page décision (Resources).

use leptos::prelude::*;
use lj_dtos::{DecisionDetail, DecisionPartyDto, SimilarDecisionHit};
use serde::{Deserialize, Serialize};

use crate::api::{ApiClient, ApiError, Failure};
use crate::i18n::Lang;

/// A page's error, streamed by its `Resource` from the server to the browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageError {
    pub status: u16,
    pub failure: Failure,
}

impl From<ApiError> for PageError {
    fn from(err: ApiError) -> Self {
        Self {
            status: err.status,
            failure: err.failure,
        }
    }
}

/// Voisins résolus, deux volets (principe / espèce). Ne porte JAMAIS d'erreur
/// en rejet — l'erreur est repliée en `error: Option<String>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarResult {
    pub principe: Vec<SimilarDecisionHit>,
    pub espece: Vec<SimilarDecisionHit>,
    pub abstained: bool,
    pub error: Option<crate::api::Failure>,
}

fn client() -> ApiClient {
    ApiClient::from_context()
}

/// Charge le détail (bloquant SSR pour le SEO). Le résumé est garanti en base et
/// porté par le détail, donc présent dans le HTML SSR (description / synthèse)
/// sans génération à la volée.
pub async fn fetch_detail(id: String, lang: Lang) -> Result<DecisionDetail, PageError> {
    if id.trim().is_empty() {
        return Err(PageError {
            status: 400,
            failure: Failure::InvalidId,
        });
    }
    let mut detail = client().fetch_decision(&id, Some(lang.code())).await?;
    detail.title = crate::helpers::retitle(
        &lang.ui().dates,
        &detail.title,
        detail.date_lecture.as_deref(),
    );
    if let Some(seat) = detail
        .seat_parts
        .as_ref()
        .and_then(lang.ui().referential.seat)
    {
        if let Some(french) = detail.seat.replace(seat.clone()) {
            detail.title = detail
                .title
                .replacen(&format!(", {french}"), &format!(", {seat}"), 1);
        }
    }
    if let Some(named) = detail.court_names.get(lang.code()).cloned() {
        use crate::helpers::renamed;
        detail.title = renamed(&named, &detail.title, detail.jurisdiction_name.as_deref());
        detail.seat = detail.seat.map(|seat| renamed(&named, &seat, None));
        detail.jurisdiction_name = Some(named.name);
    }
    for reference in detail.legal_references.iter_mut().flatten() {
        crate::i18n::relabel_title(lang, &mut reference.instrument);
    }
    for target in detail
        .paragraph_spans
        .iter_mut()
        .flatten()
        .flat_map(|span| span.targets.iter_mut())
    {
        crate::i18n::relabel_title(lang, &mut target.label);
    }
    Ok(detail)
}

/// Charge les voisins (non bloquant, streamé via `<Suspense>`). Ne rejette
/// jamais : erreur repliée en `error`.
pub async fn fetch_similar(id: String, lang: Lang) -> SimilarResult {
    if id.trim().is_empty() {
        return SimilarResult {
            principe: Vec::new(),
            espece: Vec::new(),
            abstained: false,
            error: None,
        };
    }
    match client().fetch_similar_decisions(&id, lang.code()).await {
        Ok(response) => SimilarResult {
            principe: response.principe,
            espece: response.espece,
            abstained: response.abstained,
            error: None,
        },
        Err(err) => SimilarResult {
            principe: Vec::new(),
            espece: Vec::new(),
            abstained: false,
            error: Some(err.failure),
        },
    }
}

/// Acteurs (parties + conseils) d'une décision, encart « Parties ». Non
/// bloquant, streamé ; ne rejette jamais — erreur repliée en `error`. Encart
/// masqué si la liste est vide.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartiesResult {
    pub parties: Vec<DecisionPartyDto>,
    pub error: Option<crate::api::Failure>,
}

/// Charge les parties (streamé via `<Suspense>`). Erreur repliée en `error`.
pub async fn fetch_parties(id: String) -> PartiesResult {
    if id.trim().is_empty() {
        return PartiesResult {
            parties: Vec::new(),
            error: None,
        };
    }
    match client().fetch_decision_parties(&id).await {
        Ok(response) => PartiesResult {
            parties: response.parties,
            error: None,
        },
        Err(err) => PartiesResult {
            parties: Vec::new(),
            error: Some(err.failure),
        },
    }
}

/// Adapte un futur de fetch en futur `Send`, requis par `Resource::new[_blocking]`.
///
/// Côté SSR (reqwest tokio, auth no-op) le futur est déjà `Send` → identité.
/// Côté wasm (single-thread) reqwest/`fetch` et le shim auth (`JsFuture`) sont
/// `!Send` ; `SendWrapper` les rend `Send` sans risque (un seul thread).
#[cfg(feature = "ssr")]
pub fn sendable<F>(fut: F) -> F
where
    F: std::future::Future + Send,
{
    fut
}

#[cfg(feature = "hydrate")]
pub fn sendable<F>(fut: F) -> send_wrapper::SendWrapper<F>
where
    F: std::future::Future,
{
    send_wrapper::SendWrapper::new(fut)
}

/// Récupère le segment `id` de la route `/decision/:id`. Vide si absent.
pub fn decision_id() -> Signal<String> {
    let params = leptos_router::hooks::use_params_map();
    Signal::derive(move || params.read().get("id").unwrap_or_default())
}
