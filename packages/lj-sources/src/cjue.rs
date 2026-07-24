//! Downloader EUR-Lex (CJUE) — I/O réseau au bord lj-sources (ADR 0094).
//!
//! Deux endpoints publics sans auth :
//! - SPARQL `publications.europa.eu/webapi/rdf/sparql` — métadonnées CDM, borné
//!   par année (OFFSET cappé 10 000 → partition). On liste les *works*
//!   `JUDG`/`ORDER`/`OPIN_AG` du secteur 6 (`STRSTARTS(celex,"6")`), `DISTINCT`
//!   sur le CELEX (la liste brute est multipliée par `resource-type`).
//! - resource `publications.europa.eu/resource/celex/{CELEX}` en **cascade
//!   d'`Accept`** (`application/xhtml+xml` puis `text/html` ; mono-`Accept` perd
//!   des arrêts en 404 silencieux). Texte FR par `Accept-Language: fra`, strippé
//!   au bord par [`crate::html_strip::strip_html`].
//!
//! Parsing métier dans `lj-core`. Throttle de courtoisie (~0,3 s), watermark par
//! fond façon [`crate::dila::DilaManifest`].

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

/// Endpoint SPARQL EUR-Lex.
pub const SPARQL_URL: &str = "https://publications.europa.eu/webapi/rdf/sparql";
/// Base resource EUR-Lex (content-negotiation par CELEX).
pub const RESOURCE_BASE_URL: &str = "https://publications.europa.eu/resource/celex";
/// User-Agent de contact (endpoint public sans auth, courtoisie).
pub const USER_AGENT: &str = "librejustice-cjue/0.1 (+https://github.com/)";
/// Throttle de courtoisie entre fetchs.
pub const THROTTLE: Duration = Duration::from_millis(300);
/// Tentatives sur les 5xx/429 transitoires d'EUR-Lex (Cellar/SPARQL surchargé
/// renvoie épisodiquement 502/503/504) avant d'abandonner. Backoff géométrique
/// `THROTTLE << attempt`. Sans ça, un coup de chaleur Cellar dégrade en masse les
/// décisions au « repli minimum ecli+date » (prédicats CDM perdus).
const CJUE_RETRIES: u32 = 4;
/// Page SPARQL (sous le cap OFFSET 10 000 par année).
pub const PAGE_SIZE: usize = 1000;
/// Cascade d'`Accept` du resource endpoint (xhtml d'abord — l'arrêt n'est servi
/// qu'en xhtml dans certains cas, 404 silencieux en text/html).
pub const ACCEPT_CASCADE: [&str; 2] = ["application/xhtml+xml", "text/html"];

use crate::state_paths::CJUE_DIR;

/// Liste blanche des prédicats CDM riches retenus en `source_fields` (audit
/// `cjue.md` §Champs). [`map_predicates`] réduit les URI d'autorité à leur code
/// (`…/authority/<set>/<code>` → `<code>`), garde les littéraux verbatim et les
/// URI cellar (juge/AG/citation) brutes.
const RICH_PREDICATES: &[&str] = &[
    "resource_legal_type",
    "case-law_has_type_procedure_concept_type_procedure",
    "case-law_has_procjur",
    "resource_legal_is_about_subject-matter",
    "case-law_is-about_case-law-subject-matter",
    "case-law_is_about_concept_new_case-law",
    "case-law_delivered_by_judge",
    "case-law_delivered_by_advocate-general",
    "case-law_delivered_by_court-formation",
    "case-law_has_conclusions_opinion_advocate-general",
    "case-law_originates_in_country",
    "national_judgement",
    "case-law_national-judgement",
    "case-law_uses_procedure_language",
    "resource_legal_uses_originally_language",
    "work_cites_work",
    "case-law_interpretes_resource_legal",
    "case-law_published_in_erecueil",
    "case-law_article_journal_related",
];

/// Liste SPARQL des works CJUE (secteur 6) d'une année, dédupliqués par CELEX.
/// `OFFSET`/`LIMIT` partitionnent sous le cap 10 000. `?date` = date du document,
/// `?ecli` optionnel.
fn year_sparql(year: i32, offset: usize, limit: usize) -> String {
    format!(
        "PREFIX cdm: <http://publications.europa.eu/ontology/cdm#>\n\
SELECT DISTINCT ?celex ?date ?ecli WHERE {{\n\
  ?work cdm:work_has_resource-type ?type .\n\
  VALUES ?type {{ <http://publications.europa.eu/resource/authority/resource-type/JUDG> \
<http://publications.europa.eu/resource/authority/resource-type/ORDER> \
<http://publications.europa.eu/resource/authority/resource-type/OPIN_AG> }}\n\
  ?work cdm:resource_legal_id_celex ?celex .\n\
  FILTER(STRSTARTS(STR(?celex), \"6\"))\n\
  ?work cdm:work_date_document ?date .\n\
  FILTER(?date >= \"{year}-01-01\"^^<http://www.w3.org/2001/XMLSchema#date> \
&& ?date < \"{next}-01-01\"^^<http://www.w3.org/2001/XMLSchema#date>)\n\
  OPTIONAL {{ ?work cdm:case-law_ecli ?ecli . }}\n\
}}\nORDER BY ?celex\nOFFSET {offset}\nLIMIT {limit}",
        next = year + 1
    )
}

/// Tous les triplets `?p ?o` du work d'un CELEX donné — le dump CDM complet
/// (subject-matter, juge rapporteur / AG / formation, `work_cites_work`,
/// `interpretes_resource_legal`, procédure, pays de renvoi…). C'est la mine de
/// richesse (audit `cjue.md` §Champs) que la liste paginée ne fournit pas. Le
/// work est désigné par son `resource_legal_id_celex`.
fn work_describe_sparql(celex: &str) -> String {
    format!(
        "PREFIX cdm: <http://publications.europa.eu/ontology/cdm#>\n\
SELECT ?p ?o WHERE {{\n\
  ?work cdm:resource_legal_id_celex \"{celex}\"^^<http://www.w3.org/2001/XMLSchema#string> .\n\
  ?work ?p ?o .\n\
}}"
    )
}

/// Requête titre-FR + date d'un acte législatif par CELEX (ADR 0138). Le titre FR
/// officiel porte le numéro année/séquence en tête → ré-apparié par 3ter.
fn legislation_meta_sparql(celex: &str) -> String {
    format!(
        "PREFIX cdm: <http://publications.europa.eu/ontology/cdm#>\n\
SELECT ?title ?date WHERE {{\n\
  ?work cdm:resource_legal_id_celex \"{celex}\"^^<http://www.w3.org/2001/XMLSchema#string> .\n\
  OPTIONAL {{ ?work cdm:work_date_document ?date . }}\n\
  OPTIONAL {{\n\
    ?exp cdm:expression_belongs_to_work ?work .\n\
    ?exp cdm:expression_uses_language <http://publications.europa.eu/resource/authority/language/FRA> .\n\
    ?exp cdm:expression_title ?title .\n\
  }}\n\
}} LIMIT 1"
    )
}

/// Client EUR-Lex (reqwest async + TracingMiddleware, comme `judilibre.rs`).
pub struct CjueClient {
    sparql_url: String,
    resource_base: String,
    client: ClientWithMiddleware,
}

impl CjueClient {
    /// Client de production ([`SPARQL_URL`] + [`RESOURCE_BASE_URL`]).
    pub fn new() -> Self {
        Self::with_urls(SPARQL_URL, RESOURCE_BASE_URL)
    }

    /// Client sur des bases données (trailing `/` retiré).
    pub fn with_urls(sparql_url: impl Into<String>, resource_base: impl Into<String>) -> Self {
        let client = ClientBuilder::new(
            reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .connect_timeout(Duration::from_secs(15))
                // Timeout par requête BORNÉ : sans lui une connexion morte/half-open
                // sur le chemin hôte↔EUR-Lex laisse `send().await` attendre à l'infini
                // (process en sleep, chaîne cron `&&` gelée en silence — incident
                // 2026-06-17). Au-delà, la requête erreur et `get_with_retry` réessaie.
                .timeout(Duration::from_secs(120))
                // Pas de réutilisation de connexion : fetchs séquentiels (aucun gain
                // keepalive) et une connexion poolée réutilisée finit par se figer sur
                // ce chemin réseau — chaque tentative repart donc sur une connexion neuve.
                .pool_max_idle_per_host(0)
                .build()
                .expect("reqwest client build"),
        )
        .with(TracingMiddleware::default())
        .build();
        Self {
            sparql_url: sparql_url.into().trim_end_matches('/').to_string(),
            resource_base: resource_base.into().trim_end_matches('/').to_string(),
            client,
        }
    }

    /// GET avec retry borné sur les 5xx/429 transitoires (backoff `THROTTLE <<
    /// attempt`). Renvoie `(status, text)` du dernier essai ; ne lit le corps qu'au
    /// dernier essai (ou sur statut non transitoire). L'interprétation
    /// (200/404/406/erreur) reste à l'appelant — seuls les transitoires sont
    /// réessayés ici, pas masqués.
    async fn get_with_retry(
        &self,
        url: &str,
        params: &[(&str, String)],
        accept: &str,
        accept_language: Option<&str>,
        what: &str,
    ) -> Result<(u16, String)> {
        for attempt in 0..CJUE_RETRIES {
            if attempt > 0 {
                tokio::time::sleep(THROTTLE * (1 << attempt)).await;
            }
            let mut req = self.client.get(url).header("accept", accept).query(params);
            if let Some(lang) = accept_language {
                req = req.header("accept-language", lang);
            }
            let resp = match req.send().await {
                Ok(resp) => resp,
                // Erreur d'envoi (timeout, connexion coupée/morte) = transitoire :
                // on réessaie sur une connexion neuve tant qu'il reste des tentatives.
                Err(e) if attempt + 1 < CJUE_RETRIES => {
                    tracing::warn!(what, attempt, error = %e, "requête eur-lex échouée (envoi/timeout), retry");
                    continue;
                }
                Err(e) => return Err(SourceError::Invalid(format!("requête {what}: {e}"))),
            };
            let status = resp.status().as_u16();
            let transient = status == 429 || (500..600).contains(&status);
            if transient && attempt + 1 < CJUE_RETRIES {
                continue;
            }
            let text = resp.text().await?;
            return Ok((status, text));
        }
        unreachable!("get_with_retry: la dernière itération retourne toujours")
    }

    /// Une page SPARQL d'une année (réponse SPARQL JSON results).
    pub async fn sparql_page(&self, year: i32, offset: usize) -> Result<Value> {
        let query = year_sparql(year, offset, PAGE_SIZE);
        let params: [(&str, String); 2] = [
            ("query", query),
            ("format", "application/sparql-results+json".to_string()),
        ];
        let (status, text) = self
            .get_with_retry(
                &self.sparql_url,
                &params,
                "application/sparql-results+json",
                None,
                "sparql eur-lex",
            )
            .await?;
        if status != 200 {
            return Err(SourceError::Invalid(format!(
                "eur-lex sparql statut {status}: {}",
                text.chars().take(200).collect::<String>()
            )));
        }
        Ok(sonic_rs::from_str(&text)?)
    }

    /// Texte d'un CELEX, **FR-prioritaire avec repli EN** (ADR 0120, supersede le
    /// FR-only de 0094). Pour chaque langue (`fra` puis `eng`), cascade d'`Accept`
    /// (xhtml puis html). Renvoie `Ok(Some((texte, lang)))` dès qu'une rendition
    /// 200 est servie (`lang` = code obtenu, tracé sur la décision) ; `Ok(None)` si
    /// ni FR ni EN n'est disponible (404/406/3xx partout → SKIP, l'affaire n'est pas
    /// publiée dans une de nos deux langues). Une autre erreur HTTP remonte
    /// franchement (#12).
    pub async fn resource_text(&self, celex: &str) -> Result<Option<(String, &'static str)>> {
        let url = format!("{}/{celex}", self.resource_base);
        for lang in ["fra", "eng"] {
            let mut last_status = 0u16;
            for accept in ACCEPT_CASCADE {
                let (status, text) = self
                    .get_with_retry(
                        &url,
                        &[],
                        accept,
                        Some(lang),
                        &format!("eur-lex resource {url} ({lang})"),
                    )
                    .await?;
                if status == 200 {
                    return Ok(Some((strip_html(&text), lang)));
                }
                last_status = status;
                // 404/406 = pas de rendition dans cette langue pour cet `Accept`.
                // 3xx (notamment 300 Multiple Choices) = EUR-Lex ne sert pas une
                // représentation unique pour ce CELEX : indisponibilité de contenu,
                // pas une erreur de notre côté. Dans les deux cas on poursuit (cascade
                // d'`Accept` puis langue suivante) — un CELEX problématique ne doit
                // pas aborter un bootstrap de 60 ans. Tout autre statut (4xx
                // auth/bad-request, etc.) reste une erreur franche (#12).
                let skippable = status == 404 || status == 406 || (300..400).contains(&status);
                if !skippable {
                    return Err(SourceError::Invalid(format!(
                        "eur-lex resource statut {status} {celex} (accept={accept}, lang={lang})"
                    )));
                }
            }
            if last_status != 404 && last_status != 406 {
                tracing::warn!(
                    celex,
                    lang,
                    status = last_status,
                    "eur-lex resource sans rendition unique (3xx/non-404)"
                );
            }
        }
        // Ni FR ni EN disponible (404/406/3xx partout) : SKIP.
        Ok(None)
    }

    /// Prédicats CDM riches d'un work par CELEX (SPARQL `?p ?o`), mappés en
    /// `source_fields` lisibles via [`map_predicates`]. C'est la mine de richesse
    /// de l'audit (subject-matter, juge/AG/formation, `work_cites_work`,
    /// `interpretes_resource_legal`, procédure, pays de renvoi…). Une réponse
    /// non-200 remonte franchement (#12) — l'appelant logue et retombe sur le
    /// minimum `{ecli, date}` plutôt que de masquer l'échec.
    pub async fn fetch_work_predicates(&self, celex: &str) -> Result<Value> {
        let query = work_describe_sparql(celex);
        let params: [(&str, String); 2] = [
            ("query", query),
            ("format", "application/sparql-results+json".to_string()),
        ];
        let (status, text) = self
            .get_with_retry(
                &self.sparql_url,
                &params,
                "application/sparql-results+json",
                None,
                &format!("sparql prédicats {celex}"),
            )
            .await?;
        if status != 200 {
            return Err(SourceError::Invalid(format!(
                "eur-lex sparql prédicats {celex} statut {status}: {}",
                text.chars().take(200).collect::<String>()
            )));
        }
        let raw: Value = sonic_rs::from_str(&text)?;
        Ok(map_predicates(&raw))
    }

    /// Métadonnées FR d'un acte législatif UE par CELEX (ADR 0138) : titre officiel
    /// français + date du document. `Ok(None)` si le CELEX n'existe pas ou n'a pas de
    /// titre FR (→ SKIP à l'ingestion). Le titre porte le numéro année/séquence en tête
    /// (« Règlement (UE) 2016/679 … », « Sixième directive 77/388/CEE … ») — la forme
    /// exacte que la passe de résolution 3ter ré-apparie contre `legal_text.title`.
    pub async fn legislation_meta_fr(
        &self,
        celex: &str,
    ) -> Result<Option<(String, Option<String>)>> {
        let query = legislation_meta_sparql(celex);
        let params: [(&str, String); 2] = [
            ("query", query),
            ("format", "application/sparql-results+json".to_string()),
        ];
        let (status, text) = self
            .get_with_retry(
                &self.sparql_url,
                &params,
                "application/sparql-results+json",
                None,
                &format!("sparql législation {celex}"),
            )
            .await?;
        if status != 200 {
            return Err(SourceError::Invalid(format!(
                "eur-lex sparql législation {celex} statut {status}: {}",
                text.chars().take(200).collect::<String>()
            )));
        }
        let raw: Value = sonic_rs::from_str(&text)?;
        let Some(b) = raw["results"]["bindings"]
            .as_array()
            .and_then(|a| a.first())
        else {
            return Ok(None);
        };
        let Some(title) = b["title"]["value"].as_str().filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        let date = b["date"]["value"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        Ok(Some((title.to_string(), date)))
    }
}

impl Default for CjueClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Une ligne SPARQL : CELEX (PK) + date + ECLI optionnel (jamais dérivé du
/// CELEX, décision #3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CjueResult {
    pub celex: String,
    pub date: Option<String>,
    pub ecli: Option<String>,
}

/// Parse une réponse SPARQL JSON results en lignes dédupliquées par CELEX (la
/// liste brute multiplie un work par `resource-type` — décision #4 / audit).
/// Préserve l'ordre d'apparition.
pub fn parse_sparql(resp: &Value) -> Vec<CjueResult> {
    let mut out = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let Some(bindings) = resp["results"]["bindings"].as_array() else {
        return out;
    };
    for b in bindings {
        let Some(celex) = b["celex"]["value"].as_str() else {
            continue;
        };
        if celex.is_empty() || seen.iter().any(|c| c == celex) {
            continue;
        }
        seen.push(celex.to_string());
        out.push(CjueResult {
            celex: celex.to_string(),
            date: b["date"]["value"].as_str().map(str::to_string),
            ecli: b["ecli"]["value"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        });
    }
    out
}

/// Nom local d'un prédicat CDM (`…/cdm#case-law_ecli` → `case-law_ecli`,
/// `…22-rdf-syntax-ns#type` → `type`). Queue après `#`, repli sur le dernier
/// segment de chemin.
fn predicate_local_name(uri: &str) -> &str {
    if let Some((_, tail)) = uri.rsplit_once('#') {
        return tail;
    }
    uri.rsplit('/').next().unwrap_or(uri)
}

/// Code d'autorité d'une URI EUR-Lex : queue d'un `…/authority/<set>/<code>`
/// (`…/subject-matter/IMMI` → `IMMI`, `…/formjug/CHAMB_GD_C` → `CHAMB_GD_C`,
/// `…/fd_100/PREJ` → `PREJ`). Les URI cellar (juge, AG, citation) n'ont pas de
/// code lisible : on garde l'URI brute (résolution nom/CELEX hors-ligne
/// impossible — décision audit : URI brute si non résolvable).
fn authority_code_or_uri(uri: &str) -> String {
    if let Some(rest) = uri.split("/authority/").nth(1) {
        // `<set>/<code>` → `<code>` (dernier segment).
        if let Some(code) = rest.rsplit('/').next() {
            return code.to_string();
        }
    }
    uri.to_string()
}

/// Mappe le dump SPARQL `?p ?o` d'un work en `source_fields` lisibles (audit
/// `cjue.md` §Champs). Ne retient que les prédicats riches de [`RICH_PREDICATES`],
/// la clé est le nom local du prédicat. Les valeurs URI d'autorité sont réduites
/// à leur code (`IMMI`, `CHAMB_GD_C`…), les littéraux gardés verbatim, les URI
/// cellar (juge/AG/citation) gardées brutes (non résolvables hors-ligne). Une clé
/// à valeur unique sort scalaire, multi-valuée sort en tableau JSON (ordre
/// d'apparition, doublons retirés).
pub fn map_predicates(resp: &Value) -> Value {
    use serde_json::{Map, Value as J};
    let mut collected: Vec<(String, Vec<String>)> = Vec::new();
    let Some(bindings) = resp["results"]["bindings"].as_array() else {
        return J::Object(Map::new());
    };
    for b in bindings {
        let Some(pred_uri) = b["p"]["value"].as_str() else {
            continue;
        };
        let local = predicate_local_name(pred_uri);
        if !RICH_PREDICATES.contains(&local) {
            continue;
        }
        let Some(o) = b.get("o") else { continue };
        let value = match o["type"].as_str() {
            Some("uri") => authority_code_or_uri(o["value"].as_str().unwrap_or("")),
            _ => o["value"].as_str().unwrap_or("").trim().to_string(),
        };
        if value.is_empty() {
            continue;
        }
        match collected.iter_mut().find(|(k, _)| k == local) {
            Some((_, vals)) => {
                if !vals.contains(&value) {
                    vals.push(value);
                }
            }
            None => collected.push((local.to_string(), vec![value])),
        }
    }
    let mut out = Map::new();
    for (key, mut vals) in collected {
        let v = if vals.len() == 1 {
            J::String(vals.pop().expect("len==1"))
        } else {
            J::Array(vals.into_iter().map(J::String).collect())
        };
        out.insert(key, v);
    }
    J::Object(out)
}

/// Extrait la bannière « objet » doctrinale d'un arrêt/ordonnance CJUE : le bloc
/// de mots-clés entre guillemets français (`« Renvoi préjudiciel – … »`) qui
/// suit l'en-tête (`ARRÊT DE LA COUR (…) <date>`). C'est l'équivalent d'un
/// abstract officiel FR (audit `cjue.md` §CHAMPS NON RENDUS) — **pas** `<title>`
/// (vide ou égal au CELEX). Opère sur le texte déjà strippé. `None` si aucun bloc
/// `«…»` n'est présent.
pub fn extract_objet(body_text: &str) -> Option<String> {
    let start = body_text.find('«')?;
    let after = &body_text[start + '«'.len_utf8()..];
    let end = after.find('»')?;
    let objet = after[..end].trim();
    if objet.is_empty() {
        None
    } else {
        Some(objet.to_string())
    }
}

/// Watermark CJUE par fond : dernière année dont la liste a été drainée.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CjueManifest {
    #[serde(default)]
    pub last_year_done: Option<i32>,
    #[serde(default)]
    pub fetched_at: Option<String>,
}

impl CjueManifest {
    fn now_iso_seconds() -> String {
        Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)?;
        serde_json::from_str(&raw)
            .map_err(|e| SourceError::Invalid(format!("manifest CJUE illisible: {e}")))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| SourceError::Invalid(format!("manifest CJUE non sérialisable: {e}")))?;
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

/// Chemin du manifeste CJUE sous `data_dir` (`<data_dir>/cjue/manifest.json`).
pub fn manifest_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join(CJUE_DIR).join("manifest.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn year_sparql_bounds_and_sector_filter() {
        let q = year_sparql(2024, 0, 1000);
        assert!(q.contains("STRSTARTS(STR(?celex), \"6\")"));
        assert!(q.contains("\"2024-01-01\"^^"));
        assert!(q.contains("\"2025-01-01\"^^"));
        assert!(q.contains("OFFSET 0"));
        assert!(q.contains("LIMIT 1000"));
        assert!(q.contains("DISTINCT"));
        assert!(q.contains("/JUDG>"));
        assert!(q.contains("/ORDER>"));
        assert!(q.contains("/OPIN_AG>"));
    }

    #[test]
    fn urls_trailing_slash_trimmed() {
        let c = CjueClient::with_urls("https://s/sparql/", "https://r/celex/");
        assert_eq!(c.sparql_url, "https://s/sparql");
        assert_eq!(c.resource_base, "https://r/celex");
    }

    #[test]
    fn parse_sparql_dedups_by_celex_preserving_order() {
        // Un work ressort N fois (une par resource-type) — décision #4.
        let resp = json!({
            "results": { "bindings": [
                { "celex": { "value": "62020CJ0560" },
                  "date": { "value": "2024-01-30" },
                  "ecli": { "value": "ECLI:EU:C:2024:96" } },
                { "celex": { "value": "62020CJ0560" },
                  "date": { "value": "2024-01-30" } },
                { "celex": { "value": "62023CO0614" },
                  "date": { "value": "2024-01-15" } },
            ]}
        });
        let rows = parse_sparql(&resp);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].celex, "62020CJ0560");
        // ECLI pris verbatim, jamais dérivé du CELEX (décision #3).
        assert_eq!(rows[0].ecli.as_deref(), Some("ECLI:EU:C:2024:96"));
        assert_eq!(rows[0].date.as_deref(), Some("2024-01-30"));
        assert_eq!(rows[1].celex, "62023CO0614");
        assert_eq!(rows[1].ecli, None);
    }

    #[test]
    fn parse_sparql_skips_empty_celex_and_empty_ecli() {
        let resp = json!({
            "results": { "bindings": [
                { "celex": { "value": "" } },
                { "celex": { "value": "62020CJ0001" }, "ecli": { "value": "" } },
            ]}
        });
        let rows = parse_sparql(&resp);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].celex, "62020CJ0001");
        assert_eq!(rows[0].ecli, None);
    }

    #[test]
    fn parse_sparql_empty_without_bindings() {
        assert!(parse_sparql(&json!({})).is_empty());
    }

    #[test]
    fn work_describe_sparql_binds_celex_and_selects_po() {
        let q = work_describe_sparql("62020CJ0560");
        assert!(q.contains("resource_legal_id_celex \"62020CJ0560\""));
        assert!(q.contains("?work ?p ?o ."));
        assert!(q.contains("SELECT ?p ?o"));
    }

    fn pred(p: &str, ty: &str, o: &str) -> Value {
        json!({ "p": { "value": p }, "o": { "type": ty, "value": o } })
    }

    #[test]
    fn map_predicates_fixture_readable_keys_codes_and_multivalue() {
        // Fixture style sparql_predicates.json : dump `?p ?o` du work
        // 62020CJ0560 (extrait des prédicats riches réels, audit cjue.md §Champs).
        let cdm = "http://publications.europa.eu/ontology/cdm#";
        let auth = "http://publications.europa.eu/resource/authority";
        let cellar = "http://publications.europa.eu/resource/cellar";
        let resp = json!({
            "results": { "bindings": [
                // Bruit hors RICH_PREDICATES : ignoré.
                pred(&format!("{cdm}work_version"), "literal", "definitive"),
                pred(
                    "http://www.w3.org/1999/02/22-rdf-syntax-ns#type",
                    "uri",
                    &format!("{cdm}judgement"),
                ),
                // Type d'acte (littéral verbatim).
                pred(&format!("{cdm}resource_legal_type"), "literal", "CJ"),
                // Procédure (URI autorité → code).
                pred(
                    &format!("{cdm}case-law_has_type_procedure_concept_type_procedure"),
                    "uri",
                    &format!("{auth}/fd_100/PREJ"),
                ),
                pred(
                    &format!("{cdm}case-law_has_procjur"),
                    "uri",
                    &format!("{auth}/procjur/REFER_PREL"),
                ),
                // Matière multi-valuée (URI autorité → codes, tableau).
                pred(
                    &format!("{cdm}resource_legal_is_about_subject-matter"),
                    "uri",
                    &format!("{auth}/subject-matter/IMMI"),
                ),
                pred(
                    &format!("{cdm}resource_legal_is_about_subject-matter"),
                    "uri",
                    &format!("{auth}/subject-matter/ELSJ"),
                ),
                // Formation, juge rapporteur (cellar brute), AG, conclusions.
                pred(
                    &format!("{cdm}case-law_delivered_by_court-formation"),
                    "uri",
                    &format!("{auth}/formjug/CHAMB_GD_C"),
                ),
                pred(
                    &format!("{cdm}case-law_delivered_by_judge"),
                    "uri",
                    &format!("{cellar}/6bc92600-f3ca-4c3d-b588-d733c0128593"),
                ),
                // Pays de renvoi.
                pred(
                    &format!("{cdm}case-law_originates_in_country"),
                    "uri",
                    &format!("{auth}/country/AUT"),
                ),
                // Langue de procédure.
                pred(
                    &format!("{cdm}case-law_uses_procedure_language"),
                    "uri",
                    &format!("{auth}/language/DEU"),
                ),
                // Graphe de citations (cellar brute, dédup d'un doublon).
                pred(
                    &format!("{cdm}work_cites_work"),
                    "uri",
                    &format!("{cellar}/d00948e0-b542-40be-beed-422a7fc3548e"),
                ),
                pred(
                    &format!("{cdm}work_cites_work"),
                    "uri",
                    &format!("{cellar}/b81af559-84bd-45b3-9d0e-c3f70a22516e"),
                ),
                pred(
                    &format!("{cdm}work_cites_work"),
                    "uri",
                    &format!("{cellar}/d00948e0-b542-40be-beed-422a7fc3548e"),
                ),
                // Texte UE interprété (cellar brute).
                pred(
                    &format!("{cdm}case-law_interpretes_resource_legal"),
                    "uri",
                    &format!("{cellar}/b81af559-84bd-45b3-9d0e-c3f70a22516e"),
                ),
                // Flag publication Recueil + doctrine (littéraux).
                pred(&format!("{cdm}case-law_published_in_erecueil"), "literal", "1"),
                pred(
                    &format!("{cdm}case-law_article_journal_related"),
                    "literal",
                    "1. Gazin, Fabienne: Immigration …, Europe 2024, nº 3",
                ),
                // Décision nationale d'origine (littéral, espaces collapsés).
                pred(
                    &format!("{cdm}case-law_national-judgement"),
                    "literal",
                    "  Verwaltungsgericht Wien, Beschluss vom 25/09/2020  ",
                ),
            ]}
        });

        let m = map_predicates(&resp);
        let obj = m.as_object().expect("objet");

        // Bruit hors liste blanche absent.
        assert!(!obj.contains_key("work_version"));
        assert!(!obj.contains_key("type"));

        // Littéral verbatim.
        assert_eq!(m["resource_legal_type"], json!("CJ"));
        // URI autorité → code.
        assert_eq!(
            m["case-law_has_type_procedure_concept_type_procedure"],
            json!("PREJ")
        );
        assert_eq!(m["case-law_has_procjur"], json!("REFER_PREL"));
        assert_eq!(
            m["case-law_delivered_by_court-formation"],
            json!("CHAMB_GD_C")
        );
        assert_eq!(m["case-law_originates_in_country"], json!("AUT"));
        assert_eq!(m["case-law_uses_procedure_language"], json!("DEU"));
        // Multi-valué → tableau, ordre préservé.
        assert_eq!(
            m["resource_legal_is_about_subject-matter"],
            json!(["IMMI", "ELSJ"])
        );
        // Cellar non résolvable → URI brute conservée.
        assert_eq!(
            m["case-law_delivered_by_judge"],
            json!("http://publications.europa.eu/resource/cellar/6bc92600-f3ca-4c3d-b588-d733c0128593")
        );
        // work_cites_work : doublon retiré → 2 URI brutes.
        assert_eq!(
            m["work_cites_work"],
            json!([
                "http://publications.europa.eu/resource/cellar/d00948e0-b542-40be-beed-422a7fc3548e",
                "http://publications.europa.eu/resource/cellar/b81af559-84bd-45b3-9d0e-c3f70a22516e"
            ])
        );
        assert_eq!(
            m["case-law_interpretes_resource_legal"],
            json!("http://publications.europa.eu/resource/cellar/b81af559-84bd-45b3-9d0e-c3f70a22516e")
        );
        assert_eq!(m["case-law_published_in_erecueil"], json!("1"));
        // Littéral trimé (espaces de bord).
        assert_eq!(
            m["case-law_national-judgement"],
            json!("Verwaltungsgericht Wien, Beschluss vom 25/09/2020")
        );
    }

    #[test]
    fn map_predicates_empty_without_bindings() {
        assert_eq!(map_predicates(&json!({})), json!({}));
    }

    #[test]
    fn extract_objet_from_real_header() {
        // En-tête réel strippé de 62020CJ0560 (audit : `<title>` = CELEX, inutile ;
        // l'objet est la bannière `«…»`).
        let body = "62020CJ0560 ARRÊT DE LA COUR (grande chambre) 30 janvier 2024 ( *1 ) \
« Renvoi préjudiciel – Politique relative à l'immigration – Directive 2003/86/CE – \
Notion de \u{201c}mineur non accompagné\u{201d} » Dans l'affaire C-560/20, ayant pour objet …";
        let objet = extract_objet(body).expect("objet présent");
        assert!(objet.starts_with("Renvoi préjudiciel – Politique"));
        assert!(objet.contains("Directive 2003/86/CE"));
        // Le bloc s'arrête au guillemet fermant : pas de « Dans l'affaire ».
        assert!(!objet.contains("Dans l'affaire"));
    }

    #[test]
    fn extract_objet_none_when_no_banner() {
        assert_eq!(extract_objet("ORDONNANCE sans guillemets"), None);
    }

    #[test]
    fn manifest_roundtrip_and_mark_year() {
        let dir = tempfile::tempdir().unwrap();
        let path = manifest_path(dir.path());
        let mut m = CjueManifest::load(&path).unwrap();
        assert_eq!(m.last_year_done, None);
        m.mark_year(2024);
        m.save(&path).unwrap();
        let reloaded = CjueManifest::load(&path).unwrap();
        assert_eq!(reloaded.last_year_done, Some(2024));
        assert!(reloaded.fetched_at.is_some());
    }
}
