//! European sources: the CEDH (HUDOC) and the CJUE (EUR-Lex). Each language
//! version of a ruling is a provenance of its own, with its own text.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use futures::{StreamExt, TryStreamExt};

use lj_core::decision::Decision;
use lj_parse::{build_source_fields, celex_is_opinion, europe_names, parse_cedh, parse_cjue};
use lj_pg::db::Connection;
use lj_sources::cedh::CedhResult;
use lj_sources::curia::CuriaClient;
use lj_store::repository::DecisionRepository;
use lj_vocab::languages::{eu_of_iso_639_1, iso_639_1_of_eu};

use crate::config::Settings;

use super::batch::drain_batch;
use super::{generate_public_id, Candidate, IngestCounts, IngestMode, WriteMode, BATCH_SIZE};

/// Concurrence des pages de liste HUDOC d'une année.
const LIST_CONCURRENCY: usize = 4;

async fn present_source_uids(conn: &Connection, uids: &[String]) -> Result<HashSet<String>> {
    let repo = DecisionRepository::new(conn);
    let states = repo
        .find_ingest_states(uids)
        .await
        .map_err(|e| anyhow!("find_ingest_states: {e}"))?;
    Ok(states.into_keys().collect())
}

/// First year of the CEDH bootstrap: the Court's first judgments are of 1960.
const CEDH_START_YEAR: i32 = 1960;
/// First year of the CJUE bootstrap, the first of EUR-Lex sector 6.
const CJUE_START_YEAR: i32 = 1954;
/// An EUR-Lex text under this size may be its summary of a decision left out of the Reports.
const SHORT_TEXT_BYTES: usize = 6_000;
/// The languages a CELEX has only EUR-Lex's summary in, kept with its CDM predicates.
const SUMMARY_LANGUAGES: &str = "eurlex_summary_languages";

/// Année courante (UTC), borne haute des balayages CEDH/CJUE.
fn current_year() -> i32 {
    chrono::Utc::now()
        .date_naive()
        .format("%Y")
        .to_string()
        .parse()
        .unwrap_or(CEDH_START_YEAR)
}

/// Construit le `source_fields` HTML : les métadonnées verbatim
/// (colonnes HUDOC / prédicats CDM, qui ne portent pas de texte) plus les
/// sections rebasées sur `full_text` ([`build_source_fields`] retire `text`/
/// `zones` absents et n'ajoute que `sections`).
fn html_source_fields(metadata: &serde_json::Value, decision: &Decision) -> serde_json::Value {
    build_source_fields(metadata, &decision.sections)
}

/// Octets bruts hashés pour l'idempotence d'un document HTML CEDH/CJUE :
/// métadonnées sérialisées (colonnes/prédicats) suivies du corps brut. Un
/// changement de l'un ou l'autre invalide le checksum.
fn html_checksum(metadata: &serde_json::Value, body_text: &str) -> String {
    let mut payload = serde_json::to_vec(metadata).unwrap_or_default();
    payload.extend_from_slice(body_text.as_bytes());
    super::content_checksum(&payload)
}

/// The Publications Office code of a HUDOC language, the value
/// `decision_sources.lang` carries.
fn hudoc_lang(raw: &str) -> Result<&'static str> {
    match raw {
        "FRE" => Ok("fra"),
        "ENG" => Ok("eng"),
        other => Err(anyhow!("HUDOC language {other}")),
    }
}

/// The candidate of one language version, `lang` its Publications Office code in
/// lowercase, written into `source_fields["lang"]`.
pub(super) fn html_candidate(
    decision: Decision,
    metadata: &serde_json::Value,
    body_text: &str,
    lang: &str,
) -> Candidate {
    let mut source_fields = html_source_fields(metadata, &decision);
    source_fields["lang"] = serde_json::json!(lang);
    Candidate {
        decision_id: None,
        public_id: generate_public_id(),
        content_checksum: html_checksum(metadata, body_text),
        prebuilt_source_fields: Some(source_fields),
        prebuilt_extracted: None,
        decision,
        raw_payload: Vec::new(),
        payload_format: "html".to_string(),
        write_mode: WriteMode::Full,
        passages: true,
        distinct: false,
    }
}

/// Pool DB + migrations + repo prêt pour un ingest CEDH/CJUE (sans embeddings :
/// backfill séparé, comme l'ingest DILA).
pub(super) async fn europe_pool() -> Result<deadpool_postgres::Pool> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    Ok(pool)
}

/// Lists the HUDOC judgments (`HFJUD`, `HEJUD`) and decisions (`HFDEC`, `HEDEC`)
/// of a year, French and English versions alike.
async fn cedh_rulings(client: &lj_sources::cedh::CedhClient, year: i32) -> Result<Vec<CedhResult>> {
    use lj_sources::cedh::{parse_results, PAGE_SIZE};
    let is_ruling =
        |r: &CedhResult| matches!(r.doctype.as_str(), "HFJUD" | "HEJUD" | "HFDEC" | "HEDEC");
    let first = client
        .results_page(year, 0)
        .await
        .map_err(|e| anyhow!("cedh results {year} start=0: {e}"))?;
    let (resultcount, first_rows) = parse_results(&first);
    let mut rows: Vec<CedhResult> = first_rows.into_iter().filter(is_ruling).collect();
    // HUDOC caps the results of a query.
    let year_end = (resultcount as usize).min(10_000);
    let pages: Vec<Vec<CedhResult>> = futures::stream::iter(
        (PAGE_SIZE..year_end)
            .step_by(PAGE_SIZE)
            .map(|start| async move {
                let page = client
                    .results_page(year, start)
                    .await
                    .map_err(|e| anyhow!("cedh results {year} start={start}: {e}"))?;
                Ok::<_, anyhow::Error>(parse_results(&page).1)
            }),
    )
    .buffered(LIST_CONCURRENCY)
    .try_collect()
    .await?;
    for page_rows in pages {
        rows.extend(page_rows.into_iter().filter(is_ruling));
    }
    Ok(rows)
}

/// Which of the rows HUDOC last served empty a sweep asks again. An empty body is
/// a ruling whose DOCX HUDOC has not converted yet, or the other-language entry
/// of a ruling published in one language only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CedhStubs {
    /// The daily sweep: a stub only while its ruling is recent.
    Fresh,
    /// The weekly catch-up: the stubs last asked four weeks ago or more, HUDOC
    /// publishing a translation months after the ruling.
    Due,
}

const STUB_FRESH_DAYS: i64 = 30;
const STUB_REVISIT_DAYS: i64 = 28;

impl CedhStubs {
    /// Whether the sweep asks HUDOC for a row not held, `asked` being the day it
    /// last served it empty.
    fn asks(
        self,
        asked: Option<chrono::NaiveDate>,
        ruling: Option<chrono::NaiveDate>,
        today: chrono::NaiveDate,
    ) -> bool {
        match (asked, self) {
            (None, CedhStubs::Fresh) => true,
            (None, CedhStubs::Due) => false,
            (Some(_), CedhStubs::Fresh) => {
                ruling.is_some_and(|d| (today - d).num_days() < STUB_FRESH_DAYS)
            }
            (Some(asked), CedhStubs::Due) => (today - asked).num_days() >= STUB_REVISIT_DAYS,
        }
    }
}

/// Ingests the CEDH judgments and decisions of a range of years, every language
/// version HUDOC serves. A version already held is skipped; an empty body is kept
/// in the manifest with the day it was asked, and asked again as `stubs` says.
async fn run_cedh(start_year: i32, end_year: i32, stubs: CedhStubs) -> Result<()> {
    use lj_sources::cedh::{CedhClient, CedhManifest};

    let settings = Settings::from_env()?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;

    let client = CedhClient::new(&settings.egress_proxy)?.with_body_cache(settings.cache_dir());
    let manifest_path = lj_sources::cedh::manifest_path(&settings.cache_dir());
    let mut manifest = CedhManifest::load(&manifest_path).map_err(|e| anyhow!("manifest: {e}"))?;

    let today = chrono::Utc::now().date_naive();
    let mut total = IngestCounts::default();
    let (mut gained, mut left) = (0usize, 0usize);
    for year in start_year..=end_year {
        let mut rows = cedh_rulings(&client, year).await?;
        let uids: Vec<String> = rows.iter().map(|r| format!("cedh/{}", r.itemid)).collect();
        let held = present_source_uids(&conn, &uids).await?;
        rows.retain(|r| !held.contains(&format!("cedh/{}", r.itemid)));
        let listed = rows.len();
        rows.retain(|r| stubs.asks(manifest.stubs.get(&r.itemid).copied(), ruling_day(r), today));
        left += listed - rows.len();
        // French first: a ruling's first version creates the decision.
        rows.sort_by_key(|r| u8::from(r.language != "FRE"));

        let mut counts = IngestCounts::default();
        let mut candidates: Vec<Candidate> = Vec::new();
        for row in &rows {
            let body = client
                .body_text(&row.itemid)
                .await
                .map_err(|e| anyhow!("cedh body {}: {e}", row.itemid))?;
            if body.trim().is_empty() {
                manifest.stubs.insert(row.itemid.clone(), today);
                counts.empty_skipped += 1;
                continue;
            }
            gained += usize::from(manifest.stubs.remove(&row.itemid).is_some());
            let decision = parse_cedh(&body, &row.columns, &row.itemid)
                .map_err(|e| anyhow!("parse_cedh {}: {e}", row.itemid))?;
            candidates.push(html_candidate(
                decision,
                &row.columns,
                &body,
                hudoc_lang(&row.language)?,
            ));
            if candidates.len() >= BATCH_SIZE {
                let batch = std::mem::take(&mut candidates);
                drain_batch(&conn, batch, IngestMode::MissingHash, &mut counts).await?;
            }
        }
        if !candidates.is_empty() {
            drain_batch(&conn, candidates, IngestMode::MissingHash, &mut counts).await?;
        }

        manifest.mark_year(year);
        manifest
            .save(&manifest_path)
            .map_err(|e| anyhow!("manifest save: {e}"))?;
        tracing::info!(
            year,
            created = counts.created,
            updated = counts.updated,
            skipped = counts.skipped,
            empty = counts.empty_skipped,
            "ingest_cedh_year"
        );
        total.merge(&counts);
    }

    tracing::info!(
        start_year,
        end_year,
        created = total.created,
        updated = total.updated,
        skipped = total.skipped,
        empty = total.empty_skipped,
        stubs_left = left,
        "ingest_cedh_total"
    );
    if stubs == CedhStubs::Due && gained > 0 {
        tracing::warn!(
            gained,
            still_empty = total.empty_skipped,
            "cedh stubs: HUDOC now serves a body the daily sweep no longer asks for"
        );
    }
    Ok(())
}

fn ruling_day(row: &CedhResult) -> Option<chrono::NaiveDate> {
    let kpdate = row.columns["kpdate"].as_str()?;
    chrono::NaiveDate::parse_from_str(kpdate.get(..10)?, "%Y-%m-%d").ok()
}

/// The ISO 639-1 languages of procedure of a CJUE judgment, read from its CDM
/// predicates (`…/language/DEU`).
fn cjue_procedure_languages(predicates: &serde_json::Value) -> Vec<&'static str> {
    let codes: Vec<&str> = match &predicates["case-law_uses_procedure_language"] {
        serde_json::Value::String(s) => vec![s.as_str()],
        serde_json::Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect(),
        _ => Vec::new(),
    };
    codes
        .into_iter()
        .filter_map(|c| iso_639_1_of_eu(c.rsplit('/').next().unwrap_or(c)))
        .collect()
}

/// The languages a CJUE decision is read in besides its languages of procedure.
const CJUE_READ_LANGUAGES: [&str; 2] = ["fr", "en"];

/// The languages a CJUE decision is wanted in: French and English when CELLAR publishes
/// them, and its languages of procedure, less those EUR-Lex holds only its summary in.
fn cjue_wanted_languages(
    predicates: &serde_json::Value,
    published: &[&'static str],
) -> Vec<&'static str> {
    let summaries: Vec<&str> = predicates[SUMMARY_LANGUAGES]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .collect();
    let mut wanted: Vec<&'static str> = Vec::new();
    for iso in published
        .iter()
        .copied()
        .filter(|l| CJUE_READ_LANGUAGES.contains(l))
        .chain(cjue_procedure_languages(predicates))
    {
        if !wanted.contains(&iso) && !summaries.contains(&iso) {
            wanted.push(iso);
        }
    }
    wanted
}

/// The ISO 639-1 code of a Publications Office or Curia code (`FRA`, `FR`).
fn iso_of_code(code: &str) -> Option<&'static str> {
    iso_639_1_of_eu(code)
        .or_else(|| eu_of_iso_639_1(&code.to_ascii_lowercase()).and_then(iso_639_1_of_eu))
}

/// The CDM predicates a provenance stores, without what the ingest adds per
/// language.
fn stored_predicates(source_fields: &serde_json::Value) -> serde_json::Value {
    let mut p = source_fields.clone();
    if let Some(map) = p.as_object_mut() {
        for key in [
            "sections",
            "lang",
            "resource_obtained_language",
            "objet",
            "text_source",
            "curia_docid",
        ] {
            map.remove(key);
        }
    }
    p
}

/// Ingests the CJUE judgments, orders and opinions of a range of years in every
/// language CELLAR or Curia publishes them in, one provenance `cjue/{CELEX}/{lang}`
/// per language.
///
/// Per year, the SPARQL list deduplicated by CELEX (`sparql_page` →
/// `parse_sparql`, under the OFFSET cap), and the languages of each CELEX
/// (`languages_of_year`). A CELEX whose wanted languages are all held costs no
/// request. The predicates come from a provenance already held, else from
/// `fetch_work_predicates`, which names the language of procedure. A language
/// EUR-Lex does not serve is asked again at the next run. With `short`, a text
/// held under [`SHORT_TEXT_BYTES`] is read again.
async fn run_cjue(start_year: i32, end_year: i32, short: bool) -> Result<()> {
    use lj_sources::cjue::{
        extract_objet, parse_sparql, CjueClient, CjueManifest, PAGE_SIZE, THROTTLE,
    };

    let settings = Settings::from_env()?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let client = CjueClient::new();
    let curia = CuriaClient::new().await?;
    let manifest_path = lj_sources::cjue::manifest_path(&settings.cache_dir());
    let mut manifest = CjueManifest::load(&manifest_path).map_err(|e| anyhow!("manifest: {e}"))?;

    let mut total = IngestCounts::default();
    let mut unserved = 0usize;
    let mut from_curia = 0usize;
    for year in start_year..=end_year {
        let mut offset = 0usize;
        let mut counts = IngestCounts::default();
        let published: HashMap<String, Vec<&'static str>> = client
            .languages_of_year(year)
            .await
            .map_err(|e| anyhow!("cjue languages {year}: {e}"))?
            .into_iter()
            .map(|(celex, codes)| (celex, codes.iter().filter_map(|c| iso_of_code(c)).collect()))
            .collect();
        loop {
            let page = client
                .sparql_page(year, offset)
                .await
                .map_err(|e| anyhow!("cjue sparql {year} offset={offset}: {e}"))?;
            let rows = parse_sparql(&page);
            if rows.is_empty() {
                break;
            }
            let celexes: Vec<String> = rows.iter().map(|r| r.celex.clone()).collect();
            let mut held = repo
                .cjue_languages(&celexes)
                .await
                .map_err(|e| anyhow!("cjue_languages: {e}"))?;
            let opinions: Vec<String> = celexes
                .iter()
                .filter(|c| celex_is_opinion(c))
                .cloned()
                .collect();
            for (celex, (langs, fields)) in repo.conclusions_held(&opinions).await? {
                held.entry(celex)
                    .or_insert((langs.into_iter().map(|l| (l, i32::MAX)).collect(), fields));
            }

            let mut candidates: Vec<Candidate> = Vec::new();
            let mut paged: Vec<String> = Vec::new();
            for row in &rows {
                let published = published.get(&row.celex).map_or(&[][..], Vec::as_slice);
                let (langs, stored) = match held.get(&row.celex) {
                    Some((langs, fields)) => (langs.as_slice(), Some(stored_predicates(fields))),
                    None => (&[][..], None),
                };
                let present: Vec<&str> = langs
                    .iter()
                    .filter(|(_, bytes)| !short || *bytes as usize >= SHORT_TEXT_BYTES)
                    .map(|(lang, _)| lang.as_str())
                    .collect();
                let longest_held = langs.iter().map(|(_, b)| *b as usize).max().unwrap_or(0);
                if let Some(p) = &stored {
                    if cjue_wanted_languages(p, published)
                        .iter()
                        .all(|l| present.contains(l))
                    {
                        continue;
                    }
                }
                let mut predicates = match stored {
                    Some(p) => p,
                    None => {
                        tokio::time::sleep(THROTTLE).await;
                        match client.fetch_work_predicates(&row.celex).await {
                            Ok(p) => p,
                            Err(e) => {
                                tracing::warn!(
                                    celex = %row.celex,
                                    error = %e,
                                    "cjue CDM predicates unavailable, minimum ecli+date fallback"
                                );
                                serde_json::json!({})
                            }
                        }
                    }
                };
                // The SPARQL list carries the ECLI verbatim (never derived from the
                // CELEX) and the date, which the DESCRIBE may omit or repeat.
                let map = predicates.as_object_mut().expect("map_predicates → objet");
                map.insert("case-law_ecli".into(), serde_json::json!(row.ecli));
                map.insert("work_date_document".into(), serde_json::json!(row.date));
                if let Some(title) = &row.title {
                    map.insert(TITLE_FIELD.into(), serde_json::json!(title));
                }
                if let Some(original) = cjue_procedure_languages(&predicates).first() {
                    predicates["original_lang"] =
                        serde_json::json!(eu_of_iso_639_1(original).expect("official language"));
                }
                let wanted = cjue_wanted_languages(&predicates, published);
                let mut asked: Asked = Vec::new();
                for &iso in &wanted {
                    if present.contains(&iso) {
                        continue;
                    }
                    let eu = eu_of_iso_639_1(iso).expect("wanted languages are official");
                    tokio::time::sleep(THROTTLE).await;
                    // EUR-Lex answers some renditions 500 for good (62019CJ0937 eng).
                    let body = match client.resource_text_in(&row.celex, eu).await {
                        Ok(body) => body,
                        Err(error) => {
                            tracing::warn!(celex = %row.celex, lang = eu, %error, "cjue resource failed, language skipped");
                            None
                        }
                    };
                    asked.push((iso, body.filter(|b| !b.trim().is_empty()), None));
                }
                let lacking = asked
                    .iter()
                    .any(|(_, body, _)| body.as_ref().is_none_or(|b| b.len() < SHORT_TEXT_BYTES));
                if lacking || (wanted.is_empty() && present.is_empty()) {
                    curia_texts(
                        &curia,
                        &row.celex,
                        row.ecli.as_deref(),
                        &wanted,
                        &present,
                        &mut asked,
                    )
                    .await;
                }
                let summary_languages = drop_eurlex_summaries(&mut asked, longest_held);
                if !summary_languages.is_empty() {
                    predicates[SUMMARY_LANGUAGES] = serde_json::json!(summary_languages);
                }
                let mut fetched: Vec<(&'static str, String, Option<u64>)> = Vec::new();
                for (iso, body, docid) in asked {
                    match body {
                        Some(body) => fetched.push((iso, body, docid)),
                        None => {
                            unserved += 1;
                            counts.empty_skipped += 1;
                        }
                    }
                }
                if celex_is_opinion(&row.celex) && !fetched.is_empty() {
                    write_opinion(&repo, &row.celex, &predicates, &fetched).await?;
                    paged.push(row.celex.clone());
                }
                for (iso, body, docid) in fetched {
                    let eu = eu_of_iso_639_1(iso).expect("wanted languages are official");
                    let mut fields = predicates.clone();
                    fields["resource_obtained_language"] = serde_json::json!(eu);
                    if let Some(docid) = docid {
                        fields["text_source"] = serde_json::json!("curia");
                        fields["curia_docid"] = serde_json::json!(docid);
                        from_curia += 1;
                    }
                    if iso == "fr" {
                        if let Some(objet) = extract_objet(&body) {
                            fields["objet"] = serde_json::json!(objet);
                        }
                    }
                    let decision = parse_cjue(&body, &fields, &row.celex, iso)
                        .map_err(|e| anyhow!("parse_cjue {}: {e}", row.celex))?;
                    candidates.push(html_candidate(decision, &fields, &body, eu));
                }
                if candidates.len() >= BATCH_SIZE {
                    let batch = std::mem::take(&mut candidates);
                    drain_batch(&conn, batch, IngestMode::MissingHash, &mut counts).await?;
                }
            }
            if !candidates.is_empty() {
                drain_batch(&conn, candidates, IngestMode::MissingHash, &mut counts).await?;
            }
            repo.pin_conclusions_pages(&std::mem::take(&mut paged))
                .await?;

            offset += PAGE_SIZE;
            // EUR-Lex caps OFFSET: past it, the next page is empty.
            if offset >= 10_000 {
                break;
            }
        }

        manifest.mark_year(year);
        manifest
            .save(&manifest_path)
            .map_err(|e| anyhow!("manifest save: {e}"))?;
        tracing::info!(
            year,
            created = counts.created,
            updated = counts.updated,
            skipped = counts.skipped,
            empty = counts.empty_skipped,
            "ingest_cjue_year"
        );
        total.merge(&counts);
    }

    tracing::info!(
        start_year,
        end_year,
        created = total.created,
        updated = total.updated,
        skipped = total.skipped,
        unserved,
        from_curia,
        "ingest_cjue_total"
    );
    Ok(())
}

/// Writes an Advocate General's opinion to `conclusions`, beside the page its texts make.
async fn write_opinion(
    repo: &DecisionRepository<'_>,
    celex: &str,
    predicates: &serde_json::Value,
    fetched: &[(&'static str, String, Option<u64>)],
) -> Result<()> {
    let Some((first_lang, first_body, _)) = fetched.first() else {
        return Ok(());
    };
    let parsed = parse_cjue(first_body, predicates, celex, first_lang)
        .map_err(|e| anyhow!("parse_cjue {celex}: {e}"))?;
    let langs: Vec<String> = fetched.iter().map(|(l, _, _)| l.to_string()).collect();
    let author = predicates[TITLE_FIELD]
        .as_str()
        .and_then(super::conclusions::advocate_general);
    repo.upsert_conclusions(&lj_store::repository::ConclusionsWrite {
        source: super::conclusions::EUR_LEX,
        source_uid: celex,
        jurisdiction_code: "cjue",
        case_numbers: &parsed.numero_dossiers.unwrap_or_default(),
        ruled_on: None,
        delivered_on: super::conclusions::iso_date(parsed.date_lecture.as_deref()),
        author: author.as_deref(),
        langs: &langs,
        source_fields: predicates,
        public_id: None,
        summary: None,
        summary_engine: None,
    })
    .await?;
    Ok(())
}

/// The texts asked for a CELEX: language, text if served, Curia docid if Curia served it.
type Asked = Vec<(&'static str, Option<String>, Option<u64>)>;

/// Drops from `asked` the EUR-Lex texts that are the summary of their work, under
/// [`SHORT_TEXT_BYTES`] and under half its longest text, the base's `longest_held`
/// included, and returns their languages.
fn drop_eurlex_summaries(asked: &mut Asked, longest_held: usize) -> Vec<&'static str> {
    let longest = asked
        .iter()
        .filter_map(|(_, body, _)| body.as_ref().map(String::len))
        .chain([longest_held])
        .max()
        .unwrap_or(0);
    let mut summaries = Vec::new();
    asked.retain(|(iso, body, docid)| {
        let summary = docid.is_none()
            && body
                .as_ref()
                .is_some_and(|b| b.len() < SHORT_TEXT_BYTES && 2 * b.len() < longest);
        if summary {
            summaries.push(*iso);
        }
        !summary
    });
    summaries
}

/// Asks Curia for every wanted language of `celex`, French and English included, it serves
/// that the base lacks or EUR-Lex gave short: its text, with its docid, wins over none or
/// over one under half its length.
async fn curia_texts(
    curia: &CuriaClient,
    celex: &str,
    ecli: Option<&str>,
    wanted: &[&str],
    present: &[&str],
    asked: &mut Asked,
) {
    let document = match curia.find(celex, ecli).await {
        Ok(Some(document)) => document,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(%celex, %error, "curia listing failed, EUR-Lex text kept");
            return;
        }
    };
    let mut served: Vec<&'static str> = Vec::new();
    for iso in document
        .html
        .iter()
        .chain(&document.pdf)
        .filter_map(|c| iso_of_code(c))
        .filter(|iso| wanted.contains(iso) || CJUE_READ_LANGUAGES.contains(iso))
    {
        if !served.contains(&iso) {
            served.push(iso);
        }
    }
    for iso in served {
        let held = asked.iter().position(|(l, _, _)| *l == iso);
        let short = held.is_none_or(|i| {
            asked[i]
                .1
                .as_ref()
                .is_none_or(|b| b.len() < SHORT_TEXT_BYTES)
        });
        if present.contains(&iso) || !short {
            continue;
        }
        let text = match curia.text(&document, iso).await {
            Ok(Some(text)) => text,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(%celex, lang = iso, %error, "curia text failed, EUR-Lex text kept");
                continue;
            }
        };
        match held {
            Some(i) => {
                if asked[i].1.as_ref().is_none_or(|b| 2 * b.len() < text.len()) {
                    asked[i] = (iso, Some(text), Some(document.docid));
                }
            }
            None => asked.push((iso, Some(text), Some(document.docid))),
        }
    }
}

/// Bootstrap CEDH : balaye les années [[`from_year`] (défaut [`CEDH_START_YEAR`]),
/// courante]. `from_year` cible un backfill sans re-lister le fonds ancien.
pub async fn ingest_cedh(from_year: Option<i32>) -> Result<()> {
    run_cedh(
        from_year.unwrap_or(CEDH_START_YEAR),
        current_year(),
        CedhStubs::Fresh,
    )
    .await
}

/// Warms the disk cache of the CEDH bodies (`<cache_dir>/cedh/bodies/`) for
/// the years [[`from_year`] (default [`CEDH_START_YEAR`]), current], without
/// touching the base: `ingest-cedh` then reads them locally.
pub async fn cache_cedh(from_year: Option<i32>) -> Result<()> {
    use lj_sources::cedh::CedhClient;

    const CACHE_CONCURRENCY: usize = 8;

    let settings = Settings::from_env()?;
    let client = CedhClient::new(&settings.egress_proxy)?.with_body_cache(settings.cache_dir());
    let start_year = from_year.unwrap_or(CEDH_START_YEAR);
    let end_year = current_year();

    let mut total_fetched = 0usize;
    let mut total_empty = 0usize;
    for year in start_year..=end_year {
        let rows = cedh_rulings(&client, year).await?;
        let served: Vec<bool> = futures::stream::iter(rows.iter().map(|r| {
            let client = &client;
            async move {
                let body = client
                    .body_text(&r.itemid)
                    .await
                    .map_err(|e| anyhow!("cedh body {}: {e}", r.itemid))?;
                Ok::<_, anyhow::Error>(!body.trim().is_empty())
            }
        }))
        .buffer_unordered(CACHE_CONCURRENCY)
        .try_collect()
        .await?;
        let year_fetched = served.iter().filter(|s| **s).count();
        let year_empty = served.len() - year_fetched;
        tracing::info!(
            year,
            fetched = year_fetched,
            empty = year_empty,
            "cache_cedh_year"
        );
        total_fetched += year_fetched;
        total_empty += year_empty;
    }
    tracing::info!(
        start_year,
        end_year,
        total_fetched,
        total_empty,
        "cache_cedh_total"
    );
    Ok(())
}

/// CJUE bootstrap over every year from [`CJUE_START_YEAR`]; with `short`, a text held
/// under [`SHORT_TEXT_BYTES`] is read again.
pub async fn ingest_cjue(short: bool) -> Result<()> {
    run_cjue(CJUE_START_YEAR, current_year(), short).await
}

/// Weekly catch-up of the daily CEDH sync: the rows of `[N-1, N]` HUDOC served
/// empty, asked again once their last visit is old enough.
pub async fn sync_cedh_stubs() -> Result<()> {
    let year = current_year();
    run_cedh(year - 1, year, CedhStubs::Due).await
}

/// Daily CEDH sync over the sliding window `[N-1, N]`: new judgments, the
/// language versions HUDOC has published since the last run, and the legal summaries.
pub async fn sync_cedh() -> Result<()> {
    write_europe_names().await?;
    let year = current_year();
    run_cedh(year - 1, year, CedhStubs::Fresh).await?;
    super::cedh_application::ingest_cedh_applications().await?;
    super::cedh_link::settle_cedh_steps(lj_store::repository::CedhVisit::Unread).await?;
    super::court_summaries::sync_cedh_summaries(year - 1).await
}

/// Daily CJUE sync over the sliding window `[N-1, N]`: new judgments, the
/// languages EUR-Lex has published since the last run, and the court's summaries.
pub async fn sync_cjue() -> Result<()> {
    write_europe_names().await?;
    let year = current_year();
    run_cjue(year - 1, year, false).await?;
    super::court_summaries::sync_cjue_summaries(year - 1).await
}

/// Writes the European courts' names in the other languages on their rows.
pub(super) async fn write_europe_names() -> Result<()> {
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let named = DecisionRepository::new(&conn)
        .write_jurisdiction_names(&europe_names())
        .await?;
    tracing::info!(named, "europe sync: court names written");
    Ok(())
}

/// Page of the joined-cases catch-up, under the CELLAR cap on sorted rows. A
/// CELEX year weighs tens of links, so one page answers it whole and the loop is
/// only there to guard the outlier.
const JOINED_CASES_PAGE: usize = 2_000;

/// First CELEX year swept for joined cases. Sector 6 opens in 1954, and the
/// corpus reaches back to 1958 even though the ingest sweeps from 1960: the year
/// of the CELEX is that of the case, which the judgment may postdate by years.
const JOINED_CASES_START_YEAR: i32 = 1954;

/// Poses the joined cases over the CJUE corpus that predates the predicate:
/// sweeps `case-law_joins_case_court` for the whole of CELLAR and writes it into
/// the `source_fields` of the provenances we hold, in the shape the ingest gives
/// it. Writes no docket number: `reextract-fields --jurisdiction-type CJUE
/// --field docket_numbers` reads them back through `parse_cjue`, and its
/// inverted probe then attracts the citations left pending.
pub async fn backfill_cjue_joined_cases(dry_run: bool) -> Result<()> {
    use lj_sources::cjue::{parse_joined_cases, predicate_value, CjueClient, THROTTLE};

    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = lj_store::repository::DecisionRepository::new(&conn);
    let client = CjueClient::new();

    let mut works = 0usize;
    let mut links = 0usize;
    let mut written = 0u64;
    for year in JOINED_CASES_START_YEAR..=current_year() {
        let mut offset = 0usize;
        loop {
            let page = client
                .joined_cases_page(year, offset, JOINED_CASES_PAGE)
                .await
                .map_err(|e| anyhow!("cjue affaires jointes {year} offset={offset}: {e}"))?;
            let rows = parse_joined_cases(&page);
            if rows.is_empty() {
                break;
            }
            works += rows.len();
            links += rows.iter().map(|(_, j)| j.len()).sum::<usize>();

            let (uids, values): (Vec<String>, Vec<String>) = rows
                .into_iter()
                .flat_map(|(celex, joints)| {
                    let value = predicate_value(joints).to_string();
                    lj_vocab::languages::EU_OFFICIAL_LANGUAGES
                        .iter()
                        .map(move |l| (format!("cjue/{celex}/{l}"), value.clone()))
                })
                .unzip();
            if !dry_run {
                written += repo
                    .set_source_field("case-law_joins_case_court", &uids, &values)
                    .await?;
            }

            offset += JOINED_CASES_PAGE;
            tokio::time::sleep(THROTTLE).await;
        }
    }

    tracing::info!(works, links, written, dry_run, "backfill_cjue_joined_cases");
    Ok(())
}

/// Where the CJUE provenances keep the CELLAR French title, which names the parties.
const TITLE_FIELD: &str = "expression_title";

/// Poses the CELLAR French title over the CJUE corpus ingested before the listing
/// carried it. Writes no name: chain `reextract-fields --jurisdiction-type CJUE
/// --field case_name`, then `--field search_title`.
pub async fn backfill_cjue_titles(dry_run: bool) -> Result<()> {
    use lj_sources::cjue::{parse_sparql, CjueClient, PAGE_SIZE, THROTTLE};

    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = lj_store::repository::DecisionRepository::new(&conn);
    let client = CjueClient::new();

    let mut works = 0usize;
    let mut titled = 0usize;
    let mut written = 0u64;
    for year in CJUE_START_YEAR..=current_year() {
        let mut offset = 0usize;
        while offset < 10_000 {
            let page = client
                .sparql_page(year, offset)
                .await
                .map_err(|e| anyhow!("cjue titles {year} offset={offset}: {e}"))?;
            let rows = parse_sparql(&page);
            if rows.is_empty() {
                break;
            }
            works += rows.len();
            let (uids, values): (Vec<String>, Vec<String>) = rows
                .into_iter()
                .filter_map(|row| Some((row.celex, row.title?)))
                .inspect(|_| titled += 1)
                .flat_map(|(celex, title)| {
                    let value = serde_json::json!(title).to_string();
                    lj_vocab::languages::EU_OFFICIAL_LANGUAGES
                        .iter()
                        .map(move |l| (format!("cjue/{celex}/{l}"), value.clone()))
                })
                .unzip();
            if !dry_run {
                written += repo.set_source_field(TITLE_FIELD, &uids, &values).await?;
            }
            offset += PAGE_SIZE;
            tokio::time::sleep(THROTTLE).await;
        }
    }

    tracing::info!(works, titled, written, dry_run, "backfill_cjue_titles");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stub_is_asked_daily_while_its_ruling_is_recent_then_every_four_weeks() {
        let day = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        let today = day("2026-11-16");
        let (recent, old) = (Some(day("2026-11-01")), Some(day("2025-06-12")));
        assert!(CedhStubs::Fresh.asks(None, old, today));
        assert!(CedhStubs::Fresh.asks(Some(day("2026-11-15")), recent, today));
        assert!(!CedhStubs::Fresh.asks(Some(day("2026-11-15")), old, today));
        assert!(!CedhStubs::Due.asks(None, old, today));
        assert!(!CedhStubs::Due.asks(Some(day("2026-11-01")), old, today));
        assert!(CedhStubs::Due.asks(Some(day("2026-10-19")), old, today));
    }

    #[test]
    fn a_cjue_decision_is_wanted_in_french_english_and_its_languages_of_procedure() {
        let one = serde_json::json!({
            "case-law_uses_procedure_language": "http://publications.europa.eu/resource/authority/language/DEU"
        });
        assert_eq!(cjue_wanted_languages(&one, &["fr", "da"]), ["fr", "de"]);
        assert_eq!(
            cjue_wanted_languages(&one, &["da", "en", "de", "fr"]),
            ["en", "fr", "de"]
        );
        let joined = serde_json::json!({"case-law_uses_procedure_language": ["ITA", "NLD"]});
        assert_eq!(cjue_procedure_languages(&joined), ["it", "nl"]);
        assert_eq!(cjue_wanted_languages(&joined, &["it"]), ["it", "nl"]);
        assert!(cjue_wanted_languages(&serde_json::json!({}), &[]).is_empty());
        let summarised = serde_json::json!({
            "case-law_uses_procedure_language": "ENG",
            SUMMARY_LANGUAGES: ["da", "it"],
        });
        assert_eq!(
            cjue_wanted_languages(&summarised, &["fr", "da", "it"]),
            ["fr", "en"]
        );
    }

    #[test]
    fn a_short_eurlex_text_beside_a_long_one_is_its_summary_and_a_short_order_is_not() {
        let text = |n: usize| Some("x".repeat(n));
        let mut asked: Asked = vec![
            ("fr", text(40_000), Some(1)),
            ("en", text(4_000), None),
            ("de", text(4_200), Some(1)),
            ("it", None, None),
        ];
        assert_eq!(drop_eurlex_summaries(&mut asked, 0), ["en"]);
        assert_eq!(
            asked.iter().map(|a| a.0).collect::<Vec<_>>(),
            ["fr", "de", "it"]
        );

        let mut order: Asked = vec![("fr", text(3_000), None), ("en", text(2_800), None)];
        assert!(drop_eurlex_summaries(&mut order, 3_100).is_empty());
        assert_eq!(order.len(), 2);

        let mut beside_held: Asked = vec![("da", text(3_000), None)];
        assert_eq!(drop_eurlex_summaries(&mut beside_held, 65_000), ["da"]);
    }
}
