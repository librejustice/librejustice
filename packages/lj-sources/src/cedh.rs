//! Downloader HUDOC (CEDH) — I/O réseau au bord lj-sources (ADR 0094).
//!
//! API publique sans auth sur `hudoc.echr.coe.int`, deux endpoints :
//! - liste `GET /app/query/results` — `sort` **obligatoire** (sinon 404), `length`
//!   cappé <500, fenêtre globale 10 000 → partition par année
//!   (`kpdate:[YYYY-01-01T00:00:00.0Z TO …]`). Filtre bilingue FR-prioritaire
//!   (ADR 0120, supersede le FR-only de 0094) : `contentsitename=ECHR AND NOT
//!   (doctype=PR OR HFCOMOLD OR HECOMOLD) AND (languageisocode="FRE" OR
//!   languageisocode="ENG")`. HUDOC publie un document par version linguistique :
//!   l'orchestrateur regroupe par affaire et retient la version FR si elle existe,
//!   sinon EN (les affaires EN-only — beaucoup d'irrecevabilités/comité — entrent
//!   ainsi dans le corpus). `select` = CSV des colonnes voulues (projection
//!   serveur, sérialisation creuse : seules les clés non vides reviennent).
//! - corps `GET /app/conversion/docx/html/body?...&id={itemid}` → HTML (DOCX
//!   converti), strippé en texte au bord par [`crate::html_strip::strip_html`].
//!
//! Le parsing métier vit dans `lj-core` (reçoit colonnes désérialisées + texte
//! strippé). Throttle de courtoisie (~0,25 s), User-Agent de contact. Le
//! watermark (dernière année traitée) est persisté par fond façon
//! [`crate::dila::DilaManifest`].

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
/// User-Agent de contact (endpoint public sans auth, courtoisie).
pub const USER_AGENT: &str = "librejustice-cedh/0.1 (+https://github.com/)";
/// Throttle de courtoisie entre fetchs.
pub const THROTTLE: Duration = Duration::from_millis(250);
/// Nombre de tentatives sur le corps (body) face aux 5xx/429 transitoires de
/// HUDOC avant de différer (skip). Backoff = `THROTTLE << attempt`.
const CEDH_BODY_RETRIES: u32 = 4;
/// `length` par page (cappé <500 côté serveur).
pub const PAGE_SIZE: usize = 200;
/// Colonnes projetées dans `select` — séparées par **virgule** (CSV). HUDOC a
/// cessé d'honorer le séparateur `;` : avec `;`, le `select` est ignoré et chaque
/// résultat ne renvoie que `{rank}` (aucun `itemid` → `parse_results` jette tout →
/// 0 décision ingérée). Avec `,`, l'API renvoie le jeu de colonnes complet. Les
/// valeurs internes multi-items (`appno`, `documentcollectionid2`…) gardent, elles,
/// leur `;` propre dans la réponse — c'est uniquement le séparateur de `select` qui
/// est passé en virgule.
pub const SELECT_COLUMNS: &str = "itemid,docname,appno,extractedappno,ecli,kpdate,judgementdate,\
decisiondate,introductiondate,doctype,typedescription,documentcollectionid2,article,violation,\
nonviolation,conclusion,importance,respondent,originatingbody_name,separateopinion,kpthesaurus,\
scl,sclappnos,externalsources,languageisocode";

const CEDH_SOURCE_DIR: &str = "cedh";

/// Filtre HUDOC bilingue FR-prioritaire (ADR 0120, supersede le FR-only de 0094),
/// borné à une année par `kpdate:[…]` (grounding #4 : `NOT (doctype=PR OR HFCOMOLD
/// OR HECOMOLD)`). On ramène FR **et** EN ; l'orchestrateur regroupe par affaire et
/// préfère la version FR (cf. [`super`]).
fn year_query(year: i32) -> String {
    format!(
        "contentsitename=ECHR AND (NOT (doctype=PR OR doctype=HFCOMOLD OR doctype=HECOMOLD)) \
AND (languageisocode=\"FRE\" OR languageisocode=\"ENG\") \
AND (kpdate:[{year}-01-01T00:00:00.0Z TO {year}-12-31T23:59:59.0Z])"
    )
}

/// Client HUDOC (reqwest async + TracingMiddleware, comme `judilibre.rs`).
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
        let client = ClientBuilder::new(
            reqwest::Client::builder()
                .user_agent(USER_AGENT)
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
            d.join(CEDH_SOURCE_DIR)
                .join("bodies")
                .join(format!("{itemid}.html"))
        })
    }

    /// Une page de la liste FR d'une année. `sort` posé (obligatoire), `start`
    /// décalé de `PAGE_SIZE`. Réponse brute `{resultcount, results:[{columns}]}`.
    pub async fn results_page(&self, year: i32, start: usize) -> Result<Value> {
        let url = format!("{}/app/query/results", self.base_url);
        let params: [(&str, String); 5] = [
            ("query", year_query(year)),
            ("select", SELECT_COLUMNS.to_string()),
            ("sort", "kpdate Descending".to_string()),
            ("start", start.to_string()),
            ("length", PAGE_SIZE.to_string()),
        ];
        let resp = self
            .client
            .get(&url)
            .header("accept", "application/json")
            .query(&params)
            .send()
            .await
            .map_err(|e| SourceError::Invalid(format!("requête hudoc results {url}: {e}")))?;
        let status = resp.status().as_u16();
        let text = resp.text().await?;
        if status != 200 {
            return Err(SourceError::Invalid(format!(
                "hudoc results statut {status} {url}: {}",
                text.chars().take(200).collect::<String>()
            )));
        }
        Ok(sonic_rs::from_str(&text)?)
    }

    /// Corps texte d'un document (`itemid`) : DOCX converti en HTML, strippé en
    /// texte. **Corps 0 octet ≠ texte vide** (décision #5 : lag de conversion) —
    /// le strip d'un corps vide rend une chaîne vide, l'ingest diffère alors la
    /// décision (re-fetch au prochain run).
    pub async fn body_text(&self, itemid: &str) -> Result<String> {
        // Cache disque : un corps déjà tiré est relu sans réseau (ni throttle).
        // On ne cache QUE les 200 (corps réel) : les 204/vides ne sont pas mis en
        // cache pour respecter le lag de conversion (un arrêt récent 204 peut gagner
        // son corps plus tard), donc leur absence de fichier = re-fetch au run suivant.
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
        // 429 sous charge : transitoires, PAS structurels. Un seul tuait tout le
        // bootstrap (cas réel : body 001-63180 → 502 → crash à l'année 1998). On
        // retente avec backoff ; si le transitoire persiste, on diffère (corps vide
        // → skip → re-fetch au prochain sync), même philosophie que le 204.
        let mut last_status = 0u16;
        for attempt in 0..CEDH_BODY_RETRIES {
            if attempt > 0 {
                tokio::time::sleep(THROTTLE * (1 << attempt)).await;
            }
            let resp = self
                .client
                .get(&url)
                .header("accept", "text/html")
                .query(&params)
                .send()
                .await
                .map_err(|e| SourceError::Invalid(format!("requête hudoc body {url}: {e}")))?;
            let status = resp.status().as_u16();
            last_status = status;
            // 204 No Content = corps DOCX pas encore converti (lag, surtout arrêts de
            // comité récents) : ce n'est PAS une erreur fatale mais le cas « corps vide
            // → re-fetch différé » de la décision #5. On rend une chaîne vide et
            // l'ingest diffère la décision (skip), comme pour un corps 0 octet.
            if status == 204 {
                return Ok(String::new());
            }
            if status == 200 {
                let raw = resp.text().await?;
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
            // 429 / 5xx = transitoire → on retente. Tout autre code (4xx hors 429)
            // est structurel → erreur franche immédiate.
            if status != 429 && !(500..600).contains(&status) {
                return Err(SourceError::Invalid(format!(
                    "hudoc body statut {status} {itemid}"
                )));
            }
        }
        // Transitoire persistant après retries : on diffère (skip), comme un 204.
        tracing::warn!(
            itemid,
            status = last_status,
            "hudoc body transitoire persistant après retries — différé (skip)"
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
/// du choix FR-prioritaire au regroupement par affaire (ADR 0120).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CedhResult {
    pub itemid: String,
    pub doctype: String,
    pub language: String,
    pub columns: Value,
}

/// Parse une réponse `results` HUDOC en lignes exploitables (drop des lignes
/// sans `itemid` — PK obligatoire, décision #2). `resultcount` rendu à part pour
/// que l'orchestrateur sache quand il a tout vu (fenêtre 10 000 par année).
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
            out.push(CedhResult {
                itemid: itemid.to_string(),
                doctype: columns["doctype"].as_str().unwrap_or("").to_string(),
                language: columns["languageisocode"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
                columns: columns.clone(),
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
    data_dir.join(CEDH_SOURCE_DIR).join("manifest.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn year_query_bounds_and_filters() {
        let q = year_query(2024);
        // Bilingue FR-prioritaire (ADR 0120) : FR **et** EN ramenés, regroupés en aval.
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
            "resultcount": 76307,
            "results": [
                { "columns": { "itemid": "001-250438", "doctype": "HFJUD", "appno": "1234/16", "languageisocode": "FRE" } },
                { "columns": { "itemid": "002-14552", "doctype": "CLINF" } },
            ]
        });
        let (count, rows) = parse_results(&resp);
        assert_eq!(count, 76307);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].itemid, "001-250438");
        assert_eq!(rows[0].doctype, "HFJUD");
        assert_eq!(rows[0].language, "FRE");
        assert_eq!(rows[0].columns["appno"].as_str(), Some("1234/16"));
        assert_eq!(rows[1].doctype, "CLINF");
        // `languageisocode` absent → chaîne vide (sérialisation creuse HUDOC).
        assert_eq!(rows[1].language, "");
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
