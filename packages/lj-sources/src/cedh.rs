//! Downloader HUDOC (CEDH) — I/O réseau au bord lj-sources.
//!
//! API publique sans auth, deux endpoints :
//! - **list**: `sort` required (else 404), `length` capped, and a capped global
//!   window, hence a partition by year. HUDOC publishes one document per language
//!   version: the orchestrator groups by case and keeps the FR if any, else the
//!   EN. `select` projects server side, and the serialisation is sparse, only the
//!   non-empty keys coming back;
//! - **corps** : HTML converti du DOCX, strippé au bord.
//!
//! Le parsing métier vit dans `lj-core`. Le watermark est persisté par fond
//! façon [`crate::dila::DilaManifest`].

use crate::error::{Result, SourceError};
use crate::html_strip::strip_html;
use chrono::Utc;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_tracing::TracingMiddleware;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::time::Duration;

/// Base HUDOC.
pub const BASE_URL: &str = "https://hudoc.echr.coe.int";
/// Throttle de courtoisie entre fetchs.
pub const THROTTLE: Duration = Duration::from_millis(250);
/// Tentatives face aux transitoires HUDOC, backoff `THROTTLE << attempt`.
/// Épuisées, un corps est différé (skip) ; une page de liste est une erreur
/// franche, la sauter trouerait l'année en silence.
const CEDH_RETRIES: u32 = 4;
/// `length` per page, under the server's cap.
pub const PAGE_SIZE: usize = 200;
/// Colonnes projetées dans `select`, séparées par virgule (CSV). HUDOC
/// n'honore pas le séparateur `;` : le `select` est alors ignoré et chaque
/// résultat ne renvoie que `{rank}`, sans `itemid`. Les valeurs internes
/// multi-items (`appno`, `documentcollectionid2`…) gardent, elles, leur `;`
/// propre dans la réponse.
pub const SELECT_COLUMNS: &str = "itemid,docname,appno,extractedappno,ecli,kpdate,judgementdate,\
decisiondate,introductiondate,doctype,typedescription,documentcollectionid2,article,violation,\
nonviolation,conclusion,importance,respondent,originatingbody_name,separateopinion,kpthesaurus,\
scl,sclappnos,externalsources,languageisocode";

use crate::state_paths::CEDH_DIR;

/// Filtre HUDOC bilingue FR-prioritaire, borné à une année par `kpdate:[…]`
/// (`NOT (doctype=PR OR HFCOMOLD OR HECOMOLD)`). On ramène FR et EN ;
/// l'orchestrateur regroupe par affaire et préfère la version FR (cf. [`super`]).
fn year_query(year: i32) -> String {
    format!(
        "contentsitename=ECHR AND (NOT (doctype=PR OR doctype=HFCOMOLD OR doctype=HECOMOLD)) \
AND (languageisocode=\"FRE\" OR languageisocode=\"ENG\") \
AND (kpdate:[{year}-01-01T00:00:00.0Z TO {year}-12-31T23:59:59.0Z])"
    )
}

/// The Registry's legal summaries of the decisions dated in `year`, in French and in English.
fn legal_summaries_query(year: i32) -> String {
    format!(
        "contentsitename=ECHR AND documentcollectionid2=\"CLIN\" \
AND (kpdate:[{year}-01-01T00:00:00.0Z TO {year}-12-31T23:59:59.0Z])"
    )
}

/// 429 et 5xx de passerelle : ce que HUDOC rend sous charge et qui repasse au
/// retry. Tout autre statut est structurel.
fn is_transient(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

/// HUDOC client.
pub struct CedhClient {
    base_url: String,
    client: ClientWithMiddleware,
    /// Cache disque des corps HTML bruts (`<cache_dir>/cedh/bodies/{itemid}.html`).
    /// Posé → [`Self::body_text`] lit le cache avant tout appel réseau et y écrit
    /// les corps 200. Découple le fetch (throttlé, une fois) de l'ingest (relecture
    /// locale, sans réseau — seul l'embedding reste). `None` → pas de cache.
    cache_dir: Option<std::path::PathBuf>,
}

impl CedhClient {
    /// Client de production (base [`BASE_URL`]).
    pub fn new() -> Self {
        Self::with_base_url(BASE_URL)
    }

    /// Client sur une base donnée (`base_url` normalisée, trailing `/` retiré).
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        // No User-Agent. Cloudflare in front of HUDOC waves through what reads as an
        // HTTP library and challenges the rest.
        let client = ClientBuilder::new(
            reqwest::Client::builder()
                // Unbounded, a connection HUDOC accepts and never honours parks
                // `send().await`. Past the bound the request errors and the retry departs.
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(120))
                // A pooled connection freezes on this path.
                .pool_max_idle_per_host(0)
                .build()
                .expect("reqwest client build"),
        )
        .with(TracingMiddleware::default())
        .build();
        Self {
            base_url,
            client,
            cache_dir: None,
        }
    }

    /// Active le cache disque des corps sous `cache_dir` (`<cache_dir>/cedh/bodies/`).
    pub fn with_body_cache(mut self, cache_dir: impl Into<std::path::PathBuf>) -> Self {
        self.cache_dir = Some(cache_dir.into());
        self
    }

    /// Chemin du corps HTML brut caché pour un `itemid` (les `itemid` HUDOC sont
    /// `[0-9-]+`, sûrs comme nom de fichier).
    fn body_cache_path(&self, itemid: &str) -> Option<std::path::PathBuf> {
        self.cache_dir.as_ref().map(|d| {
            d.join(CEDH_DIR)
                .join("bodies")
                .join(format!("{itemid}.html"))
        })
    }

    /// Drops the cached body of `itemid`, which the next [`Self::body_text`] fetches again.
    pub fn forget_body(&self, itemid: &str) -> Result<()> {
        match self.body_cache_path(itemid).map(fs::remove_file) {
            Some(Err(e)) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// A page of the documents of a year, raw `{resultcount, results:[{columns}]}`.
    pub async fn results_page(&self, year: i32, start: usize) -> Result<Value> {
        self.query_page(year_query(year), start).await
    }

    /// A page of the legal summaries of a year, in the shape of [`Self::results_page`].
    pub async fn legal_summaries_page(&self, year: i32, start: usize) -> Result<Value> {
        self.query_page(legal_summaries_query(year), start).await
    }

    /// `sort` is mandatory: HUDOC answers 404 without it.
    async fn query_page(&self, query: String, start: usize) -> Result<Value> {
        let url = format!("{}/app/query/results", self.base_url);
        let params: [(&str, String); 5] = [
            ("query", query),
            ("select", SELECT_COLUMNS.to_string()),
            ("sort", "kpdate Descending".to_string()),
            ("start", start.to_string()),
            ("length", PAGE_SIZE.to_string()),
        ];
        let mut last = (0u16, String::new());
        for attempt in 0..CEDH_RETRIES {
            if attempt > 0 {
                tokio::time::sleep(THROTTLE * (1 << attempt)).await;
            }
            let envoi = self
                .client
                .get(&url)
                .header("accept", "application/json")
                .query(&params)
                .send()
                .await;
            let resp = match envoi {
                Ok(resp) => resp,
                Err(e) if attempt + 1 < CEDH_RETRIES => {
                    tracing::warn!(%url, attempt, error = %e, "hudoc results request failed (send/timeout), retry");
                    continue;
                }
                Err(e) => {
                    return Err(SourceError::Invalid(format!(
                        "requête hudoc results {url}: {e}"
                    )));
                }
            };
            let status = resp.status().as_u16();
            let text = resp
                .text()
                .await
                .map_err(|e| SourceError::Invalid(format!("corps hudoc results {url}: {e}")))?;
            if status == 200 {
                return Ok(sonic_rs::from_str(&text)?);
            }
            if !is_transient(status) {
                return Err(SourceError::Invalid(format!(
                    "hudoc results statut {status} {url}: {}",
                    text.chars().take(200).collect::<String>()
                )));
            }
            last = (status, text);
        }
        Err(SourceError::Invalid(format!(
            "hudoc results statut {} après {CEDH_RETRIES} tentatives {url}: {}",
            last.0,
            last.1.chars().take(200).collect::<String>()
        )))
    }

    /// Corps texte d'un document (`itemid`) : DOCX converti en HTML, strippé en
    /// texte. Corps 0 octet ≠ texte vide (lag de conversion) — le strip d'un corps
    /// vide rend une chaîne vide, l'ingest diffère alors la décision (re-fetch au
    /// prochain run).
    pub async fn body_text(&self, itemid: &str) -> Result<String> {
        // Cache disque : un corps déjà tiré est relu sans réseau (ni throttle). On
        // ne cache que les 200 : un 204 (lag de conversion) peut gagner son corps plus
        // tard, son absence de fichier vaut re-fetch au run suivant.
        if let Some(path) = self.body_cache_path(itemid) {
            if path.exists() {
                return Ok(strip_html(&fs::read_to_string(&path)?));
            }
        }
        let url = format!("{}/app/conversion/docx/html/body", self.base_url);
        let params: [(&str, String); 4] = [
            ("library", "ECHR".to_string()),
            ("id", itemid.to_string()),
            ("filename", format!("{itemid}.docx")),
            ("logEvent", "False".to_string()),
        ];
        // HUDOC renvoie épisodiquement des 5xx de passerelle (502/503/504) et des
        // 429 sous charge, transitoires. On retente avec backoff ; si le transitoire
        // persiste, on diffère (corps vide → skip → re-fetch au prochain sync), même
        // philosophie que le 204.
        let mut last_status = 0u16;
        for attempt in 0..CEDH_RETRIES {
            // Throttle de courtoisie au réseau seul : payé ici, après le cache — un hit
            // disque ne dort pas. `<< attempt` = backoff des retries (250 ms, 500 ms, 1 s,
            // 2 s).
            tokio::time::sleep(THROTTLE * (1 << attempt)).await;
            let envoi = self
                .client
                .get(&url)
                .header("accept", "text/html")
                .query(&params)
                .send()
                .await;
            let resp = match envoi {
                Ok(resp) => resp,
                Err(e) if attempt + 1 < CEDH_RETRIES => {
                    tracing::warn!(itemid, attempt, error = %e, "hudoc body request failed (send/timeout), retry");
                    continue;
                }
                // Même philosophie que le transitoire persistant : un corps injoignable se
                // diffère, il ne fait pas tomber l'année entière.
                Err(e) => {
                    tracing::warn!(itemid, error = %e, "hudoc body unreachable after retries (send/timeout), deferred (skip)");
                    return Ok(String::new());
                }
            };
            let status = resp.status().as_u16();
            last_status = status;
            // 204 No Content = corps DOCX pas encore converti (lag, surtout arrêts de
            // comité récents) : on rend une chaîne vide et l'ingest diffère la décision
            // (skip), comme pour un corps 0 octet.
            if status == 204 {
                return Ok(String::new());
            }
            if status == 200 {
                let raw = resp
                    .text()
                    .await
                    .map_err(|e| SourceError::Invalid(format!("corps hudoc {itemid}: {e}")))?;
                if let Some(path) = self.body_cache_path(itemid) {
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    let tmp = path.with_extension("html.tmp");
                    fs::write(&tmp, &raw)?;
                    fs::rename(&tmp, &path)?;
                }
                return Ok(strip_html(&raw));
            }
            if !is_transient(status) {
                return Err(SourceError::Invalid(format!(
                    "hudoc body statut {status} {itemid}"
                )));
            }
        }
        tracing::warn!(
            itemid,
            status = last_status,
            "hudoc body still transient after retries, deferred (skip)"
        );
        Ok(String::new())
    }
}

impl Default for CedhClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Une ligne `results` extraite : `itemid` (PK, jamais vide) + colonnes brutes.
/// `language` = `languageisocode` projeté (`"FRE"`/`"ENG"`, vide si absent) — clé
/// du choix FR-prioritaire au regroupement par affaire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CedhResult {
    pub itemid: String,
    pub doctype: String,
    pub language: String,
    pub columns: Value,
}

/// Parses a HUDOC `results` response into usable rows, dropping those without
/// `itemid`, the mandatory PK. `resultcount` comes back apart, so the orchestrator
/// knows when it has seen everything under the capped window of a year.
///
/// Le champ `rank` (score de pertinence du moteur de recherche HUDOC, hors
/// `select`, injecté dans chaque résultat) est retiré des colonnes : il varie
/// à chaque publication dans la fenêtre d'année et empoisonnerait le checksum
/// d'idempotence.
pub fn parse_results(resp: &Value) -> (u64, Vec<CedhResult>) {
    let resultcount = resp["resultcount"].as_u64().unwrap_or(0);
    let mut out = Vec::new();
    if let Some(arr) = resp["results"].as_array() {
        for item in arr {
            let columns = &item["columns"];
            let Some(itemid) = columns["itemid"].as_str() else {
                continue;
            };
            if itemid.is_empty() {
                continue;
            }
            let mut columns = columns.clone();
            if let Some(obj) = columns.as_object_mut() {
                obj.remove("rank");
            }
            out.push(CedhResult {
                itemid: itemid.to_string(),
                doctype: columns["doctype"].as_str().unwrap_or("").to_string(),
                language: columns["languageisocode"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
                columns,
            });
        }
    }
    (resultcount, out)
}

/// Watermark CEDH par fond : dernière année dont la liste a été entièrement
/// drainée. Variante CEDH du [`crate::dila::DilaManifest`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CedhManifest {
    #[serde(default)]
    pub last_year_done: Option<i32>,
    #[serde(default)]
    pub fetched_at: Option<String>,
}

impl CedhManifest {
    fn now_iso_seconds() -> String {
        Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)?;
        serde_json::from_str(&raw)
            .map_err(|e| SourceError::Invalid(format!("manifest CEDH illisible: {e}")))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| SourceError::Invalid(format!("manifest CEDH non sérialisable: {e}")))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn mark_year(&mut self, year: i32) {
        self.last_year_done = Some(year);
        self.fetched_at = Some(Self::now_iso_seconds());
    }
}

/// Chemin du manifeste CEDH sous `data_dir` (`<data_dir>/cedh/manifest.json`).
pub fn manifest_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join(CEDH_DIR).join("manifest.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn year_query_bounds_and_filters() {
        let q = year_query(2024);
        assert!(q.contains("languageisocode=\"FRE\" OR languageisocode=\"ENG\""));
        assert!(q.contains("NOT (doctype=PR OR doctype=HFCOMOLD OR doctype=HECOMOLD)"));
        assert!(q.contains("kpdate:[2024-01-01T00:00:00.0Z TO 2024-12-31T23:59:59.0Z]"));
    }

    #[test]
    fn base_url_trailing_slash_trimmed() {
        let c = CedhClient::with_base_url("https://x/");
        assert_eq!(c.base_url, "https://x");
    }

    #[test]
    fn parse_results_extracts_itemid_doctype_and_columns() {
        let resp = json!({
            "resultcount": 5000,
            "results": [
                { "columns": { "itemid": "001-250438", "doctype": "HFJUD", "appno": "1234/16", "languageisocode": "FRE" } },
                { "columns": { "itemid": "002-14552", "doctype": "CLINF" } },
            ]
        });
        let (count, rows) = parse_results(&resp);
        assert_eq!(count, 5000);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].itemid, "001-250438");
        assert_eq!(rows[0].doctype, "HFJUD");
        assert_eq!(rows[0].language, "FRE");
        assert_eq!(rows[0].columns["appno"].as_str(), Some("1234/16"));
        assert_eq!(rows[1].doctype, "CLINF");
        assert_eq!(rows[1].language, "");
    }

    #[test]
    fn parse_results_strips_volatile_rank() {
        let resp = json!({
            "resultcount": 1,
            "results": [
                { "columns": { "itemid": "001-1", "doctype": "HFJUD", "rank": 1.5 } },
            ]
        });
        let (_, rows) = parse_results(&resp);
        assert!(rows[0].columns.get("rank").is_none());
        assert_eq!(rows[0].columns["itemid"].as_str(), Some("001-1"));
    }

    #[test]
    fn parse_results_drops_rows_without_itemid() {
        let resp = json!({
            "resultcount": 1,
            "results": [
                { "columns": { "doctype": "HFJUD" } },
                { "columns": { "itemid": "", "doctype": "HFJUD" } },
                { "columns": { "itemid": "001-1", "doctype": "HFJUD" } },
            ]
        });
        let (_, rows) = parse_results(&resp);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].itemid, "001-1");
    }

    #[test]
    fn parse_results_empty_when_no_results_array() {
        let (count, rows) = parse_results(&json!({}));
        assert_eq!(count, 0);
        assert!(rows.is_empty());
    }

    #[test]
    fn manifest_roundtrip_and_mark_year() {
        let dir = tempfile::tempdir().unwrap();
        let path = manifest_path(dir.path());
        let mut m = CedhManifest::load(&path).unwrap();
        assert_eq!(m.last_year_done, None);
        m.mark_year(2024);
        m.save(&path).unwrap();
        let reloaded = CedhManifest::load(&path).unwrap();
        assert_eq!(reloaded.last_year_done, Some(2024));
        assert!(reloaded.fetched_at.is_some());
    }
}
