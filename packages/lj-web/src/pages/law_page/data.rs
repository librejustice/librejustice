//! Chargement de données des pages /texte (Resources) : fetchers via
//! l'`ApiClient`, erreurs sérialisables, `sendable` aiguillé par cible.

use leptos::prelude::*;
use lj_dtos::{
    CitingDecisionHit, CoCitedArticle, LawArticleResponse, LawSectionItem, LawSectionResponse,
    TocEntry, TocNode,
};
use serde::{Deserialize, Serialize};

use crate::api::{ApiClient, PageParams};

pub use crate::pages::decision_page::data::{sendable, PageError};

/// Limite de décisions citantes affichées sous un article ou une division ; la
/// profondeur passe par `/decision-search?legalArticle=…`.
const CITING_LIMIT: u32 = 5;

/// Clé d'article LEGI extraite de la route `/norm/{juris}/{code}/{num}[/{date}]`.
/// `juris` names the jurisdiction that enacts, `place` the reader's place
/// (`?place=PF`), absent for the default one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleKey {
    pub juris: String,
    pub code: String,
    pub num: String,
    pub date: Option<String>,
    pub place: Option<String>,
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
        .fetch_legi_article(
            &key.code,
            &key.num,
            key.date.as_deref(),
            key.place.as_deref(),
            &key.juris,
        )
        .await?;
    Ok(article)
}

/// The place vocabulary of the picker. Never rejects: an error leaves
/// the picker empty.
pub async fn fetch_places() -> Vec<lj_dtos::PlaceNode> {
    client()
        .fetch_places()
        .await
        .map(|r| r.places)
        .unwrap_or_default()
}

/// The reader's place of the current URL (`?place=PF`).
pub fn reader_place() -> Signal<Option<String>> {
    let key = article_key();
    Signal::derive(move || key.get().place)
}

/// The query carrying the reader's place onto an internal href, empty for the
/// default place so that one reading has one URL.
pub fn reader_suffix(place: Option<&str>) -> String {
    match place {
        Some(l) if l != lj_dtos::DEFAULT_READER_PLACE => format!("?place={l}"),
        _ => String::new(),
    }
}

/// Charge les décisions citantes (non bloquant, streamé via `<Suspense>`).
/// `date` = version servie (bornage à sa fenêtre de validité côté API). Ne
/// rejette jamais : erreur repliée en `error`.
pub async fn fetch_citing(
    juris: String,
    code: String,
    num: String,
    date: Option<String>,
    place: Option<String>,
) -> CitingResult {
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
        .fetch_legi_citing(&code, &num, date.as_deref(), place.as_deref(), &juris, page)
        .await
    {
        Ok(hits) => CitingResult { hits, error: None },
        Err(err) => CitingResult {
            hits: Vec::new(),
            error: Some(err.message),
        },
    }
}

/// Charge les décisions citant une division (non bloquant, streamé via
/// `<Suspense>`) : union du sous-arbre. `date` = consultation Chronolégi, qui
/// choisit la composition de ce sous-arbre. Ne rejette jamais : erreur repliée en
/// `error`.
pub async fn fetch_section_citing(
    juris: String,
    code: String,
    path: String,
    date: Option<String>,
) -> CitingResult {
    if code.trim().is_empty() || path.trim().is_empty() {
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
        .fetch_law_section_citing(&code, &path, date.as_deref(), &juris, page)
        .await
    {
        Ok(hits) => CitingResult { hits, error: None },
        Err(err) => CitingResult {
            hits: Vec::new(),
            error: Some(err.message),
        },
    }
}

/// Articles co-cités résolus (« souvent cité avec »). Ne rejette jamais :
/// l'erreur est repliée en silence (liste vide) — le bloc est un enrichissement,
/// pas un contenu porteur.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedResult {
    pub items: Vec<CoCitedArticle>,
}

/// Charge les articles co-cités (non bloquant, streamé via `<Suspense>`).
pub async fn fetch_related(juris: String, code: String, num: String) -> RelatedResult {
    if code.trim().is_empty() || num.trim().is_empty() {
        return RelatedResult { items: Vec::new() };
    }
    match client().fetch_legi_related(&code, &num, &juris).await {
        Ok(items) => RelatedResult { items },
        Err(_) => RelatedResult { items: Vec::new() },
    }
}

/// Table des matières d'un code résolue. `tree` = arbre structurel réel, prime
/// sur `entries` (repli à plat) ; `reading` = vue-lecture intégrale des textes
/// courts (corps joints), prime sur les deux. Ne rejette jamais : l'erreur est
/// repliée en `error`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TocResult {
    pub entries: Vec<TocEntry>,
    pub tree: Vec<TocNode>,
    pub reading: Vec<LawSectionItem>,
    pub error: Option<String>,
}

/// Charge la table des matières d'un code (non bloquant, streamé via
/// `<Suspense>`). `date` = consultation Chronolégi (`?date=`), absente = en
/// vigueur. Ne rejette jamais : erreur repliée en `error`.
pub async fn fetch_toc(juris: String, code: String, date: Option<String>) -> TocResult {
    if code.trim().is_empty() {
        return TocResult {
            entries: Vec::new(),
            tree: Vec::new(),
            reading: Vec::new(),
            error: None,
        };
    }
    match client()
        .fetch_code_toc(&code, date.as_deref(), &juris)
        .await
    {
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

/// Charge la vue-lecture d'une section (`/norm/{juris}/{code}/{path}`).
/// `date` = consultation Chronolégi (`?date=`). Bloquant SSR (SEO).
pub async fn fetch_section(
    juris: String,
    code: String,
    path: String,
    date: Option<String>,
) -> Result<LawSectionResponse, PageError> {
    if code.trim().is_empty() || path.trim().is_empty() {
        return Err(PageError {
            status: 400,
            message: "Référence de section invalide".to_string(),
        });
    }
    let section = client()
        .fetch_law_section(&code, &path, date.as_deref(), &juris)
        .await?;
    Ok(section)
}

/// What an undated single segment resolves to: the article of that key, else
/// the first-level section of that path.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum ArticleOrSection {
    Article(LawArticleResponse),
    Section(LawSectionResponse),
}

/// Fetches the article, and on a 404 of an undated key the section of the same
/// segment, in one blocking resource so the page hydrates on the node it served.
pub async fn fetch_article_or_section(
    key: ArticleKey,
    section_date: Option<String>,
) -> Result<ArticleOrSection, PageError> {
    match fetch_article(key.clone()).await {
        Ok(article) => Ok(ArticleOrSection::Article(article)),
        Err(err) if err.status == 404 && key.date.is_none() => {
            fetch_section(key.juris, key.code, key.num, section_date)
                .await
                .map(ArticleOrSection::Section)
                .map_err(|mut err| {
                    if err.status == 404 {
                        err.message =
                            "Aucun article ni aucune division à cette adresse".to_string();
                    }
                    err
                })
        }
        Err(err) => Err(err),
    }
}

/// Lit la date Chronolégi optionnelle (`?date=YYYY-MM-DD`) de l'URL courante.
pub fn chrono_date() -> Signal<Option<String>> {
    let query = leptos_router::hooks::use_query_map();
    Signal::derive(move || query.read().get("date").filter(|d| !d.is_empty()))
}

/// What the segments after a text's slug name: an article, at a date or not,
/// the comparison of two of its versions, otherwise a section path. One
/// segment is an article first, the page falling back on the section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormNode {
    Article { num: String, date: Option<String> },
    Compare { num: String, de: String, a: String },
    Section(String),
}

impl NormNode {
    pub fn parse(rest: &str) -> Self {
        let segs: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
        match segs.as_slice() {
            [num] => NormNode::Article {
                num: (*num).to_string(),
                date: None,
            },
            [num, date] if is_iso_date(date) => NormNode::Article {
                num: (*num).to_string(),
                date: Some((*date).to_string()),
            },
            [num, "compare", de, a] => NormNode::Compare {
                num: (*num).to_string(),
                de: (*de).to_string(),
                a: (*a).to_string(),
            },
            _ => NormNode::Section(segs.join("/")),
        }
    }
}

fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

/// The segments after the text's slug, the route's wildcard.
pub fn norm_node() -> Memo<NormNode> {
    let params = leptos_router::hooks::use_params_map();
    Memo::new(move |_| NormNode::parse(&params.read().get("rest").unwrap_or_default()))
}

/// Reads `juris`, `code` and a section's path, the lone key of an article when the
/// page falls back to it.
pub fn section_key() -> Signal<(String, String, String)> {
    let params = leptos_router::hooks::use_params_map();
    Signal::derive(move || {
        let p = params.read();
        let path = match NormNode::parse(&p.get("rest").unwrap_or_default()) {
            NormNode::Section(path) => path,
            NormNode::Article { num, .. } | NormNode::Compare { num, .. } => num,
        };
        (
            p.get("juris").unwrap_or_default(),
            p.get("code").unwrap_or_default(),
            path,
        )
    })
}

/// Charge le sommaire d'un code (`/norm/{juris}/{code}`). Bloquant SSR (SEO).
pub async fn fetch_code_summary(
    juris: String,
    code: String,
) -> Result<lj_dtos::LawCodeSummary, PageError> {
    if code.trim().is_empty() {
        return Err(PageError {
            status: 400,
            message: "Code invalide".to_string(),
        });
    }
    let summary = client().fetch_legi_code_summary(&code, &juris).await?;
    Ok(summary)
}

/// Clé du comparateur `/norm/{juris}/{code}/{num}/compare/{from}/{to}`.
/// `de`/`a` = dates ISO de fenêtre de version (`initiale` = borne ouverte).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompareKey {
    pub juris: String,
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
        let (num, de, a) = match NormNode::parse(&p.get("rest").unwrap_or_default()) {
            NormNode::Compare { num, de, a } => (num, de, a),
            _ => Default::default(),
        };
        CompareKey {
            juris: p.get("juris").unwrap_or_default(),
            code: p.get("code").unwrap_or_default(),
            num,
            de,
            a,
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
        .fetch_legi_compare(&key.code, &key.num, &key.de, &key.a, &key.juris)
        .await?;
    Ok(cmp)
}

/// Lit les segments `code`/`num`/`date` de la route `/norm/{juris}/{code}/{num}[/{date}]`.
/// A memo rather than a derived signal: leaving the article disposes it, and the
/// view's pending effects then have no live source to rerun on.
pub fn article_key() -> Memo<ArticleKey> {
    let params = leptos_router::hooks::use_params_map();
    let query = leptos_router::hooks::use_query_map();
    Memo::new(move |_| {
        let p = params.read();
        let (num, date) = match NormNode::parse(&p.get("rest").unwrap_or_default()) {
            NormNode::Article { num, date } => (num, date),
            _ => Default::default(),
        };
        ArticleKey {
            juris: p.get("juris").unwrap_or_default(),
            code: p.get("code").unwrap_or_default(),
            num,
            date,
            place: query.read().get("place").filter(|l| !l.is_empty()),
        }
    })
}

/// Lit les segments `juris` et `code` de la route `/norm/{juris}/{code}`.
pub fn code_param() -> Signal<(String, String)> {
    let params = leptos_router::hooks::use_params_map();
    Signal::derive(move || {
        let p = params.read();
        (
            p.get("juris").unwrap_or_default(),
            p.get("code").unwrap_or_default(),
        )
    })
}
