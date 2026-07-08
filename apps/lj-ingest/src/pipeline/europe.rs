//! Sources européennes : CEDH (HUDOC) et CJUE (EUR-Lex). Corpus bilingue
//! FR-prioritaire (ADR 0120, supersede le FR-only de 0094) : on retient la version
//! FR d'une affaire si elle existe, sinon EN.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};

use lj_core::decision::Decision;
use lj_core::parsing::{build_source_fields, parse_cedh, parse_cjue};
use lj_sources::cedh::CedhResult;
use lj_store::db::Connection;
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

use super::batch::drain_batch;
use super::embed::build_embedder_opt;
use super::{generate_public_id, Candidate, IngestCounts, IngestMode, WriteMode, BATCH_SIZE};

/// Clé d'affaire HUDOC regroupant les versions linguistiques d'un même arrêt :
/// ECLI si présent, sinon numéro(s) de requête (`appno`), sinon `itemid` (isolé).
/// HUDOC publie un document par langue ; deux versions d'une affaire partagent
/// ECLI et `appno`.
fn cedh_case_key(row: &CedhResult) -> String {
    let ecli = row.columns["ecli"].as_str().unwrap_or("").trim();
    if !ecli.is_empty() {
        return format!("ecli:{ecli}");
    }
    let appno = row.columns["appno"].as_str().unwrap_or("").trim();
    if !appno.is_empty() {
        return format!("appno:{appno}");
    }
    format!("itemid:{}", row.itemid)
}

/// Regroupe les enregistrements HUDOC par affaire (`cedh_case_key`), chaque groupe
/// ordonné **FR d'abord** (repli EN, ADR 0120), en préservant l'ordre de première
/// apparition des affaires. Un arrêt a un enregistrement **par langue avec un
/// doctype distinct** (`HFJUD` = arrêt FR, `HEJUD` = judgment EN) : on garde les
/// deux versions pour **cascader au fetch** — le FR est souvent un stub 204 quand
/// la langue officielle de l'affaire est l'anglais (cf. note 2026-07-03), le texte
/// réel vivant alors dans le `HEJUD`.
fn group_cases(rows: Vec<CedhResult>) -> Vec<Vec<CedhResult>> {
    let mut idx: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Vec<CedhResult>> = Vec::new();
    for row in rows {
        let key = cedh_case_key(&row);
        let i = *idx.entry(key).or_insert_with(|| {
            out.push(Vec::new());
            out.len() - 1
        });
        out[i].push(row);
    }
    // FRE (false → 0) avant les autres langues : tri stable, ordre interne conservé.
    for group in &mut out {
        group.sort_by_key(|r| u8::from(r.language != "FRE"));
    }
    out
}

/// Cascade FR→EN **au fetch** pour une affaire (patron [`lj_sources::cjue::CjueClient::resource_text`]) :
/// tente le corps de chaque version linguistique (FR d'abord, cf. [`group_cases`])
/// et renvoie la **première** dont le corps est non vide, avec son texte strippé.
/// `Ok(None)` si aucune langue n'a de corps (204 partout → SKIP différé, décision #5).
/// C'est ce qui remonte la couverture de ~17 % à ~99 % : sur les affaires dont le FR
/// est un stub 204, on bascule sur le `HEJUD` anglais qui, lui, porte le texte.
async fn cedh_case_body<'a>(
    client: &lj_sources::cedh::CedhClient,
    case: &'a [CedhResult],
) -> Result<Option<(&'a CedhResult, String)>> {
    for row in case {
        tokio::time::sleep(lj_sources::cedh::THROTTLE).await;
        let body = client
            .body_text(&row.itemid)
            .await
            .map_err(|e| anyhow!("cedh body {}: {e}", row.itemid))?;
        if !body.trim().is_empty() {
            return Ok(Some((row, body)));
        }
    }
    Ok(None)
}

/// Sous-ensemble des `source_uid` **déjà présents** en base (provenances actives).
/// Le re-bootstrap bilingue s'en sert pour ne pas re-fetcher les corps déjà ingérés
/// (courtoisie HUDOC/EUR-Lex : on ne tire que la traîne neuve EN-only). Le sync
/// incrémental ne l'utilise pas — il doit re-fetcher l'année courante.
/// Stratégie de filtrage des affaires déjà en base, partagée par [`run_cedh`] et
/// [`run_cjue`] (mêmes subtilités bilingues FR-prioritaires, ADR 0120).
#[derive(Clone, Copy)]
enum EuropeSkip {
    /// Bootstrap / re-bootstrap : saute toute affaire déjà ingérée (on ne tire
    /// que la traîne neuve, courtoisie de la source).
    AllPresent,
    /// Sync incrémental : saute les affaires déjà servies en **français** ;
    /// re-fetch les nouvelles et les EN-only (upgrade FR différé, ADR 0120). Un
    /// arrêt déjà en FR est définitif → pas de re-téléchargement nocturne inutile.
    FrPresentOnly,
}

/// Sous-ensemble des `source_uid` présents ET déjà servis en FR — le sync CJUE
/// saute ceux-là (cf. [`DecisionRepository::find_fr_source_uids`]).
async fn present_fr_source_uids(conn: &Connection, uids: &[String]) -> Result<HashSet<String>> {
    DecisionRepository::new(conn)
        .find_fr_source_uids(uids)
        .await
        .map_err(|e| anyhow!("find_fr_source_uids: {e}"))
}

async fn present_source_uids(conn: &Connection, uids: &[String]) -> Result<HashSet<String>> {
    let repo = DecisionRepository::new(conn);
    let states = repo
        .find_ingest_states(uids)
        .await
        .map_err(|e| anyhow!("find_ingest_states: {e}"))?;
    Ok(states.into_keys().collect())
}

/// Première année balayée au bootstrap CEDH/CJUE. La CEDH rend des arrêts depuis
/// 1960 (première affaire 1961) ; EUR-Lex secteur 6 depuis 1954. On part de 1960
/// — un fond plus ancien rendrait des listes vides (drain immédiat, sans effet).
const EUROPE_START_YEAR: i32 = 1960;

/// Année courante (UTC), borne haute des balayages CEDH/CJUE.
fn current_year() -> i32 {
    chrono::Utc::now()
        .date_naive()
        .format("%Y")
        .to_string()
        .parse()
        .unwrap_or(EUROPE_START_YEAR)
}

/// Construit le `source_fields` HTML (ADR 0094) : les métadonnées verbatim
/// (colonnes HUDOC / prédicats CDM, qui ne portent pas de texte) plus les
/// sections rebasées sur `full_text` ([`build_source_fields`] retire `text`/
/// `zones` absents et n'ajoute que `sections`).
fn html_source_fields(metadata: &serde_json::Value, decision: &Decision) -> serde_json::Value {
    build_source_fields(metadata, &decision.sections)
}

/// Octets bruts hashés pour l'idempotence d'un document HTML CEDH/CJUE
/// (grounding #7) : métadonnées sérialisées (colonnes/prédicats) suivies du corps
/// brut. Un changement de l'un ou l'autre invalide le checksum.
fn html_checksum(metadata: &serde_json::Value, body_text: &str) -> String {
    let mut payload = serde_json::to_vec(metadata).unwrap_or_default();
    payload.extend_from_slice(body_text.as_bytes());
    super::content_checksum(&payload)
}

/// Normalise un code langue source (HUDOC 'FRE'/'ENG', EUR-Lex 'fra'/'eng') vers
/// ISO-639-2/T ('fra'/'eng'), la valeur portée par la colonne `decision_sources.lang`
/// (ADR 0153). Toute autre valeur → `None` (langue non gérée).
fn iso639_2t(raw: &str) -> Option<&'static str> {
    match raw {
        "FRE" | "fra" => Some("fra"),
        "ENG" | "eng" => Some("eng"),
        _ => None,
    }
}

/// Construit le candidat CEDH/CJUE depuis `(decision parsée, métadonnées, corps,
/// langue de la source)`. La langue est **dite ici par l'ingester** dans
/// `source_fields["lang"]` (ADR 0153) → matérialisée en colonne `lang` par
/// `upsert_decision_source`.
fn html_candidate(
    decision: Decision,
    metadata: &serde_json::Value,
    body_text: &str,
    lang: Option<&str>,
) -> Candidate {
    let mut source_fields = html_source_fields(metadata, &decision);
    if let Some(l) = lang {
        source_fields["lang"] = serde_json::json!(l);
    }
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
        dila_fond: None,
    }
}

/// Pool DB + migrations + repo prêt pour un ingest CEDH/CJUE (sans embeddings :
/// backfill séparé, comme l'ingest DILA).
pub(super) async fn europe_pool() -> Result<deadpool_postgres::Pool> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    Ok(pool)
}

/// Ingère les arrêts CEDH (HUDOC) d'une plage d'années en base (corpus bilingue
/// FR-prioritaire, ADR 0120).
///
/// Pour chaque année (de la plus ancienne à la plus récente), en quatre temps :
/// 1. **Liste seule** (métadonnées, sans corps) : pagine la liste bilingue
///    (`results_page`, `sort` posé, fenêtre 10 000) et ne retient que les **arrêts**
///    FR (`HFJUD`) **et** EN (`HEJUD`) ; notes `CLINF`/press `PR` non ingérées
///    (grounding #4).
/// 2. **Regroupement par affaire** : [`group_cases`] regroupe par ECLI/`appno`, FR
///    d'abord — les deux versions restent pour cascader au fetch (ADR 0120).
/// 3. **Filtrage des présents** (`skip`) : `AllPresent` (bootstrap) retire toute
///    affaire déjà en base ; `FrPresentOnly` (sync) ne retire que celles déjà
///    servies en **français** (`lang='fra'`, ADR 0153) et re-visite les
///    EN-only pour capter l'upgrade FR différé (le stub `HFJUD` passe de 204 à 200
///    quand la traduction paraît → l'autorité bascule le `full_text` en français,
///    ADR 0127/0153). Une affaire est « présente » si l'un de ses itemids l'est.
/// 4. **Corps + ingestion** : fetch converti+strippé (`body_text`). **Corps vide ⇒
///    SKIP** (lag de conversion DOCX→HTML, re-fetch au prochain run). Sinon
///    `parse_cedh` → `drain_batch` (ECLI-first actif, `payload_format = "html"`,
///    `source_uid = cedh/{itemid}`). La langue retenue est tracée dans
///    `source_fields` (`languageisocode`). Idempotent par `content_checksum`,
///    throttle de courtoisie, manifeste par année.
async fn run_cedh(start_year: i32, end_year: i32, skip: EuropeSkip) -> Result<()> {
    use lj_sources::cedh::{parse_results, CedhClient, CedhManifest, PAGE_SIZE};

    let settings = Settings::from_env()?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;

    let (embedder, require_embeddings) = build_embedder_opt(&settings).await?;
    // Cache disque des corps (warmable hors-DB via `cache_cedh`) : relecture locale
    // sans réseau ni throttle — seul l'embedding reste comme coût réel.
    let client = CedhClient::new().with_body_cache(settings.cache_dir());
    let manifest_path = lj_sources::cedh::manifest_path(&settings.cache_dir());
    let mut manifest = CedhManifest::load(&manifest_path).map_err(|e| anyhow!("manifest: {e}"))?;

    let mut total = IngestCounts::default();
    let mut empty_bodies = 0usize;
    let mut en_fallback = 0usize;
    for year in start_year..=end_year {
        // (1) Liste seule : draine toutes les pages de l'année, ne garde que les arrêts.
        let mut rows: Vec<CedhResult> = Vec::new();
        let mut start = 0usize;
        loop {
            let page = client
                .results_page(year, start)
                .await
                .map_err(|e| anyhow!("cedh results {year} start={start}: {e}"))?;
            let (resultcount, page_rows) = parse_results(&page);
            if page_rows.is_empty() {
                break;
            }
            // Arrêts FR (`HFJUD`) **et** EN (`HEJUD`) : les deux versions d'une
            // affaire remontent pour cascader au fetch (cf. group_cases / note 2026-07-03).
            rows.extend(
                page_rows
                    .into_iter()
                    .filter(|r| r.doctype == "HFJUD" || r.doctype == "HEJUD"),
            );
            start += PAGE_SIZE;
            // Fenêtre globale 10 000 par année (cap HUDOC) : au-delà on s'arrête.
            if start as u64 >= resultcount || start >= 10_000 {
                break;
            }
        }

        // (2) Regroupement par affaire (FR d'abord, repli EN au fetch).
        let mut cases = group_cases(rows);

        // (3) Filtrage des présents selon la stratégie. Une affaire (2 itemids
        // FR/EN) est « présente » si l'un de ses itemids l'est. `AllPresent` saute
        // tout ingéré ; `FrPresentOnly` ne saute que le déjà-FR et re-visite les
        // EN-only (upgrade FR différé, ADR 0120/0153).
        if !cases.is_empty() {
            let uids: Vec<String> = cases
                .iter()
                .flatten()
                .map(|r| format!("cedh/{}", r.itemid))
                .collect();
            let skip_uids = match skip {
                EuropeSkip::AllPresent => present_source_uids(&conn, &uids).await?,
                EuropeSkip::FrPresentOnly => present_fr_source_uids(&conn, &uids).await?,
            };
            cases.retain(|case| {
                !case
                    .iter()
                    .any(|r| skip_uids.contains(&format!("cedh/{}", r.itemid)))
            });
        }

        // (4) Cascade FR→EN par affaire + ingestion par batch.
        let mut counts = IngestCounts::default();
        let mut candidates: Vec<Candidate> = Vec::new();
        for case in &cases {
            let Some((row, body)) = cedh_case_body(&client, case).await? else {
                // Aucune langue de l'affaire n'a de corps : re-fetch différé.
                empty_bodies += 1;
                counts.empty_skipped += 1;
                continue;
            };
            if row.language != "FRE" {
                en_fallback += 1;
            }
            let decision = parse_cedh(&body, &row.columns, &row.itemid)
                .map_err(|e| anyhow!("parse_cedh {}: {e}", row.itemid))?;
            candidates.push(html_candidate(
                decision,
                &row.columns,
                &body,
                iso639_2t(&row.language),
            ));
            if candidates.len() >= BATCH_SIZE {
                let batch = std::mem::take(&mut candidates);
                drain_batch(
                    &conn,
                    embedder.as_ref(),
                    batch,
                    require_embeddings,
                    IngestMode::MissingHash,
                    &mut counts,
                )
                .await?;
            }
        }
        if !candidates.is_empty() {
            drain_batch(
                &conn,
                embedder.as_ref(),
                candidates,
                require_embeddings,
                IngestMode::MissingHash,
                &mut counts,
            )
            .await?;
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
        empty_bodies,
        en_fallback,
        "ingest_cedh_total"
    );
    Ok(())
}

/// Ingère les arrêts/ordonnances CJUE (EUR-Lex) d'une plage d'années (corpus
/// bilingue FR-prioritaire, ADR 0120).
///
/// Pour chaque année : pagine la liste SPARQL dédupliquée par CELEX (`sparql_page`
/// → `parse_sparql`, OFFSET cappé 10 000). En `AllPresent` (re-bootstrap), retire
/// d'emblée les CELEX déjà en base ; en `FrPresentOnly` (sync), seuls les déjà-FR
/// (on re-fetch la traîne neuve + les EN-only). Pour chaque
/// CELEX restant : fetch le texte du resource **FR-prioritaire avec repli EN**
/// (`resource_text`, cascade d'`Accept` × langue). **Ni FR ni EN / corps vide ⇒
/// SKIP** (affaire indisponible dans nos deux langues). Sinon `parse_cjue` →
/// `drain_batch` (ECLI-first actif, `payload_format = "html"`,
/// `source_uid = cjue/{CELEX}`) ; la langue obtenue est tracée en `source_fields`
/// (`resource_obtained_language`). Conclusions AG (`OPIN_AG`) regroupées par CELEX,
/// non ingérées comme décisions concurrentes (décision #4). Idempotent par
/// `content_checksum`, throttle de courtoisie, manifeste par année.
///
/// `source_fields` CJUE = les **prédicats CDM riches** par CELEX
/// (`fetch_work_predicates` : subject-matter, juge/AG/formation, `work_cites_work`,
/// `interpretes_resource_legal`, procédure, pays de renvoi… — la mine de richesse
/// de l'audit `cjue.md`), plus la bannière « objet » extraite du corps et l'ECLI +
/// date de la liste. Si le fetch des prédicats échoue (réseau / SPARQL), on logue
/// et on retombe sur le minimum `{case-law_ecli, work_date_document}` (pas de
/// fallback silencieux — l'erreur est tracée).
async fn run_cjue(start_year: i32, end_year: i32, skip: EuropeSkip) -> Result<()> {
    use lj_sources::cjue::{
        extract_objet, parse_sparql, CjueClient, CjueManifest, PAGE_SIZE, THROTTLE,
    };

    let settings = Settings::from_env()?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;

    let (embedder, require_embeddings) = build_embedder_opt(&settings).await?;
    let client = CjueClient::new();
    let manifest_path = lj_sources::cjue::manifest_path(&settings.cache_dir());
    let mut manifest = CjueManifest::load(&manifest_path).map_err(|e| anyhow!("manifest: {e}"))?;

    let mut total = IngestCounts::default();
    let mut no_lang_skipped = 0usize;
    let mut en_fallback = 0usize;
    for year in start_year..=end_year {
        let mut offset = 0usize;
        let mut counts = IngestCounts::default();
        loop {
            let page = client
                .sparql_page(year, offset)
                .await
                .map_err(|e| anyhow!("cjue sparql {year} offset={offset}: {e}"))?;
            let mut rows = parse_sparql(&page);
            if rows.is_empty() {
                break;
            }
            // On saute les CELEX déjà ingérés selon la stratégie (on ne re-fetch que
            // ce qui le mérite, courtoisie EUR-Lex). Le `len` post-filtrage ne sert pas à
            // la pagination — l'OFFSET avance toujours de `PAGE_SIZE` sur la liste brute.
            let uids: Vec<String> = rows.iter().map(|r| format!("cjue/{}", r.celex)).collect();
            let skip_uids = match skip {
                EuropeSkip::AllPresent => present_source_uids(&conn, &uids).await?,
                EuropeSkip::FrPresentOnly => present_fr_source_uids(&conn, &uids).await?,
            };
            rows.retain(|r| !skip_uids.contains(&format!("cjue/{}", r.celex)));

            let mut candidates: Vec<Candidate> = Vec::new();
            for row in &rows {
                tokio::time::sleep(THROTTLE).await;
                let Some((body, lang)) = client
                    .resource_text(&row.celex)
                    .await
                    .map_err(|e| anyhow!("cjue resource {}: {e}", row.celex))?
                else {
                    // Ni FR ni EN servi (404/406/3xx) : affaire hors de nos deux langues.
                    no_lang_skipped += 1;
                    counts.empty_skipped += 1;
                    continue;
                };
                if body.trim().is_empty() {
                    no_lang_skipped += 1;
                    counts.empty_skipped += 1;
                    continue;
                }
                if lang != "fra" {
                    en_fallback += 1;
                }
                // Prédicats CDM riches par CELEX (la mine de richesse, audit
                // `cjue.md`). Échec du fetch → repli minimum tracé (pas de
                // fallback silencieux).
                tokio::time::sleep(THROTTLE).await;
                let mut predicates = match client.fetch_work_predicates(&row.celex).await {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(
                            celex = %row.celex,
                            error = %e,
                            "cjue prédicats CDM indisponibles, repli minimum ecli+date"
                        );
                        serde_json::json!({})
                    }
                };
                // ECLI verbatim (jamais dérivé du CELEX, décision #3) + date :
                // toujours présents, source = liste SPARQL (le DESCRIBE peut les
                // omettre ou les multiplier). Bannière « objet » = abstract FR
                // officiel extrait du corps (pas `<title>`, audit §Champs).
                let map = predicates.as_object_mut().expect("map_predicates → objet");
                map.insert("case-law_ecli".into(), serde_json::json!(row.ecli));
                map.insert("work_date_document".into(), serde_json::json!(row.date));
                // Langue de la rendition réellement servie (FR-prioritaire, repli EN,
                // ADR 0120) — traçabilité du corpus bilingue (badge UI).
                map.insert("resource_obtained_language".into(), serde_json::json!(lang));
                if let Some(objet) = extract_objet(&body) {
                    map.insert("objet".into(), serde_json::json!(objet));
                }
                let decision = parse_cjue(&body, &predicates, &row.celex)
                    .map_err(|e| anyhow!("parse_cjue {}: {e}", row.celex))?;
                candidates.push(html_candidate(
                    decision,
                    &predicates,
                    &body,
                    iso639_2t(lang),
                ));
                if candidates.len() >= BATCH_SIZE {
                    let batch = std::mem::take(&mut candidates);
                    drain_batch(
                        &conn,
                        embedder.as_ref(),
                        batch,
                        require_embeddings,
                        IngestMode::MissingHash,
                        &mut counts,
                    )
                    .await?;
                }
            }
            if !candidates.is_empty() {
                drain_batch(
                    &conn,
                    embedder.as_ref(),
                    candidates,
                    require_embeddings,
                    IngestMode::MissingHash,
                    &mut counts,
                )
                .await?;
            }

            offset += PAGE_SIZE;
            // OFFSET cappé 10 000 (EUR-Lex) : au-delà, la page suivante est vide.
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
        no_lang_skipped,
        en_fallback,
        "ingest_cjue_total"
    );
    Ok(())
}

/// Bootstrap CEDH : balaye les années [[`from_year`] (défaut [`EUROPE_START_YEAR`]),
/// courante] en `AllPresent` : sur un re-bootstrap (corpus déjà peuplé), toute
/// affaire déjà ingérée est sautée, seuls les corps neufs (traîne EN-only, ADR
/// 0120) sont fetchés. Idempotent (`content_checksum`) — re-jouable sans dupliquer.
/// `from_year` cible un backfill (ex. trou 2022-2025) sans re-lister le fonds ancien.
pub async fn ingest_cedh(from_year: Option<i32>) -> Result<()> {
    run_cedh(
        from_year.unwrap_or(EUROPE_START_YEAR),
        current_year(),
        EuropeSkip::AllPresent,
    )
    .await
}

/// Réchauffe **uniquement** le cache disque des corps CEDH (`<cache_dir>/cedh/bodies/`)
/// pour les années [[`from_year`] (défaut [`EUROPE_START_YEAR`]), courante] — **sans
/// toucher la base** (ni pool, ni migrations). Découple le fetch réseau (throttlé,
/// une fois) de l'ingest : une fois le cache chaud, `ingest-cedh` relit tout en local
/// et seul l'embedding reste. Fetch borné en concurrence (HUDOC n'impose pas de
/// rate-limit ; ~140 ms/corps mesuré).
pub async fn cache_cedh(from_year: Option<i32>) -> Result<()> {
    use lj_sources::cedh::{parse_results, CedhClient, PAGE_SIZE};
    use std::sync::Arc;

    const CACHE_CONCURRENCY: usize = 8;

    let settings = Settings::from_env()?;
    let client = Arc::new(CedhClient::new().with_body_cache(settings.cache_dir()));
    let start_year = from_year.unwrap_or(EUROPE_START_YEAR);
    let end_year = current_year();

    let mut total_fetched = 0usize;
    let mut total_empty = 0usize;
    for year in start_year..=end_year {
        // (1) Liste seule (métadonnées), ne garder que les arrêts, comme `run_cedh`.
        let mut rows: Vec<CedhResult> = Vec::new();
        let mut start = 0usize;
        loop {
            let page = client
                .results_page(year, start)
                .await
                .map_err(|e| anyhow!("cedh results {year} start={start}: {e}"))?;
            let (resultcount, page_rows) = parse_results(&page);
            if page_rows.is_empty() {
                break;
            }
            rows.extend(
                page_rows
                    .into_iter()
                    .filter(|r| r.doctype == "HFJUD" || r.doctype == "HEJUD"),
            );
            start += PAGE_SIZE;
            if start as u64 >= resultcount || start >= 10_000 {
                break;
            }
        }
        // (2) Regroupement par affaire (FR d'abord) → on réchauffe exactement ce que
        // l'ingest lira : cascade FR→EN, on s'arrête au 1er corps non vide par affaire.
        let cases = group_cases(rows);

        // (3) Fetch borné en concurrence (une affaire par tâche) : chaque `body_text`
        // écrit le cache disque. La tâche cascade FR→EN et s'arrête au 1er corps.
        let mut year_fetched = 0usize;
        let mut year_empty = 0usize;
        for batch in cases.chunks(CACHE_CONCURRENCY) {
            let mut set = tokio::task::JoinSet::new();
            for case in batch {
                let client = Arc::clone(&client);
                let itemids: Vec<String> = case.iter().map(|r| r.itemid.clone()).collect();
                set.spawn(async move {
                    for id in &itemids {
                        match client.body_text(id).await {
                            Ok(b) if !b.trim().is_empty() => {
                                return Ok::<bool, anyhow::Error>(true)
                            }
                            Ok(_) => continue,
                            Err(e) => return Err(anyhow!("cedh body {id}: {e}")),
                        }
                    }
                    Ok(false)
                });
            }
            while let Some(joined) = set.join_next().await {
                if joined.map_err(|e| anyhow!("join cache {e}"))?? {
                    year_fetched += 1;
                } else {
                    year_empty += 1;
                }
            }
        }
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

/// Bootstrap CJUE : balaye toutes les années [[`EUROPE_START_YEAR`], courante]
/// (saute toute affaire déjà ingérée, cf. [`ingest_cedh`]).
pub async fn ingest_cjue() -> Result<()> {
    run_cjue(EUROPE_START_YEAR, current_year(), EuropeSkip::AllPresent).await
}

/// Sync incrémental CEDH : fenêtre glissante `[N-1, N]` en `FrPresentOnly` (aligné
/// sur [`sync_cjue`], ADR 0153). Ne re-fetch que les affaires neuves et celles
/// encore servies en **EN-only** : leur stub `HFJUD` (204 au run précédent) est
/// re-sondé et bascule le `full_text` en français dès que la traduction paraît
/// (upgrade FR différé, ADR 0120/0127). La fenêtre N-1 rattrape l'arrêt publié
/// EN-only fin d'année N-1 dont le FR n'arrive qu'en N. Les affaires déjà FR sont
/// sautées (définitives) → pas de re-téléchargement inutile. Le remplissage initial
/// d'une base vierge reste le bootstrap [`ingest_cedh`] (historique complet).
pub async fn sync_cedh() -> Result<()> {
    let year = current_year();
    run_cedh(year - 1, year, EuropeSkip::FrPresentOnly).await
}

/// Sync incrémental CJUE : balaye l'année courante **et la précédente** (fenêtre
/// glissante `[N-1, N]`), et ne re-fetch que les affaires neuves + celles encore
/// servies en EN-only (upgrade FR différé, ADR 0120). La fenêtre rattrape l'upgrade
/// FR d'un arrêt publié EN-only en fin d'année N-1 dont la traduction n'arrive qu'en
/// N : sans elle, le sync de janvier ne re-visiterait jamais N-1 et l'EN-only y
/// resterait figé jusqu'à un re-bootstrap. Les arrêts déjà en FR (la quasi-totalité)
/// sont sautés (cf. [`EuropeSkip::FrPresentOnly`]) : le re-listing SPARQL de N-1 ne
/// déclenche aucun re-téléchargement de corps, seuls les rares EN-only sont re-tirés.
pub async fn sync_cjue() -> Result<()> {
    let year = current_year();
    run_cjue(year - 1, year, EuropeSkip::FrPresentOnly).await
}
