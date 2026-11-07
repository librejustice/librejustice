//! Downloader EUR-Lex (CJUE) — I/O réseau au bord lj-sources.
//!
//! Deux endpoints publics sans auth :
//! - SPARQL `publications.europa.eu/webapi/rdf/sparql`: CDM metadata, partitioned
//!   by year under the OFFSET cap. Lists the `JUDG`/`ORDER`/`OPIN_AG` *works* of
//!   sector 6 (`STRSTARTS(celex,"6")`), `DISTINCT` on the CELEX (the raw list is
//!   multiplied by `resource-type`).
//! - resource `publications.europa.eu/resource/celex/{CELEX}` en **cascade
//!   d'`Accept`** (`application/xhtml+xml` puis `text/html`). Texte FR par
//!   `Accept-Language: fra`, strippé au bord par [`crate::html_strip::strip_html`].
//!
//! Parsing métier dans `lj-core`. Throttle de courtoisie (~0,3 s), watermark par
//! fond façon [`crate::dila::DilaManifest`].

use crate::error::{Result, SourceError};
use crate::html_strip::strip_html;
use chrono::{NaiveDate, Utc};
use regex::Regex;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_tracing::TracingMiddleware;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
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
/// Tentatives sur les 5xx/429 transitoires d'EUR-Lex avant d'abandonner. Backoff
/// géométrique `THROTTLE << attempt`.
const CJUE_RETRIES: u32 = 4;
/// SPARQL page, under the OFFSET cap of a year.
pub const PAGE_SIZE: usize = 1000;
/// CELEX per validity-bound query, bounded by the URL length EUR-Lex accepts.
const VALIDITY_CHUNK: usize = 150;
/// Borne EUR-Lex d'un acte sans terme.
const NO_END_OF_VALIDITY: &str = "9999-12-31";
/// Cascade d'`Accept` du resource endpoint : certains arrêts ne sont servis qu'en
/// xhtml, `text/html` répondant alors 404.
pub const ACCEPT_CASCADE: [&str; 2] = ["application/xhtml+xml", "text/html"];
/// CELLAR answers 404 to a bare `application/pdf` for an act it holds only as PDF/A.
const PDF_ACCEPT: &str = "application/pdf;type=pdfa1b, application/pdf;type=pdfa1a, application/pdf;type=pdfa2a, application/pdf";

use crate::state_paths::CJUE_DIR;

/// Liste blanche des prédicats CDM riches retenus en `source_fields`.
/// [`map_predicates`] réduit les URI d'autorité à leur code
/// (`…/authority/<set>/<code>` → `<code>`), garde les littéraux verbatim et les URI
/// cellar (juge/AG/citation) brutes.
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
    "case-law_joins_case_court",
    "work_cites_work",
    "case-law_interpretes_resource_legal",
    "case-law_published_in_erecueil",
    "case-law_article_journal_related",
];

/// SPARQL list of the CJUE works (sector 6) of a year, deduplicated by CELEX.
/// `OFFSET`/`LIMIT` partition under the cap. `?date` is the document date,
/// `?ecli` and the French title optional.
fn year_sparql(year: i32, offset: usize, limit: usize) -> String {
    format!(
        "PREFIX cdm: <http://publications.europa.eu/ontology/cdm#>\n\
SELECT DISTINCT ?celex ?date ?ecli ?title WHERE {{\n\
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
  OPTIONAL {{ ?exp cdm:expression_belongs_to_work ?work .\n\
     ?exp cdm:expression_uses_language <http://publications.europa.eu/resource/authority/language/FRA> .\n\
     ?exp cdm:expression_title ?title . }}\n\
}}\nORDER BY ?celex\nOFFSET {offset}\nLIMIT {limit}",
        next = year + 1
    )
}

/// Every CJUE work of `year` with the languages it has a manifestation in, `FRA DEU…`.
fn languages_sparql(year: i32, offset: usize, limit: usize) -> String {
    format!(
        "PREFIX cdm: <http://publications.europa.eu/ontology/cdm#>\n\
SELECT ?celex (GROUP_CONCAT(DISTINCT STR(?lang); separator=\" \") AS ?langs) WHERE {{\n\
  ?work cdm:work_has_resource-type ?type .\n\
  VALUES ?type {{ <http://publications.europa.eu/resource/authority/resource-type/JUDG> \
<http://publications.europa.eu/resource/authority/resource-type/ORDER> \
<http://publications.europa.eu/resource/authority/resource-type/OPIN_AG> }}\n\
  ?work cdm:resource_legal_id_celex ?celex .\n\
  FILTER(STRSTARTS(STR(?celex), \"6\"))\n\
  ?work cdm:work_date_document ?date .\n\
  FILTER(?date >= \"{year}-01-01\"^^<http://www.w3.org/2001/XMLSchema#date> \
&& ?date < \"{next}-01-01\"^^<http://www.w3.org/2001/XMLSchema#date>)\n\
  ?exp cdm:expression_belongs_to_work ?work ;\n\
       cdm:expression_uses_language ?lang .\n\
  ?man cdm:manifestation_manifests_expression ?exp .\n\
}}\nGROUP BY ?celex\nORDER BY ?celex\nOFFSET {offset}\nLIMIT {limit}",
        next = year + 1
    )
}

/// The court's summaries dated in `year`, each with the CELEX of the decision it sums
/// up: `_SUM` for a judgment of the Recueil, `_INF` for a decision left out of it.
fn summaries_sparql(year: i32, offset: usize, limit: usize) -> String {
    format!(
        "PREFIX cdm: <http://publications.europa.eu/ontology/cdm#>\n\
SELECT DISTINCT ?summary ?celex WHERE {{\n\
  ?s a cdm:summary_case-law ;\n\
     cdm:summary_case-law_id_celex ?summary ;\n\
     cdm:summary_summarizes_work ?work ;\n\
     cdm:work_date_document ?date .\n\
  ?work cdm:resource_legal_id_celex ?celex .\n\
  FILTER(?date >= \"{year}-01-01\"^^<http://www.w3.org/2001/XMLSchema#date> \
&& ?date < \"{next}-01-01\"^^<http://www.w3.org/2001/XMLSchema#date>)\n\
}}\nORDER BY ?summary\nOFFSET {offset}\nLIMIT {limit}",
        next = year + 1
    )
}

/// Tous les triplets `?p ?o` du work d'un CELEX donné — le dump CDM complet
/// (subject-matter, juge rapporteur / AG / formation, `work_cites_work`,
/// `interpretes_resource_legal`, procédure, pays de renvoi…), que la liste paginée
/// ne fournit pas. Le work est désigné par son `resource_legal_id_celex`.
///
/// `case-law_joins_case_court` takes a branch of its own: its object is
/// sometimes a URI bearing the case number, sometimes a cellar UUID bearing
/// none, and only the case resource holds the `dossier_identifier` that names
/// it. Without that hop a link may name nothing.
fn work_describe_sparql(celex: &str) -> String {
    format!(
        "PREFIX cdm: <http://publications.europa.eu/ontology/cdm#>\n\
SELECT ?p ?o WHERE {{\n\
  ?work cdm:resource_legal_id_celex \"{celex}\"^^<http://www.w3.org/2001/XMLSchema#string> .\n\
  {{ ?work ?p ?o . FILTER(?p != cdm:case-law_joins_case_court) }}\n\
  UNION\n\
  {{ ?work cdm:case-law_joins_case_court ?case .\n\
     OPTIONAL {{ ?case cdm:dossier_identifier ?id }}\n\
     BIND(cdm:case-law_joins_case_court AS ?p)\n\
     BIND(COALESCE(?id, STR(?case)) AS ?o) }}\n\
}}"
    )
}

/// Joined cases of one CELEX year: `?celex` is the work that judges, `?joint` the
/// case it settles with it, in the same shape as [`work_describe_sparql`]. Feeds
/// the catch-up over the already ingested corpus, which the per-CELEX describe
/// does not replay.
///
/// The year partitions the sweep: Virtuoso refuses a sorted query whose
/// `OFFSET + LIMIT` passes its cap, which the whole relation would cross.
pub fn joined_cases_sparql(year: i32, offset: usize, limit: usize) -> String {
    format!(
        "PREFIX cdm: <http://publications.europa.eu/ontology/cdm#>\n\
SELECT ?celex ?joint WHERE {{\n\
  ?work cdm:case-law_joins_case_court ?case .\n\
  ?work cdm:resource_legal_id_celex ?celex .\n\
  FILTER(STRSTARTS(STR(?celex), \"6{year}\"))\n\
  OPTIONAL {{ ?case cdm:dossier_identifier ?id }}\n\
  BIND(COALESCE(?id, STR(?case)) AS ?joint)\n\
}}\nORDER BY ?celex ?joint\nOFFSET {offset}\nLIMIT {limit}"
    )
}

/// Row cap of the consolidated-versions query: a full answer is truncated.
const CONSOLIDATIONS_LIMIT: usize = 10_000;

/// Réduit les lignes d'une page SPARQL de validité : par CELEX, la fin la plus
/// tardive, et à part les CELEX que le sentinel déclare sans terme.
fn reduire_bornes(
    page: &Value,
    ends: &mut std::collections::HashMap<String, NaiveDate>,
    sans_terme: &mut std::collections::HashSet<String>,
) -> Result<()> {
    for row in page["results"]["bindings"].as_array().into_iter().flatten() {
        let (Some(celex), Some(end)) =
            (row["celex"]["value"].as_str(), row["end"]["value"].as_str())
        else {
            continue;
        };
        if end == NO_END_OF_VALIDITY {
            sans_terme.insert(celex.to_string());
            continue;
        }
        let fin = NaiveDate::parse_from_str(end, "%Y-%m-%d").map_err(|e| {
            SourceError::Invalid(format!("borne de validité {celex} = {end:?}: {e}"))
        })?;
        ends.entry(celex.to_string())
            .and_modify(|f: &mut NaiveDate| *f = (*f).max(fin))
            .or_insert(fin);
    }
    Ok(())
}

/// Borne de validité d'un lot d'actes, par CELEX de base.
fn validity_sparql(bases: &[String]) -> String {
    let values = bases
        .iter()
        .map(|c| format!("\"{c}\"^^xsd:string"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "PREFIX cdm: <http://publications.europa.eu/ontology/cdm#>\n\
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n\
SELECT DISTINCT ?celex ?end WHERE {{\n\
  VALUES ?celex {{ {values} }}\n\
  ?w cdm:resource_legal_id_celex ?celex .\n\
  ?w cdm:resource_legal_date_end-of-validity ?end .\n\
}}"
    )
}

/// Date of the document of a chunk of acts, by CELEX.
fn document_date_sparql(bases: &[String]) -> String {
    let values = bases
        .iter()
        .map(|c| format!("\"{c}\"^^xsd:string"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "PREFIX cdm: <http://publications.europa.eu/ontology/cdm#>\n\
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n\
SELECT DISTINCT ?celex ?date WHERE {{\n\
  VALUES ?celex {{ {values} }}\n\
  ?w cdm:resource_legal_id_celex ?celex .\n\
  ?w cdm:work_date_document ?date .\n\
}}"
    )
}

/// Consolidated works based on a chunk of acts, by base CELEX.
fn consolidated_versions_sparql(bases: &[String]) -> String {
    let values = bases
        .iter()
        .map(|c| format!("\"{c}\"^^xsd:string"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "PREFIX cdm: <http://publications.europa.eu/ontology/cdm#>\n\
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n\
SELECT DISTINCT ?base ?cons WHERE {{\n\
  VALUES ?base {{ {values} }}\n\
  ?b cdm:resource_legal_id_celex ?base .\n\
  ?c cdm:act_consolidated_based_on_resource_legal ?b .\n\
  ?c cdm:resource_legal_id_celex ?cons .\n\
}}\nLIMIT {CONSOLIDATIONS_LIMIT}"
    )
}

/// Secondary-law resource types swept from the registry. The delegated and
/// implementing acts are most of it, and they cost the weekly net one `VALUES`
/// slot each, never the daily run, which asks a single SPARQL delta whatever the
/// size of the registry. Left out, `DEC`, `DEC_IMPL`,
/// `RECO`, `CORRIGENDUM` and the budgets.
const LEGISLATION_TYPES: &[&str] = &["REG", "DIR", "REG_IMPL", "REG_DEL", "DIR_IMPL", "DIR_DEL"];

/// SPARQL listing of secondary law (sector 3) matching `filter` on `?work`, FR
/// title and document date included. The title is joined here rather than asked
/// act by act: one page answers what notices would cost a request each.
fn legislation_sparql(filter: &str, offset: usize, limit: usize) -> String {
    let types = LEGISLATION_TYPES
        .iter()
        .map(|t| format!("<http://publications.europa.eu/resource/authority/resource-type/{t}>"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "PREFIX cdm: <http://publications.europa.eu/ontology/cdm#>\n\
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n\
SELECT DISTINCT ?celex ?date ?title WHERE {{\n\
  ?work cdm:work_has_resource-type ?type .\n\
  VALUES ?type {{ {types} }}\n\
  ?work cdm:resource_legal_id_celex ?celex .\n\
  FILTER(STRSTARTS(STR(?celex), \"3\") && !CONTAINS(STR(?celex), \"R(\"))\n\
  ?work cdm:work_date_document ?date .\n\
  {filter}\n\
  ?exp cdm:expression_belongs_to_work ?work .\n\
  ?exp cdm:expression_uses_language <http://publications.europa.eu/resource/authority/language/FRA> .\n\
  ?exp cdm:expression_title ?title .\n\
}}\nORDER BY ?celex\nOFFSET {offset}\nLIMIT {limit}"
    )
}

/// One year of the registry, by document date: the partition of the full sweep,
/// a year staying under the OFFSET cap.
fn legislation_page_sparql(year: i32, offset: usize, limit: usize) -> String {
    legislation_sparql(
        &format!(
            "FILTER(?date >= \"{year}-01-01\"^^xsd:date && ?date < \"{next}-01-01\"^^xsd:date)",
            next = year + 1
        ),
        offset,
        limit,
    )
}

/// The acts CELLAR registered since `since` (AAAA-MM-JJ, inclusive). Keyed on
/// the registration and not on the document date, which precedes the OJ
/// publication by up to a year (32026R0144, dated 14/01/2025, published
/// 21/01/2026). The property is on every act of the six types.
fn legislation_since_sparql(since: &str, offset: usize, limit: usize) -> String {
    legislation_sparql(
        &format!(
            "?work cdm:work_date_creation_legacy ?created .\n  \
             FILTER(?created >= \"{since}\"^^xsd:date)"
        ),
        offset,
        limit,
    )
}

/// One year of the registry acts whose FR expression has a Formex 4
/// manifestation, the only ones `formex_act_fr` can read a base act from.
fn formex_fr_sparql(year: i32, offset: usize, limit: usize) -> String {
    let types = LEGISLATION_TYPES
        .iter()
        .map(|t| format!("<http://publications.europa.eu/resource/authority/resource-type/{t}>"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "PREFIX cdm: <http://publications.europa.eu/ontology/cdm#>\n\
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n\
SELECT DISTINCT ?celex WHERE {{\n\
  ?work cdm:work_has_resource-type ?type .\n\
  VALUES ?type {{ {types} }}\n\
  ?work cdm:resource_legal_id_celex ?celex .\n\
  FILTER(STRSTARTS(STR(?celex), \"3\") && !CONTAINS(STR(?celex), \"R(\"))\n\
  ?work cdm:work_date_document ?date .\n\
  FILTER(?date >= \"{year}-01-01\"^^xsd:date && ?date < \"{next}-01-01\"^^xsd:date)\n\
  ?exp cdm:expression_belongs_to_work ?work .\n\
  ?exp cdm:expression_uses_language <http://publications.europa.eu/resource/authority/language/FRA> .\n\
  ?manif cdm:manifestation_manifests_expression ?exp .\n\
  ?manif cdm:manifestation_type \"fmx4\"^^xsd:string .\n\
}}\nORDER BY ?celex\nOFFSET {offset}\nLIMIT {limit}",
        next = year + 1
    )
}

/// Base CELEX (sector 3) of every regulation or directive that has a
/// consolidated series, under the OFFSET cap.
fn consolidated_bases_sparql(offset: usize, limit: usize) -> String {
    format!(
        "PREFIX cdm: <http://publications.europa.eu/ontology/cdm#>\n\
SELECT DISTINCT ?base WHERE {{\n\
  ?w cdm:act_consolidated_date ?v .\n\
  ?w cdm:resource_legal_id_celex ?celex .\n\
  FILTER(STRSTARTS(STR(?celex), \"0\"))\n\
  BIND(CONCAT(\"3\", SUBSTR(STRBEFORE(STR(?celex), \"-\"), 2)) AS ?base)\n\
  FILTER(REGEX(?base, \"^3[0-9]{{4}}[RL]\"))\n\
}}\nORDER BY ?base\nOFFSET {offset}\nLIMIT {limit}"
    )
}

/// XHTML or HTML manifestations of `celex` in `lang` (uppercase Publications
/// Office code), XHTML first.
fn text_manifestation_sparql(celex: &str, lang: &str) -> String {
    format!(
        "PREFIX cdm: <http://publications.europa.eu/ontology/cdm#>\n\
SELECT ?manif WHERE {{\n\
  ?work cdm:resource_legal_id_celex \"{celex}\"^^<http://www.w3.org/2001/XMLSchema#string> .\n\
  ?exp cdm:expression_belongs_to_work ?work .\n\
  ?exp cdm:expression_uses_language <http://publications.europa.eu/resource/authority/language/{lang}> .\n\
  ?manif cdm:manifestation_manifests_expression ?exp .\n\
  ?manif cdm:manifestation_type ?type .\n\
  FILTER(STR(?type) IN (\"xhtml\", \"html\"))\n\
}} ORDER BY DESC(STR(?type) = \"xhtml\")"
    )
}

/// FR Formex 4 manifestation of a base act. The CELEX alias
/// `{celex}.FRA.fmx4` answers 404 for base acts, the OJ publishing act by act
/// under its own alias (`resource/oj/L_202302831.FRA.fmx4`); a consolidated
/// version still resolves through it.
fn formex_manifestation_sparql(celex: &str) -> String {
    format!(
        "PREFIX cdm: <http://publications.europa.eu/ontology/cdm#>\n\
SELECT ?manif WHERE {{\n\
  ?work cdm:resource_legal_id_celex \"{celex}\"^^<http://www.w3.org/2001/XMLSchema#string> .\n\
  ?exp cdm:expression_belongs_to_work ?work .\n\
  ?exp cdm:expression_uses_language <http://publications.europa.eu/resource/authority/language/FRA> .\n\
  ?manif cdm:manifestation_manifests_expression ?exp .\n\
  ?manif cdm:manifestation_type \"fmx4\"^^<http://www.w3.org/2001/XMLSchema#string> .\n\
}} LIMIT 1"
    )
}

/// Requête titre-FR + date d'un acte législatif par CELEX. Le titre FR officiel
/// porte le numéro année/séquence en tête.
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

/// Client EUR-Lex (reqwest async + TracingMiddleware).
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
                // Timeout par requête BORNÉ : une connexion morte ou half-open laisserait
                // `send().await` attendre indéfiniment. Au-delà, la requête erreur et
                // `get_with_retry` réessaie.
                .timeout(Duration::from_secs(120))
                // Pas de réutilisation de connexion : les fetchs sont séquentiels et une connexion
                // poolée finit par se figer sur ce chemin réseau — chaque tentative repart donc
                // sur une connexion neuve.
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
    /// (200/404/406/erreur) reste à l'appelant.
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
                // Erreur d'envoi (timeout, connexion coupée) = transitoire : on réessaie sur une
                // connexion neuve tant qu'il reste des tentatives.
                Err(e) if attempt + 1 < CJUE_RETRIES => {
                    tracing::warn!(what, attempt, error = %e, "eur-lex request failed (send/timeout), retry");
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

    /// The Publications Office codes (`FRA`, `DEU`…) each CELEX of `year` is published in.
    pub async fn languages_of_year(&self, year: i32) -> Result<HashMap<String, Vec<String>>> {
        let mut out = HashMap::new();
        let mut offset = 0;
        loop {
            let params: [(&str, String); 2] = [
                ("query", languages_sparql(year, offset, PAGE_SIZE)),
                ("format", "application/sparql-results+json".to_string()),
            ];
            let (status, text) = self
                .get_with_retry(
                    &self.sparql_url,
                    &params,
                    "application/sparql-results+json",
                    None,
                    "sparql languages",
                )
                .await?;
            if status != 200 {
                return Err(SourceError::Invalid(format!(
                    "eur-lex sparql languages statut {status}: {}",
                    text.chars().take(200).collect::<String>()
                )));
            }
            let page: Value = sonic_rs::from_str(&text)?;
            let rows = page["results"]["bindings"].as_array().map_or(0, Vec::len);
            for row in page["results"]["bindings"].as_array().into_iter().flatten() {
                let (Some(celex), Some(langs)) = (
                    row["celex"]["value"].as_str(),
                    row["langs"]["value"].as_str(),
                ) else {
                    continue;
                };
                let codes = langs
                    .split_whitespace()
                    .filter_map(|uri| uri.rsplit('/').next())
                    .map(str::to_string)
                    .collect();
                out.insert(celex.to_string(), codes);
            }
            if rows < PAGE_SIZE {
                return Ok(out);
            }
            offset += PAGE_SIZE;
        }
    }

    /// Every summary of a year: `(summary CELEX, decision CELEX)`.
    pub async fn summaries_of_year(&self, year: i32) -> Result<Vec<(String, String)>> {
        let mut out = Vec::new();
        let mut offset = 0;
        loop {
            let params: [(&str, String); 2] = [
                ("query", summaries_sparql(year, offset, PAGE_SIZE)),
                ("format", "application/sparql-results+json".to_string()),
            ];
            let (status, text) = self
                .get_with_retry(
                    &self.sparql_url,
                    &params,
                    "application/sparql-results+json",
                    None,
                    "sparql summaries",
                )
                .await?;
            if status != 200 {
                return Err(SourceError::Invalid(format!(
                    "eur-lex sparql summaries statut {status}: {}",
                    text.chars().take(200).collect::<String>()
                )));
            }
            let page: Value = sonic_rs::from_str(&text)?;
            let rows: Vec<(String, String)> = page["results"]["bindings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| {
                    Some((
                        row["summary"]["value"].as_str()?.to_string(),
                        row["celex"]["value"].as_str()?.to_string(),
                    ))
                })
                .collect();
            let full = rows.len() == PAGE_SIZE;
            out.extend(rows);
            if !full {
                return Ok(out);
            }
            offset += PAGE_SIZE;
        }
    }

    /// The court's summary `celex` in `lang` (`fra`, `deu`…) as served, xhtml else
    /// html, through the first item a `300 Multiple Choices` lists. `Ok(None)` when no
    /// such rendition exists: the oldest summaries are served as PDF alone.
    pub async fn summary_html_in(&self, celex: &str, lang: &str) -> Result<Option<String>> {
        let url = format!("{}/{celex}", self.resource_base);
        let item =
            Regex::new(r#"https?://publications\.europa\.eu/resource/cellar/[^\s)<"]+/DOC_\d+"#)
                .expect("regex items cellar");
        for accept in ACCEPT_CASCADE {
            let what = format!("eur-lex summary {celex} ({lang})");
            let (status, body) = self
                .get_with_retry(&url, &[], accept, Some(lang), &what)
                .await?;
            match status {
                200 => return Ok(Some(body)),
                300 => {
                    let Some(doc) = item.find(&body).map(|m| m.as_str().to_string()) else {
                        continue;
                    };
                    let (status, body) =
                        self.get_with_retry(&doc, &[], accept, None, &what).await?;
                    if status == 200 {
                        return Ok(Some(body));
                    }
                }
                404 | 406 => {}
                s if (300..400).contains(&s) => {}
                s => {
                    return Err(SourceError::Invalid(format!(
                        "eur-lex summary statut {s} {celex} (accept={accept}, lang={lang})"
                    )))
                }
            }
        }
        Ok(None)
    }

    /// The court's summary `celex` in `lang` as a PDF, the only rendition of those printed before 2005.
    /// A staff case is printed twice, its abstract in the staff cases' Reports (`_FPIA_`) and its
    /// summary beside the judgment, which a `300 Multiple Choices` is followed to.
    pub async fn summary_pdf_in(&self, celex: &str, lang: &str) -> Result<Option<Vec<u8>>> {
        let url = format!("{}/{celex}", self.resource_base);
        let mut response = self.get_pdf(&url, Some(lang), celex).await?;
        if response.status().as_u16() == 300 {
            let body = response.text().await?;
            let item = Regex::new(
                r#"(?s)href="(https?://publications\.europa\.eu/resource/cellar/[^"]+/DOC_\d+)".*?title="stream_name">([^<]*)<"#,
            )
            .expect("regex items cellar");
            let items: Vec<(&str, &str)> = item
                .captures_iter(&body)
                .map(|c| {
                    (
                        c.get(1).expect("href").as_str(),
                        c.get(2).expect("name").as_str(),
                    )
                })
                .collect();
            let Some(&(doc, _)) = items
                .iter()
                .find(|(_, name)| !name.contains("_FPIA_"))
                .or(items.first())
            else {
                return Err(SourceError::Invalid(format!(
                    "eur-lex summary pdf {celex} ({lang}): a 300 without items"
                )));
            };
            response = self.get_pdf(doc, None, celex).await?;
        }
        match response.status().as_u16() {
            200 => Ok(Some(response.bytes().await?.to_vec())),
            404 | 406 => Ok(None),
            s => Err(SourceError::Invalid(format!(
                "eur-lex summary pdf statut {s} {celex} ({lang})"
            ))),
        }
    }

    async fn get_pdf(
        &self,
        url: &str,
        lang: Option<&str>,
        celex: &str,
    ) -> Result<reqwest::Response> {
        let mut request = self.client.get(url).header("accept", "application/pdf");
        if let Some(lang) = lang {
            request = request.header("accept-language", lang);
        }
        request.send().await.map_err(|e| {
            SourceError::Invalid(format!("eur-lex summary pdf {celex} ({lang:?}): {e}"))
        })
    }

    /// One page of a year's legislative registry: `(celex, date, FR title)`, in
    /// CELEX order. A full page means more remain, the caller advances `offset`.
    pub async fn legislation_page(
        &self,
        year: i32,
        offset: usize,
    ) -> Result<Vec<(String, String, String)>> {
        self.legislation_query(legislation_page_sparql(year, offset, PAGE_SIZE))
            .await
    }

    /// One page of the acts registered since `since`, same shape as
    /// [`Self::legislation_page`].
    pub async fn legislation_since_page(
        &self,
        since: &str,
        offset: usize,
    ) -> Result<Vec<(String, String, String)>> {
        self.legislation_query(legislation_since_sparql(since, offset, PAGE_SIZE))
            .await
    }

    async fn legislation_query(&self, query: String) -> Result<Vec<(String, String, String)>> {
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
                "sparql registre législatif",
            )
            .await?;
        if status != 200 {
            return Err(SourceError::Invalid(format!(
                "eur-lex sparql registre statut {status}: {}",
                text.chars().take(200).collect::<String>()
            )));
        }
        let page: Value = sonic_rs::from_str(&text)?;
        let mut out = Vec::new();
        for row in page["results"]["bindings"].as_array().into_iter().flatten() {
            let (Some(celex), Some(date), Some(title)) = (
                row["celex"]["value"].as_str(),
                row["date"]["value"].as_str(),
                row["title"]["value"].as_str(),
            ) else {
                continue;
            };
            out.push((celex.to_string(), date.to_string(), title.to_string()));
        }
        Ok(out)
    }

    /// Base CELEX of every act a probe can find a Formex for: its base act has
    /// a FR Formex 4 manifestation, or it has a consolidated series. One SPARQL
    /// listing per year, where probing an act without costs a notice and a
    /// manifestation request each.
    pub async fn acts_with_formex(
        &self,
        years: std::ops::RangeInclusive<i32>,
    ) -> Result<std::collections::HashSet<String>> {
        let mut out = std::collections::HashSet::new();
        for year in years {
            let mut offset = 0usize;
            loop {
                let page = self
                    .sparql_column(formex_fr_sparql(year, offset, PAGE_SIZE), "celex")
                    .await?;
                let pleine = page.len() >= PAGE_SIZE;
                out.extend(page);
                if !pleine {
                    break;
                }
                offset += PAGE_SIZE;
            }
        }
        let mut offset = 0usize;
        loop {
            let page = self
                .sparql_column(consolidated_bases_sparql(offset, PAGE_SIZE), "base")
                .await?;
            let pleine = page.len() >= PAGE_SIZE;
            out.extend(page);
            if !pleine {
                break;
            }
            offset += PAGE_SIZE;
        }
        Ok(out)
    }

    async fn sparql_column(&self, query: String, var: &str) -> Result<Vec<String>> {
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
                "sparql formex",
            )
            .await?;
        if status != 200 {
            return Err(SourceError::Invalid(format!(
                "eur-lex sparql formex statut {status}: {}",
                text.chars().take(200).collect::<String>()
            )));
        }
        let page: Value = sonic_rs::from_str(&text)?;
        Ok(page["results"]["bindings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| row[var]["value"].as_str().map(str::to_string))
            .collect())
    }

    /// The date each act stops being in force, by base CELEX; absent means no end.
    /// The end of the acts is a **set**, asked at once, where the same answer would
    /// cost one HTTP notice per act.
    ///
    /// `resource_legal_in-force` is not read: it contradicts the end date on some
    /// acts of the fond (flag at 1, end dated in the past).
    ///
    /// An act sometimes carries several ends, the « no end » sentinel included,
    /// mixed with a past date. The sentinel vetoes, else the latest end wins, so
    /// the order of the rows never decides.
    pub async fn acts_end_of_validity(
        &self,
        bases: &[String],
    ) -> Result<std::collections::HashMap<String, NaiveDate>> {
        let mut ends = std::collections::HashMap::new();
        let mut sans_terme = std::collections::HashSet::new();
        // A CELEX twice in one `VALUES` stalls CELLAR's Virtuoso.
        let mut bases = bases.to_vec();
        bases.sort_unstable();
        bases.dedup();
        for chunk in bases.chunks(VALIDITY_CHUNK) {
            let params: [(&str, String); 2] = [
                ("query", validity_sparql(chunk)),
                ("format", "application/sparql-results+json".to_string()),
            ];
            let (status, text) = self
                .get_with_retry(
                    &self.sparql_url,
                    &params,
                    "application/sparql-results+json",
                    None,
                    "sparql borne de validité",
                )
                .await?;
            if status != 200 {
                return Err(SourceError::Invalid(format!(
                    "eur-lex sparql validité statut {status}: {}",
                    text.chars().take(200).collect::<String>()
                )));
            }
            let page: Value = sonic_rs::from_str(&text)?;
            reduire_bornes(&page, &mut ends, &mut sans_terme)?;
        }
        ends.retain(|celex, _| !sans_terme.contains(celex));
        Ok(ends)
    }

    /// Date of the document of each act, by CELEX; the earliest when CELLAR holds
    /// several. An act without one is absent.
    pub async fn acts_document_dates(
        &self,
        bases: &[String],
    ) -> Result<std::collections::HashMap<String, NaiveDate>> {
        let mut dates = std::collections::HashMap::new();
        let mut bases = bases.to_vec();
        bases.sort_unstable();
        bases.dedup();
        for chunk in bases.chunks(VALIDITY_CHUNK) {
            let params: [(&str, String); 2] = [
                ("query", document_date_sparql(chunk)),
                ("format", "application/sparql-results+json".to_string()),
            ];
            let (status, text) = self
                .get_with_retry(
                    &self.sparql_url,
                    &params,
                    "application/sparql-results+json",
                    None,
                    "sparql document date",
                )
                .await?;
            if status != 200 {
                return Err(SourceError::Invalid(format!(
                    "eur-lex sparql document date status {status}: {}",
                    text.chars().take(200).collect::<String>()
                )));
            }
            let page: Value = sonic_rs::from_str(&text)?;
            for row in page["results"]["bindings"].as_array().into_iter().flatten() {
                let (Some(celex), Some(date)) = (
                    row["celex"]["value"].as_str(),
                    row["date"]["value"].as_str(),
                ) else {
                    continue;
                };
                let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|e| {
                    SourceError::Invalid(format!("document date {celex} = {date:?}: {e}"))
                })?;
                dates
                    .entry(celex.to_string())
                    .and_modify(|d: &mut NaiveDate| *d = (*d).min(date))
                    .or_insert(date);
            }
        }
        Ok(dates)
    }

    /// Text of a CELEX in one language (`lang`, Publications Office code in
    /// lowercase: `fra`, `deu`…), through the `Accept` cascade (xhtml then html).
    /// `Ok(None)` when no rendition is served in that language (404/406/3xx);
    /// any other HTTP status is an error.
    pub async fn resource_text_in(&self, celex: &str, lang: &str) -> Result<Option<String>> {
        let url = format!("{}/{celex}", self.resource_base);
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
                return Ok(Some(strip_html(&text)));
            }
            last_status = status;
            // 3xx (notably 300 Multiple Choices): EUR-Lex serves no single representation.
            let skippable = status == 404 || status == 406 || (300..400).contains(&status);
            if !skippable {
                return Err(SourceError::Invalid(format!(
                    "eur-lex resource statut {status} {celex} (accept={accept}, lang={lang})"
                )));
            }
        }
        if let Some(text) = self.manifestation_text(celex, lang).await? {
            return Ok(Some(text));
        }
        if let Some(text) = self.pdf_text(celex, lang).await? {
            return Ok(Some(text));
        }
        if last_status != 404 && last_status != 406 {
            tracing::warn!(
                celex,
                lang,
                status = last_status,
                "eur-lex resource without a single rendition (3xx/non-404)"
            );
        }
        Ok(None)
    }

    /// Text of the PDF of `celex` in `lang`, the only rendition of the decisions of the 1950s.
    async fn pdf_text(&self, celex: &str, lang: &str) -> Result<Option<String>> {
        let url = format!("{}/{celex}", self.resource_base);
        let response = self
            .client
            .get(&url)
            .header("accept", PDF_ACCEPT)
            .header("accept-language", lang)
            .send()
            .await
            .map_err(|e| SourceError::Invalid(format!("eur-lex pdf {celex} ({lang}): {e}")))?;
        if response.status() != reqwest::StatusCode::OK {
            return Ok(None);
        }
        let raw = crate::pdf::pdftotext_extract(&response.bytes().await?)?;
        Ok(Some(lj_parse::reflow_page_text(&raw)).filter(|t| !t.trim().is_empty()))
    }

    /// Text of the XHTML or HTML manifestation of `celex` in `lang`, found through the catalogue:
    /// the CELEX negotiation answers 404 for some French texts it holds.
    async fn manifestation_text(&self, celex: &str, lang: &str) -> Result<Option<String>> {
        let query = text_manifestation_sparql(celex, &lang.to_ascii_uppercase());
        let Some(manif) = self.sparql_column(query, "manif").await?.into_iter().next() else {
            return Ok(None);
        };
        let (status, rdf) = self
            .get_with_retry(
                &manif,
                &[],
                "*/*",
                None,
                &format!("cellar {celex} ({lang})"),
            )
            .await?;
        if status != 200 {
            return Ok(None);
        }
        let item = Regex::new(
            r#"rdf:about="(https?://publications\.europa\.eu/resource/cellar/[^"]+/DOC_\d+)""#,
        )
        .expect("regex items cellar");
        let Some(doc) = item.captures(&rdf).map(|c| c[1].to_string()) else {
            return Ok(None);
        };
        let (status, text) = self
            .get_with_retry(&doc, &[], "*/*", None, &format!("cellar item {doc}"))
            .await?;
        // Its `<head>` titles the document by its CELEX, which no reader wants.
        let head = Regex::new(r"(?is)<head\b.*?</head>").expect("regex head");
        Ok((status == 200)
            .then(|| drop_contents(&strip_html(&head.replace(&text, ""))).to_string()))
    }

    /// Prédicats CDM riches d'un work par CELEX (SPARQL `?p ?o`), mappés en
    /// `source_fields` lisibles via [`map_predicates`]. Une réponse non-200 remonte
    /// franchement — l'appelant logue et retombe sur le minimum `{ecli, date}`.
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

    /// One raw page of [`joined_cases_sparql`].
    pub async fn joined_cases_page(&self, year: i32, offset: usize, limit: usize) -> Result<Value> {
        let params: [(&str, String); 2] = [
            ("query", joined_cases_sparql(year, offset, limit)),
            ("format", "application/sparql-results+json".to_string()),
        ];
        let (status, text) = self
            .get_with_retry(
                &self.sparql_url,
                &params,
                "application/sparql-results+json",
                None,
                &format!("sparql affaires jointes {year} offset={offset}"),
            )
            .await?;
        if status != 200 {
            return Err(SourceError::Invalid(format!(
                "eur-lex sparql affaires jointes {year} offset={offset} statut {status}: {}",
                text.chars().take(200).collect::<String>()
            )));
        }
        Ok(sonic_rs::from_str(&text)?)
    }

    /// Métadonnées FR d'un acte législatif UE par CELEX : titre officiel français +
    /// date du document. `Ok(None)` si le CELEX n'existe pas ou n'a pas de titre FR (→
    /// SKIP à l'ingestion). Le titre porte le numéro année/séquence en tête
    /// (« Règlement (UE) 2016/679 … », « Sixième directive 77/388/CEE … ») — la forme
    /// exacte que la passe de résolution ré-apparie contre `legal_text.title`.
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

    // ————— Formex CELLAR (fond eu-law synchronisé) —————

    /// Dates (`AAAAMMJJ`, sorted, deduplicated) of each act's consolidated
    /// versions, by base CELEX; an act never consolidated maps to an empty list.
    /// One query per chunk where the notice branch costs a request per act, for
    /// the same versions.
    pub async fn acts_consolidated_versions(
        &self,
        bases: &[String],
    ) -> Result<std::collections::HashMap<String, Vec<String>>> {
        let mut bases = bases.to_vec();
        bases.sort_unstable();
        bases.dedup();
        let mut out: std::collections::HashMap<String, Vec<String>> =
            bases.iter().map(|b| (b.clone(), Vec::new())).collect();
        for chunk in bases.chunks(VALIDITY_CHUNK) {
            let params: [(&str, String); 2] = [
                ("query", consolidated_versions_sparql(chunk)),
                ("format", "application/sparql-results+json".to_string()),
            ];
            let (status, text) = self
                .get_with_retry(
                    &self.sparql_url,
                    &params,
                    "application/sparql-results+json",
                    None,
                    "sparql consolidated versions",
                )
                .await?;
            if status != 200 {
                return Err(SourceError::Invalid(format!(
                    "eur-lex sparql consolidated versions status {status}: {}",
                    text.chars().take(200).collect::<String>()
                )));
            }
            let page: Value = sonic_rs::from_str(&text)?;
            let rows = page["results"]["bindings"].as_array();
            if rows.is_some_and(|r| r.len() >= CONSOLIDATIONS_LIMIT) {
                return Err(SourceError::Invalid(format!(
                    "eur-lex sparql consolidated versions truncated at {CONSOLIDATIONS_LIMIT} rows"
                )));
            }
            for row in rows.into_iter().flatten() {
                let (Some(base), Some(cons)) =
                    (row["base"]["value"].as_str(), row["cons"]["value"].as_str())
                else {
                    continue;
                };
                if let (Some(r), Some(versions)) =
                    (parse_consolidated_celex(cons), out.get_mut(base))
                {
                    if base.get(1..) == Some(r.body.as_str()) {
                        versions.push(r.version);
                    }
                }
            }
        }
        for versions in out.values_mut() {
            versions.sort();
            versions.dedup();
        }
        Ok(out)
    }

    /// Formex 4 FR d'un CELEX (de base, ou consolidé `0{corps}-{AAAAMMJJ}`) :
    /// la manifestation `.FRA.fmx4` est un RDF qui liste les items cellar
    /// `DOC_n` ; chaque item est du Formex nu **ou un ZIP** (actes à figures).
    /// Returns the act document (`<ACT>`/`<CONS.ACT>`, the one carrying the most
    /// `<ARTICLE>`) first, then the annex volumes (`<ANNEX>`) a base act
    /// publishes as separate documents; a consolidated one carries its own
    /// inline, as `<CONS.ANNEX>`. Empty: no manifestation (pre-Formex act,
    /// purely amending directive), an act document with no article, or an item
    /// CELLAR does not serve.
    ///
    /// One unserved item abandons the manifestation instead of returning what
    /// came through: the caller dates what it reads, and a half-fetched form
    /// would close the articles that merely failed to download. Old consolidated
    /// versions answer 500 item after item.
    pub async fn formex_act_fr(&self, celex: &str) -> Result<Vec<String>> {
        let url = if celex.starts_with('3') {
            match self
                .sparql_column(formex_manifestation_sparql(celex), "manif")
                .await?
                .into_iter()
                .next()
            {
                Some(manif) => manif,
                None => return Ok(Vec::new()),
            }
        } else {
            format!("{}/{celex}.FRA.fmx4", self.resource_base)
        };
        let (status, rdf) = self
            .get_with_retry(
                &url,
                &[],
                "*/*",
                Some("fra"),
                &format!("cellar fmx4 {celex}"),
            )
            .await?;
        if status == 404 || status == 406 || (300..400).contains(&status) {
            return Ok(Vec::new());
        }
        if status != 200 {
            return Err(SourceError::Invalid(format!(
                "cellar fmx4 {celex} statut {status}"
            )));
        }
        let re = Regex::new(
            r#"rdf:about="(https?://publications\.europa\.eu/resource/cellar/[^"]+/DOC_\d+)""#,
        )
        .expect("regex items cellar");
        let mut docs: Vec<String> = re.captures_iter(&rdf).map(|c| c[1].to_string()).collect();
        docs.sort();
        docs.dedup();

        let act_root = Regex::new(r"<(CONS\.ACT|ACT)[\s>]").expect("regex racine acte");
        let annex_root = Regex::new(r"<ANNEX[\s>]").expect("regex racine annexe");
        let mut best: Option<(usize, String)> = None;
        let mut annexes: Vec<String> = Vec::new();
        for doc in &docs {
            let (status, bytes) = self
                .get_bytes_with_retry(doc, &format!("cellar item {doc}"))
                .await?;
            if status != 200 {
                tracing::warn!(
                    celex,
                    doc,
                    status,
                    "cellar item not served, manifestation dropped"
                );
                return Ok(Vec::new());
            }
            let bodies = if bytes.starts_with(b"PK") {
                zip_xml_members(&bytes)?
            } else {
                vec![String::from_utf8_lossy(&bytes).into_owned()]
            };
            for body in bodies {
                if act_root.is_match(&body) {
                    let n = body.matches("<ARTICLE").count();
                    if n > 0 && best.as_ref().is_none_or(|(bn, _)| n > *bn) {
                        best = Some((n, body));
                    }
                } else if annex_root.is_match(&body) {
                    annexes.push(body);
                }
            }
        }
        let Some((_, act)) = best else {
            return Ok(Vec::new());
        };
        let mut out = vec![act];
        out.append(&mut annexes);
        Ok(out)
    }

    /// GET binaire avec le même retry borné que [`Self::get_with_retry`] — les
    /// items cellar `DOC_n` peuvent être des ZIP, `resp.text()` les corromprait.
    async fn get_bytes_with_retry(&self, url: &str, what: &str) -> Result<(u16, Vec<u8>)> {
        for attempt in 0..CJUE_RETRIES {
            if attempt > 0 {
                tokio::time::sleep(THROTTLE * (1 << attempt)).await;
            }
            let req = self
                .client
                .get(url)
                .header("accept", "*/*")
                .header("accept-language", "fra");
            let resp = match req.send().await {
                Ok(resp) => resp,
                Err(e) if attempt + 1 < CJUE_RETRIES => {
                    tracing::warn!(what, attempt, error = %e, "cellar request failed (send/timeout), retry");
                    continue;
                }
                Err(e) => return Err(SourceError::Invalid(format!("requête {what}: {e}"))),
            };
            let status = resp.status().as_u16();
            let transient = status == 429 || (500..600).contains(&status);
            if transient && attempt + 1 < CJUE_RETRIES {
                continue;
            }
            let bytes = resp.bytes().await?;
            return Ok((status, bytes.to_vec()));
        }
        unreachable!("get_bytes_with_retry: la dernière itération retourne toujours")
    }
}

/// Membres `.xml` d'un ZIP cellar en mémoire (acte + enveloppe `doc.xml` ;
/// les figures TIFF sont ignorées).
fn zip_xml_members(bytes: &[u8]) -> Result<Vec<String>> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    let mut out = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        if entry.is_dir() || !entry.name().to_lowercase().ends_with(".xml") {
            continue;
        }
        let mut raw = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut raw)?;
        out.push(String::from_utf8_lossy(&raw).into_owned());
    }
    Ok(out)
}

impl Default for CjueClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Une ligne SPARQL : CELEX (PK) + date + ECLI optionnel, jamais dérivé du CELEX.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CjueResult {
    pub celex: String,
    pub date: Option<String>,
    pub ecli: Option<String>,
    /// The French title, `#`-separated fields whose second names the parties.
    pub title: Option<String>,
}

/// Parse une réponse SPARQL JSON results en lignes dédupliquées par CELEX (la
/// liste brute multiplie un work par `resource-type`). Préserve l'ordre
/// d'apparition.
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
        let title = b["title"]["value"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if celex.is_empty() {
            continue;
        }
        if let Some(i) = seen.iter().position(|c| c == celex) {
            out[i].title = out[i].title.take().or(title);
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
            title,
        });
    }
    out
}

/// Groups a [`joined_cases_sparql`] page into `(celex, joined cases)`, in the
/// order the response gives, duplicates dropped. The values are what
/// `case-law_joins_case_court` carries in `source_fields`, so the catch-up
/// writes exactly what the ingest would have written.
pub fn parse_joined_cases(resp: &Value) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    let Some(bindings) = resp["results"]["bindings"].as_array() else {
        return out;
    };
    for b in bindings {
        let (Some(celex), Some(joint)) =
            (b["celex"]["value"].as_str(), b["joint"]["value"].as_str())
        else {
            continue;
        };
        if celex.is_empty() || joint.is_empty() {
            continue;
        }
        match out.iter_mut().find(|(c, _)| c == celex) {
            Some((_, joints)) => {
                if !joints.iter().any(|j| j == joint) {
                    joints.push(joint.to_string());
                }
            }
            None => out.push((celex.to_string(), vec![joint.to_string()])),
        }
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
/// `…/fd_100/PREJ` → `PREJ`). Les URI cellar (juge, AG, citation) n'ont pas de code
/// lisible : on garde l'URI brute.
fn authority_code_or_uri(uri: &str) -> String {
    if let Some(rest) = uri.split("/authority/").nth(1) {
        if let Some(code) = rest.rsplit('/').next() {
            return code.to_string();
        }
    }
    uri.to_string()
}

/// Shape a predicate takes in `source_fields`: scalar on a single value, JSON
/// array beyond. The catch-up over the corpus writes through here, so it cannot
/// hand `parse_cjue` a shape the ingest never produces.
pub fn predicate_value(mut vals: Vec<String>) -> Value {
    match vals.len() {
        1 => Value::String(vals.pop().expect("len==1")),
        _ => Value::Array(vals.into_iter().map(Value::String).collect()),
    }
}

/// Mappe le dump SPARQL `?p ?o` d'un work en `source_fields` lisibles. Ne retient
/// que les prédicats riches de [`RICH_PREDICATES`], la clé est le nom local du
/// prédicat. Les valeurs URI d'autorité sont réduites à leur code (`IMMI`,
/// `CHAMB_GD_C`…), les littéraux gardés verbatim, les URI cellar gardées brutes.
/// Une clé à valeur unique sort scalaire, multi-valuée sort en tableau JSON (ordre
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
    for (key, vals) in collected {
        out.insert(key, predicate_value(vals));
    }
    J::Object(out)
}

/// Extrait la bannière « objet » doctrinale d'un arrêt/ordonnance CJUE : le bloc de
/// mots-clés entre guillemets français (`« Renvoi préjudiciel – … »`) qui suit
/// l'en-tête (`ARRÊT DE LA COUR (…) <date>`). Opère sur le texte déjà strippé.
/// `None` si aucun bloc `«…»` n'est présent.
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

/// Drops the table of contents a catalogue rendition opens on: its headings, one per line, up to
/// the first one's return as a section heading, each of them heading a section of the text below.
pub fn drop_contents(text: &str) -> &str {
    let lines: Vec<&str> = text.split('\n').take(10).collect();
    let Some(k) = (2..lines.len()).find(|&k| lines[k] == lines[0]) else {
        return text;
    };
    let rest = &text[lines[..k].iter().map(|l| l.len() + 1).sum::<usize>()..];
    let headings = lines[..k]
        .iter()
        .all(|h| h.len() <= 80 && rest.split('\n').any(|l| l == *h));
    if headings {
        rest
    } else {
        text
    }
}

/// CELEX consolidé décomposé : corps (`2020R0761`) + version (`AAAAMMJJ`).
/// Le corps s'apparie au CELEX de base par `celex_base[1..]`, secteur ignoré.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsolidatedRef {
    pub body: String,
    pub version: String,
}

/// `02020R0761-20260501` → corps + version. `None` si la forme n'est pas celle
/// d'une série consolidée secteur 0 (`0{corps}-{AAAAMMJJ}`).
pub fn parse_consolidated_celex(celex: &str) -> Option<ConsolidatedRef> {
    let (head, version) = celex.split_once('-')?;
    if !celex.starts_with('0')
        || head.len() < 2
        || version.len() != 8
        || !version.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some(ConsolidatedRef {
        body: head[1..].to_string(),
        version: version.to_string(),
    })
}

/// Eu-law manifest: the registry watermark and the acts already visited.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EuLawManifest {
    /// Inclusive bound (AAAA-MM-JJ) of the next registry delta of
    /// `sync-eu-catalog`; absent, the registry is swept whole.
    #[serde(default)]
    pub catalog_since: Option<String>,
    /// Day each act (CELEX base) was last probed with an answer, Formex or none.
    /// A visited act without a body had no Formex, and is not probed again.
    #[serde(default)]
    pub visited: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub fetched_at: Option<String>,
}

impl EuLawManifest {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)?;
        serde_json::from_str(&raw)
            .map_err(|e| SourceError::Invalid(format!("manifest eu-law illisible: {e}")))
    }

    pub fn save(&mut self, path: &Path) -> Result<()> {
        self.fetched_at = Some(Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| SourceError::Invalid(format!("manifest eu-law non sérialisable: {e}")))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// Chemin du manifeste eu-law sous `data_dir` (`<data_dir>/eu_law/manifest.json`).
pub fn eu_law_manifest_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir
        .join(crate::state_paths::EU_LAW_DIR)
        .join("manifest.json")
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
        assert!(q.contains("cdm:expression_title ?title"));
    }

    #[test]
    fn urls_trailing_slash_trimmed() {
        let c = CjueClient::with_urls("https://s/sparql/", "https://r/celex/");
        assert_eq!(c.sparql_url, "https://s/sparql");
        assert_eq!(c.resource_base, "https://r/celex");
    }

    #[test]
    fn parse_sparql_dedups_by_celex_preserving_order() {
        let resp = json!({
            "results": { "bindings": [
                { "celex": { "value": "62020CJ0560" },
                  "date": { "value": "2024-01-30" },
                  "ecli": { "value": "ECLI:EU:C:2024:96" } },
                { "celex": { "value": "62020CJ0560" },
                  "date": { "value": "2024-01-30" },
                  "title": { "value": "Arrêt de la Cour du 30 janvier 2024.#X contre Y.#Affaire C-560/20." } },
                { "celex": { "value": "62023CO0614" },
                  "date": { "value": "2024-01-15" } },
            ]}
        });
        let rows = parse_sparql(&resp);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].celex, "62020CJ0560");
        assert_eq!(rows[0].ecli.as_deref(), Some("ECLI:EU:C:2024:96"));
        assert_eq!(rows[0].date.as_deref(), Some("2024-01-30"));
        assert!(rows[0].title.as_deref().unwrap().contains("#X contre Y.#"));
        assert_eq!(rows[1].celex, "62023CO0614");
        assert_eq!(rows[1].ecli, None);
        assert_eq!(rows[1].title, None);
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
        let cdm = "http://publications.europa.eu/ontology/cdm#";
        let auth = "http://publications.europa.eu/resource/authority";
        let cellar = "http://publications.europa.eu/resource/cellar";
        let resp = json!({
            "results": { "bindings": [
                pred(&format!("{cdm}work_version"), "literal", "definitive"),
                pred(
                    "http://www.w3.org/1999/02/22-rdf-syntax-ns#type",
                    "uri",
                    &format!("{cdm}judgement"),
                ),
                pred(&format!("{cdm}resource_legal_type"), "literal", "CJ"),
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
                pred(
                    &format!("{cdm}case-law_originates_in_country"),
                    "uri",
                    &format!("{auth}/country/AUT"),
                ),
                pred(
                    &format!("{cdm}case-law_uses_procedure_language"),
                    "uri",
                    &format!("{auth}/language/DEU"),
                ),
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
                pred(
                    &format!("{cdm}case-law_interpretes_resource_legal"),
                    "uri",
                    &format!("{cellar}/b81af559-84bd-45b3-9d0e-c3f70a22516e"),
                ),
                pred(&format!("{cdm}case-law_published_in_erecueil"), "literal", "1"),
                pred(
                    &format!("{cdm}case-law_article_journal_related"),
                    "literal",
                    "1. Gazin, Fabienne: Immigration …, Europe 2024, nº 3",
                ),
                pred(
                    &format!("{cdm}case-law_national-judgement"),
                    "literal",
                    "  Verwaltungsgericht Wien, Beschluss vom 25/09/2020  ",
                ),
            ]}
        });

        let m = map_predicates(&resp);
        let obj = m.as_object().expect("objet");

        assert!(!obj.contains_key("work_version"));
        assert!(!obj.contains_key("type"));

        assert_eq!(m["resource_legal_type"], json!("CJ"));
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
        assert_eq!(
            m["resource_legal_is_about_subject-matter"],
            json!(["IMMI", "ELSJ"])
        );
        assert_eq!(
            m["case-law_delivered_by_judge"],
            json!(
                "http://publications.europa.eu/resource/cellar/6bc92600-f3ca-4c3d-b588-d733c0128593"
            )
        );
        assert_eq!(
            m["work_cites_work"],
            json!([
                "http://publications.europa.eu/resource/cellar/d00948e0-b542-40be-beed-422a7fc3548e",
                "http://publications.europa.eu/resource/cellar/b81af559-84bd-45b3-9d0e-c3f70a22516e"
            ])
        );
        assert_eq!(
            m["case-law_interpretes_resource_legal"],
            json!(
                "http://publications.europa.eu/resource/cellar/b81af559-84bd-45b3-9d0e-c3f70a22516e"
            )
        );
        assert_eq!(m["case-law_published_in_erecueil"], json!("1"));
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
    fn drop_contents_keeps_a_header_that_heads_no_section() {
        let toc = "Parties\nMotifs de l'arrêt\nDispositif\nParties\nDans l'affaire T-406/08,\nMotifs de l'arrêt\n1. Faits\nDispositif\nLe recours est rejeté.";
        assert_eq!(
            drop_contents(toc),
            "Parties\nDans l'affaire T-406/08,\nMotifs de l'arrêt\n1. Faits\nDispositif\nLe recours est rejeté."
        );
        let opinion = "Provisional text\nOPINION OF ADVOCATE GENERAL\nMEDINA\ndelivered on 11 June 2026 (1)\nProvisional text\nJoined Cases C-706/25 and C-707/25";
        assert_eq!(drop_contents(opinion), opinion);
    }

    #[test]
    fn extract_objet_from_real_header() {
        let body = "62020CJ0560 ARRÊT DE LA COUR (grande chambre) 30 janvier 2024 ( *1 ) \
« Renvoi préjudiciel – Politique relative à l'immigration – Directive 2003/86/CE – \
Notion de \u{201c}mineur non accompagné\u{201d} » Dans l'affaire C-560/20, ayant pour objet …";
        let objet = extract_objet(body).expect("objet présent");
        assert!(objet.starts_with("Renvoi préjudiciel – Politique"));
        assert!(objet.contains("Directive 2003/86/CE"));
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

    #[test]
    fn registry_delta_keys_on_registration_not_document_date() {
        let q = legislation_since_sparql("2026-08-27", 1000, PAGE_SIZE);
        assert!(q.contains("cdm:work_date_creation_legacy ?created"));
        assert!(q.contains("?created >= \"2026-08-27\"^^xsd:date"));
        assert!(!q.contains("?date >="));
        assert!(q.contains("OFFSET 1000"));
        let y = legislation_page_sparql(2024, 0, PAGE_SIZE);
        assert!(y.contains("?date >= \"2024-01-01\"^^xsd:date && ?date < \"2025-01-01\""));
    }

    #[test]
    fn validity_sparql_types_each_celex_and_stays_under_the_url_cap() {
        let bases: Vec<String> = (0..VALIDITY_CHUNK).map(|i| format!("3196{i:07}")).collect();
        let q = validity_sparql(&bases);
        assert!(q.contains("cdm:resource_legal_date_end-of-validity ?end"));
        // Sans le type, le VALUES ne joint rien : la propriété CELEX est un
        // `xsd:string` typé, et un littéral nu ne l'égale pas.
        assert!(q.contains("\"31960000000\"^^xsd:string"));
        assert!(!q.contains("in-force"));
        // The batch goes by GET, and CELLAR answers 400 to an overlong URL.
        assert!(q.len() < 8_000, "requête de {} octets", q.len());
    }

    #[test]
    fn document_date_sparql_stays_under_the_url_cap() {
        let bases: Vec<String> = (0..VALIDITY_CHUNK).map(|i| format!("3196{i:07}")).collect();
        let q = document_date_sparql(&bases);
        assert!(q.contains("\"31960000000\"^^xsd:string"));
        assert!(q.len() < 8_000, "query of {} bytes", q.len());
    }

    #[test]
    fn consolidated_versions_sparql_stays_under_the_url_cap() {
        let bases: Vec<String> = (0..VALIDITY_CHUNK).map(|i| format!("3196{i:07}")).collect();
        let q = consolidated_versions_sparql(&bases);
        assert!(q.contains("\"31960000000\"^^xsd:string"));
        assert!(q.len() < 8_000, "query of {} bytes", q.len());
    }

    /// Forme rendue par CELLAR pour 31994L0022 : une directive de 1994 y porte
    /// une fin au 30/06/1993 ET le sentinel sans terme. Le sentinel tranche,
    /// sinon l'acte, en vigueur, sort du fond servi.
    #[test]
    fn le_sentinel_sans_terme_oppose_son_veto_a_une_fin_datee() {
        let page: Value = sonic_rs::from_str(
            r#"{"results":{"bindings":[
                 {"celex":{"value":"31994L0022"},"end":{"value":"1993-06-30"}},
                 {"celex":{"value":"31994L0022"},"end":{"value":"9999-12-31"}},
                 {"celex":{"value":"31964L0433"},"end":{"value":"2005-12-31"}}]}}"#,
        )
        .expect("page json");
        let mut ends = std::collections::HashMap::new();
        let mut sans_terme = std::collections::HashSet::new();
        reduire_bornes(&page, &mut ends, &mut sans_terme).expect("réduction");
        ends.retain(|celex, _| !sans_terme.contains(celex));

        assert!(!ends.contains_key("31994L0022"));
        assert_eq!(
            ends.get("31964L0433"),
            Some(&NaiveDate::from_ymd_opt(2005, 12, 31).unwrap())
        );
    }

    /// Plusieurs fins datées sans sentinel : la plus tardive, jamais la dernière
    /// ligne arrivée.
    #[test]
    fn plusieurs_fins_datees_retiennent_la_plus_tardive() {
        let page: Value = sonic_rs::from_str(
            r#"{"results":{"bindings":[
                 {"celex":{"value":"31967R0422"},"end":{"value":"2010-01-01"}},
                 {"celex":{"value":"31967R0422"},"end":{"value":"1975-06-30"}}]}}"#,
        )
        .expect("page json");
        let mut ends = std::collections::HashMap::new();
        let mut sans_terme = std::collections::HashSet::new();
        reduire_bornes(&page, &mut ends, &mut sans_terme).expect("réduction");

        assert_eq!(
            ends.get("31967R0422"),
            Some(&NaiveDate::from_ymd_opt(2010, 1, 1).unwrap())
        );
    }

    #[test]
    fn parse_consolidated_celex_splits_body_and_version() {
        let r = parse_consolidated_celex("02020R0761-20260501").expect("forme consolidée");
        assert_eq!(r.body, "2020R0761");
        assert_eq!(r.version, "20260501");
        assert_eq!(parse_consolidated_celex("32020R0761"), None);
        assert_eq!(parse_consolidated_celex("02020R0761-2026"), None);
        assert_eq!(parse_consolidated_celex("62020CJ0560"), None);
    }
}
