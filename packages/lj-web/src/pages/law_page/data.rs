//! Chargement de données des pages /texte (Resources). Calqué sur
//! `pages::decision_page::data` : fetchers via l'`ApiClient`, erreurs
//! sérialisables, `sendable` aiguillé par cible.

use leptos::prelude::*;
use lj_dtos::{
    CitingDecisionHit, CoCitedArticle, LawArticleResponse, LawSectionItem, LawSectionResponse,
    TocEntry, TocNode,
};
use serde::{Deserialize, Serialize};

use crate::api::{ApiClient, PageParams};

pub use crate::pages::decision_page::data::{sendable, PageError};

/// Limite de décisions citantes affichées sous l'article (première page).
const CITING_LIMIT: u32 = 20;

/// Clé d'article LEGI extraite de la route `/texte/{code}/{num}[/{date}]`.
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

/// Charge les décisions citantes (non bloquant, streamé via `<Suspense>`).
/// `date` = version servie (bornage à sa fenêtre de validité côté API). Ne
/// rejette jamais : erreur repliée en `error`.
pub async fn fetch_citing(code: String, num: String, date: Option<String>) -> CitingResult {
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
    match client()
        .fetch_legi_citing(&code, &num, date.as_deref(), page)
        .await
    {
        Ok(hits) => CitingResult { hits, error: None },
        Err(err) => CitingResult {
            hits: Vec::new(),
            error: Some(err.message),
        },
    }
}

/// Articles co-cités résolus (« souvent cité avec », Phase D). Ne rejette
/// jamais : l'erreur est repliée en silence (liste vide) — le bloc est un
/// enrichissement, pas un contenu porteur.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedResult {
    pub items: Vec<CoCitedArticle>,
}

/// Charge les articles co-cités (non bloquant, streamé via `<Suspense>`).
pub async fn fetch_related(code: String, num: String) -> RelatedResult {
    if code.trim().is_empty() || num.trim().is_empty() {
        return RelatedResult { items: Vec::new() };
    }
    match client().fetch_legi_related(&code, &num).await {
        Ok(items) => RelatedResult { items },
        Err(_) => RelatedResult { items: Vec::new() },
    }
}

/// Table des matières d'un code résolue. `tree` = arbre structurel réel
/// (ADR 0207), prime sur `entries` (repli à plat) ; `reading` = vue-lecture
/// intégrale des textes courts (corps joints), prime sur les deux. Ne rejette
/// jamais : l'erreur est repliée en `error` (parité `CitingResult`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TocResult {
    pub entries: Vec<TocEntry>,
    pub tree: Vec<TocNode>,
    pub reading: Vec<LawSectionItem>,
    pub error: Option<String>,
}

/// Charge la table des matières d'un code (non bloquant, streamé via `<Suspense>`).
/// `date` = consultation Chronolégi (`?date=`, ADR 0193 §5), absente = en
/// vigueur. Ne rejette jamais : erreur repliée en `error`.
pub async fn fetch_toc(code: String, date: Option<String>) -> TocResult {
    if code.trim().is_empty() {
        return TocResult {
            entries: Vec::new(),
            tree: Vec::new(),
            reading: Vec::new(),
            error: None,
        };
    }
    match client().fetch_code_toc(&code, date.as_deref()).await {
        Ok(resp) => TocResult {
            entries: resp.entries,
            tree: resp.tree,
            reading: resp.reading,
            error: None,
        },
        Err(err) => TocResult {
            entries: Vec::new(),
            tree: Vec::new(),
            reading: Vec::new(),
            error: Some(err.message),
        },
    }
}

/// Charge la vue-lecture d'une section (`/texte/{code}/section/{cid}`, ADR 0207).
/// `date` = consultation Chronolégi (`?date=`). Bloquant SSR (SEO).
pub async fn fetch_section(
    code: String,
    cid: String,
    date: Option<String>,
) -> Result<LawSectionResponse, PageError> {
    if code.trim().is_empty() || cid.trim().is_empty() {
        return Err(PageError {
            status: 400,
            message: "Référence de section invalide".to_string(),
        });
    }
    let section = client()
        .fetch_law_section(&code, &cid, date.as_deref())
        .await?;
    Ok(section)
}

/// Lit la date Chronolégi optionnelle (`?date=YYYY-MM-DD`) de l'URL courante.
pub fn chrono_date() -> Signal<Option<String>> {
    let query = leptos_router::hooks::use_query_map();
    Signal::derive(move || query.read().get("date").filter(|d| !d.is_empty()))
}

/// Lit les segments `code`/`cid` de la route `/texte/{code}/section/{cid}`.
pub fn section_key() -> Signal<(String, String)> {
    let params = leptos_router::hooks::use_params_map();
    Signal::derive(move || {
        let p = params.read();
        (
            p.get("code").unwrap_or_default(),
            p.get("cid").unwrap_or_default(),
        )
    })
}

/// Charge le sommaire d'un code (`/texte/{code}`). Bloquant SSR (SEO).
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

/// Clé du comparateur de versions `/texte/{code}/{num}/comparer/{de}/{a}`
/// (ADR 0193). `de`/`a` = dates ISO de fenêtre de version (`initiale` = borne
/// ouverte).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompareKey {
    pub code: String,
    pub num: String,
    pub de: String,
    pub a: String,
}

/// Lit les segments de la route du comparateur.
pub fn compare_key() -> Signal<CompareKey> {
    let params = leptos_router::hooks::use_params_map();
    Signal::derive(move || {
        let p = params.read();
        CompareKey {
            code: p.get("code").unwrap_or_default(),
            num: p.get("num").unwrap_or_default(),
            de: p.get("de").unwrap_or_default(),
            a: p.get("a").unwrap_or_default(),
        }
    })
}

/// Charge la comparaison de deux versions (bloquant SSR). Le
/// `LawCompareResponse` porte la timeline pour les sélecteurs — un seul appel.
pub async fn fetch_compare(key: CompareKey) -> Result<lj_dtos::LawCompareResponse, PageError> {
    if key.code.trim().is_empty() || key.num.trim().is_empty() {
        return Err(PageError {
            status: 400,
            message: "Référence d'article invalide".to_string(),
        });
    }
    let cmp = client()
        .fetch_legi_compare(&key.code, &key.num, &key.de, &key.a)
        .await?;
    Ok(cmp)
}

/// Lit les segments `code`/`num`/`date` de la route `/texte/{code}/{num}[/{date}]`.
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

/// Lit le segment `code` de la route `/texte/{code}`.
pub fn code_param() -> Signal<String> {
    let params = leptos_router::hooks::use_params_map();
    Signal::derive(move || params.read().get("code").unwrap_or_default())
}
