//! Chargement de données des pages /loi (Resources). Calqué sur
//! `pages::decision_page::data` : fetchers via l'`ApiClient`, erreurs
//! sérialisables, `sendable` aiguillé par cible.

use leptos::prelude::*;
use lj_dtos::{CitingDecisionHit, LawArticleResponse, TocEntry};
use serde::{Deserialize, Serialize};

use crate::api::{ApiClient, PageParams};

pub use crate::pages::decision_page::data::{sendable, PageError};

/// Limite de décisions citantes affichées sous l'article (première page).
const CITING_LIMIT: u32 = 20;

/// Clé d'article LEGI extraite de la route `/loi/{code}/{num}[/{date}]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleKey {
    pub code: String,
    pub num: String,
    pub date: Option<String>,
}

/// Décisions citantes résolues. Ne rejette jamais : l'erreur est repliée en
/// `error` (parité `SimilarResult`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CitingResult {
    pub hits: Vec<CitingDecisionHit>,
    pub error: Option<String>,
}

fn client() -> ApiClient {
    ApiClient::from_context()
}

/// Charge l'article à la date (ou en vigueur) + sa timeline (bloquant SSR pour le
/// SEO). Le `LawArticleResponse` porte déjà la timeline (`versions`), donc un
/// seul appel suffit pour l'en-tête, le corps et la timeline.
pub async fn fetch_article(key: ArticleKey) -> Result<LawArticleResponse, PageError> {
    if key.code.trim().is_empty() || key.num.trim().is_empty() {
        return Err(PageError {
            status: 400,
            message: "Référence d'article invalide".to_string(),
        });
    }
    let article = client()
        .fetch_legi_article(&key.code, &key.num, key.date.as_deref())
        .await?;
    Ok(article)
}

/// Charge les décisions citantes (non bloquant, streamé via `<Suspense>`). Ne
/// rejette jamais : erreur repliée en `error`.
pub async fn fetch_citing(code: String, num: String) -> CitingResult {
    if code.trim().is_empty() || num.trim().is_empty() {
        return CitingResult {
            hits: Vec::new(),
            error: None,
        };
    }
    let page = PageParams {
        limit: CITING_LIMIT,
        offset: 0,
    };
    match client().fetch_legi_citing(&code, &num, page).await {
        Ok(hits) => CitingResult { hits, error: None },
        Err(err) => CitingResult {
            hits: Vec::new(),
            error: Some(err.message),
        },
    }
}

/// Table des matières d'un code résolue. Ne rejette jamais : l'erreur est repliée
/// en `error` (parité `CitingResult`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TocResult {
    pub entries: Vec<TocEntry>,
    pub error: Option<String>,
}

/// Charge la table des matières d'un code (non bloquant, streamé via `<Suspense>`).
/// Ne rejette jamais : erreur repliée en `error`.
pub async fn fetch_toc(code: String) -> TocResult {
    if code.trim().is_empty() {
        return TocResult {
            entries: Vec::new(),
            error: None,
        };
    }
    match client().fetch_code_toc(&code).await {
        Ok(resp) => TocResult {
            entries: resp.entries,
            error: None,
        },
        Err(err) => TocResult {
            entries: Vec::new(),
            error: Some(err.message),
        },
    }
}

/// Charge le sommaire d'un code (`/loi/{code}`). Bloquant SSR (SEO).
pub async fn fetch_code_summary(code: String) -> Result<lj_dtos::LawCodeSummary, PageError> {
    if code.trim().is_empty() {
        return Err(PageError {
            status: 400,
            message: "Code invalide".to_string(),
        });
    }
    let summary = client().fetch_legi_code_summary(&code).await?;
    Ok(summary)
}

/// Lit les segments `code`/`num`/`date` de la route `/loi/{code}/{num}[/{date}]`.
pub fn article_key() -> Signal<ArticleKey> {
    let params = leptos_router::hooks::use_params_map();
    Signal::derive(move || {
        let p = params.read();
        ArticleKey {
            code: p.get("code").unwrap_or_default(),
            num: p.get("num").unwrap_or_default(),
            date: p.get("date").filter(|d| !d.is_empty()),
        }
    })
}

/// Lit le segment `code` de la route `/loi/{code}`.
pub fn code_param() -> Signal<String> {
    let params = leptos_router::hooks::use_params_map();
    Signal::derive(move || params.read().get("code").unwrap_or_default())
}
