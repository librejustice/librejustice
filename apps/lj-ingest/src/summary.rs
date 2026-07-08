//! Backfill offline du champ `decisions.summary` via Mistral (port de
//! `apps/ingest/librejustice/summary.py` + `librejustice_api/{mistral,titles}.py`).
//!
//! Pipeline :
//!
//! 1. Itération keyset sur `decisions` : rows dont `summary IS NULL` ou
//!    `summary_prompt_version < target_version`.
//! 2. Pour chaque row, on lit le texte intégral (`decisions.full_text`, ADR
//!    0084) et on l'envoie à Mistral.
//! 3. UPDATE via `DecisionRepository::set_summary` (pas de SQL inline).
//!
//! Concurrence asyncio → ici un sémaphore Tokio. Retry exponentiel sur 429/5xx
//! avec jitter (max 5 tentatives). Rotation de clés round-robin.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use lj_core::summary::{build_summary_input, clean_summary, SUMMARY_PROMPT};
use lj_llm::mistral::{backoff_delay_s, is_retryable_status, MistralClient, MistralError};
use lj_store::repository::{DecisionRepository, MissingSummaryRow};
use tokio::sync::Semaphore;

use crate::config::Settings;

const MAX_RETRIES: u32 = 5;

/// Concurrence par défaut **par clé** du pool chat (`concurrency=None`). Calibrée
/// sur ~5 RPS/clé soutenables × ~1,5 s de latence/requête ≈ 7,5 requêtes en vol/clé
/// pour saturer le quota sans le franchir. Round-robin par requête ⇒ multiplie par
/// le nombre de clés. La vraie borne du fournisseur est par minute (RPS×60) et tolère
/// le burst : ce sémaphore vise le régime *soutenu* (backfill), le burst d'un run cron
/// (quelques centaines de décisions) passe sous le budget minute sans 429.
const CONCURRENCY_PER_KEY: usize = 7;

/// Libellés humains du `juridiction_type` (port de `juridictionTypeLabels`).
fn juridiction_label(juridiction_type: &str) -> Option<&'static str> {
    Some(match juridiction_type {
        "TA" => "Tribunal administratif",
        "CAA" => "Cour administrative d'appel",
        "CE" => "Conseil d'État",
        "CC" => "Cour de cassation",
        "CA" => "Cour d'appel",
        "TJ" => "Tribunal judiciaire",
        "TCOM" => "Tribunal de commerce",
        _ => return None,
    })
}

const FR_MONTHS: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

/// « 2026-05-29 » → « 29 mai 2026 ». Passe-plat si pas une date ISO complète.
/// Port de `_format_fr_date`.
fn format_fr_date(date_lecture: &str) -> String {
    let parts: Vec<&str> = date_lecture.split('-').collect();
    if parts.len() != 3 {
        return date_lecture.to_string();
    }
    let (Ok(year), Ok(month), Ok(day)) = (
        parts[0].parse::<i32>(),
        parts[1].parse::<usize>(),
        parts[2].parse::<i32>(),
    ) else {
        return date_lecture.to_string();
    };
    match FR_MONTHS.get(month.wrapping_sub(1)) {
        Some(m) => format!("{day} {m} {year}"),
        None => date_lecture.to_string(),
    }
}

/// Nom de juridiction affichable : `jurisdiction_name` assaini, sinon libellé.
/// Port de `decision_jurisdiction`.
fn decision_jurisdiction(juridiction_type: &str, jurisdiction_name: Option<&str>) -> String {
    if let Some(name) = jurisdiction_name {
        let cleaned = name.trim().replace(" ,", ",");
        let cleaned = cleaned.trim();
        if !cleaned.is_empty() {
            return cleaned.to_string();
        }
    }
    juridiction_label(juridiction_type)
        .unwrap_or(juridiction_type)
        .to_string()
}

/// Titre lisible « <juridiction>, <date FR>, <numéro> » (port de
/// `decision_title`). Date/numéro optionnels.
pub fn decision_title(
    juridiction_type: &str,
    jurisdiction_name: Option<&str>,
    date_lecture: Option<&str>,
    docket_numbers: Option<&[String]>,
) -> String {
    let mut parts = vec![decision_jurisdiction(juridiction_type, jurisdiction_name)];
    if let Some(date) = date_lecture {
        parts.push(format_fr_date(date));
    }
    if let Some(dockets) = docket_numbers {
        if let Some(first) = dockets.first() {
            parts.push(first.clone());
        }
    }
    parts.join(", ")
}

/// Génère le résumé neutre (≤500 c) d'une décision via le client Mistral
/// partagé (port de `generate_summary`). En-tête « [Décision] <titre> » pour
/// donner au modèle la juridiction exacte ; couche déterministe `clean_summary`
/// pour retirer les codes d'anonymisation résiduels.
async fn generate_summary(
    client: &MistralClient,
    body_text: &str,
    title: &str,
) -> Result<String, MistralError> {
    let user_content = build_summary_input(&format!("[Décision] {title}\n\n{body_text}"));
    // `prompt_cache_key` stable : SUMMARY_PROMPT (préfixe système) est identique sur
    // toutes les décisions du backfill → tokens du prompt facturés à 10 % (cache Mistral).
    let raw = client
        .chat(SUMMARY_PROMPT, &user_content, Some(400), Some("sum-v4"))
        .await?;
    Ok(clean_summary(&raw))
}

/// Appelle `generate_summary` avec back-off exponentiel + jitter (port de
/// `_call_with_retry`).
///
/// Retries sur 429/5xx et erreurs de transport. Tout 4xx ≠ 429 remonte
/// immédiatement.
pub async fn call_with_retry(
    client: &MistralClient,
    body_text: &str,
    title: &str,
    public_id: &str,
) -> Result<String, MistralError> {
    let mut last_err: Option<MistralError> = None;
    for attempt in 0..MAX_RETRIES {
        match generate_summary(client, body_text, title).await {
            Ok(summary) => return Ok(summary),
            Err(MistralError::Status(code)) if !is_retryable_status(code) => {
                return Err(MistralError::Status(code));
            }
            Err(err) => last_err = Some(err),
        }
        let delay = backoff_delay_s(attempt, rand01());
        tracing::warn!(
            public_id,
            attempt = attempt + 1,
            max = MAX_RETRIES,
            backoff = delay,
            error = %last_err.as_ref().map(|e| e.to_string()).unwrap_or_default(),
            "summary_retry"
        );
        tokio::time::sleep(std::time::Duration::from_secs_f64(delay)).await;
    }
    Err(last_err.expect("au moins une tentative a échoué"))
}

/// Format ETA « 12h03m04s » (port de `_format_eta`).
pub fn format_eta(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    format!("{h}h{m:02}m{sec:02}s")
}

/// Corps de la décision pour le résumé, depuis `decisions.full_text` (= le
/// `texte_integral_clean` indexé, ADR 0084 — le payload brut a été droppé).
///
/// On découpe `full_text` en lignes, on trim chacune, on jette les vides, et on
/// joint par `\n\n` (= `_decision_paragraphs` joint par `"\n\n"` côté Python).
/// `None` si pas de paragraphe non vide.
fn body_text_from_full_text(full_text: &str) -> Option<String> {
    let paragraphs: Vec<&str> = full_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if paragraphs.is_empty() {
        return None;
    }
    Some(paragraphs.join("\n\n"))
}

/// Tirage `rand01` ∈ [0, 1) non cryptographique (jitter / sampling). Dérivé de
/// l'horloge — suffisant pour désynchroniser des workers / varier l'ordre.
fn rand01() -> f64 {
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0) as f64)
        / 1_000_000_000.0
}

/// Backfill `decisions.summary` jusqu'à `summary_prompt_version >=
/// target_version` pour toutes les décisions ayant un payload source (port de
/// `backfill_summaries`).
///
/// Lecture et écriture sur deux connexions distinctes du pool. Les appels
/// Mistral sont concurrents via un sémaphore ; les writes sont sérialisés (une
/// connexion d'écriture, un UPDATE par décision). `concurrency=None` ⇒ dérivé du
/// nombre de clés du pool (cf. [`CONCURRENCY_PER_KEY`]).
pub async fn backfill_summaries(
    target_version: i16,
    concurrency: Option<usize>,
    batch_size: i64,
    limit: Option<i64>,
    shuffle: bool,
) -> Result<()> {
    let settings = Settings::from_env()?;
    if settings.mistral_api_keys.is_empty() {
        return Err(anyhow!(
            "LIBREJUSTICE_MISTRAL_API_KEYS vide — impossible de générer des summaries."
        ));
    }
    // Les clés tournent en round-robin par requête : le débit soutenable scale
    // avec le pool. À défaut d'override, on pose ~`CONCURRENCY_PER_KEY` requêtes en
    // vol par clé (sémaphore + back-off s'auto-throttlent ensuite si on déborde).
    let keys = settings.mistral_api_keys.len();
    let concurrency = concurrency.unwrap_or(keys * CONCURRENCY_PER_KEY).max(1);
    tracing::info!(keys, concurrency, "summary_backfill_concurrency");

    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let fetch_conn = pool
        .get()
        .await
        .map_err(|e| anyhow!("pool.get fetch: {e}"))?;
    let write_conn = pool
        .get()
        .await
        .map_err(|e| anyhow!("pool.get write: {e}"))?;
    // Backfill de maintenance : on lève la borne API (build_pool pose
    // statement_timeout=30s). Le scan de frontière des décisions sans résumé sur
    // ~3,5 M lignes dépasse 30s — batch long assumé, pas une requête interactive.
    fetch_conn
        .batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout fetch: {e}"))?;
    write_conn
        .batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout write: {e}"))?;
    let fetch_repo = DecisionRepository::new(&fetch_conn);
    let write_repo = DecisionRepository::new(&write_conn);

    let client = Arc::new(MistralClient::new(
        settings.mistral_api_keys.clone(),
        settings.mistral_model.clone(),
    )?);
    let sem = Arc::new(Semaphore::new(concurrency));

    // Sampling de review : démarre la pagination keyset à un id aléatoire (avec
    // wraparound côté repo), sinon le backfill commence toujours par les mêmes
    // petits id.
    let start_id = if shuffle {
        let max = fetch_repo.max_decision_id().await?;
        if max > 0 {
            (rand01() * (max as f64)) as i64
        } else {
            0
        }
    } else {
        0
    };

    let start = std::time::Instant::now();
    let mut processed: usize = 0;
    let mut persisted: usize = 0;

    // Le repo matérialise les batches (frontières identiques au générateur
    // Python) ; on traite chaque batch en concurrence bornée puis on persiste.
    let batches = fetch_repo
        .iter_decisions_missing_summary(target_version, batch_size, limit, start_id, shuffle)
        .await?;

    for batch in batches {
        // Reconstruit le corps de chaque décision (I/O DB séquentiel sur la
        // connexion de lecture) avant de lancer les appels Mistral.
        let mut jobs: Vec<(i64, String, String)> = Vec::with_capacity(batch.len());
        for row in &batch {
            let title = row_title(row);
            let body = fetch_repo
                .fetch_full_texts(&[row.decision_id])
                .await?
                .into_iter()
                .next()
                .and_then(|(_, ft)| body_text_from_full_text(&ft));
            match body {
                Some(body) => jobs.push((row.decision_id, body, title)),
                None => tracing::warn!(
                    public_id = %row.public_id,
                    decision_id = row.decision_id,
                    "summary_no_body"
                ),
            }
        }

        // Appels Mistral concurrents (sémaphore = `concurrency`).
        let mut tasks = Vec::with_capacity(jobs.len());
        for (decision_id, body, title) in jobs {
            let client = Arc::clone(&client);
            let sem = Arc::clone(&sem);
            tasks.push(tokio::spawn(async move {
                let _permit = sem.acquire().await.expect("sémaphore non fermé");
                let result = call_with_retry(&client, &body, &title, "").await;
                (decision_id, result)
            }));
        }

        processed += batch.len();
        // Persiste les résumés au fil des complétions (writes sérialisés).
        for task in tasks {
            let (decision_id, result) = task.await.map_err(|e| anyhow!("join: {e}"))?;
            match result {
                Ok(summary) if !summary.is_empty() => {
                    write_repo
                        .set_summary(decision_id, &summary, target_version)
                        .await
                        .map_err(|e| anyhow!("set_summary id={decision_id}: {e}"))?;
                    persisted += 1;
                }
                Ok(_) => {
                    tracing::warn!(decision_id, "summary_empty");
                }
                Err(e) => {
                    tracing::error!(decision_id, error = %e, "summary_failed");
                }
            }
        }
        let elapsed = start.elapsed().as_secs_f64();
        let rate = processed as f64 / elapsed.max(1e-6);
        tracing::info!(
            processed,
            persisted,
            rate,
            elapsed = %format_eta(elapsed),
            "summary_progress"
        );
    }

    tracing::info!(
        processed,
        persisted,
        elapsed = %format_eta(start.elapsed().as_secs_f64()),
        "summary_done"
    );
    Ok(())
}

/// Titre lisible d'une ligne de backfill (port du `decision_title(...)` injecté
/// dans le prompt). `docket_numbers` → premier numéro éventuel.
fn row_title(row: &MissingSummaryRow) -> String {
    decision_title(
        &row.juridiction_type,
        row.jurisdiction_name.as_deref(),
        row.date_lecture.as_deref(),
        row.docket_numbers.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec : _format_fr_date « 2026-05-29 » → « 29 mai 2026 », passe-plat sinon.
    #[test]
    fn fr_date() {
        assert_eq!(format_fr_date("2026-05-29"), "29 mai 2026");
        assert_eq!(format_fr_date("2026-01-01"), "1 janvier 2026");
        assert_eq!(format_fr_date("2026-12-31"), "31 décembre 2026");
        assert_eq!(format_fr_date("pas-une-date-x"), "pas-une-date-x");
        assert_eq!(format_fr_date("2026/05/29"), "2026/05/29");
    }

    // Spec : decision_title « <juridiction>, <date>, <numéro> ».
    #[test]
    fn title_full() {
        let t = decision_title(
            "CC",
            Some("Cour de cassation"),
            Some("2026-05-29"),
            Some(&["24-17.384".to_string()]),
        );
        assert_eq!(t, "Cour de cassation, 29 mai 2026, 24-17.384");
    }

    // Spec : sans jurisdiction_name on retombe sur le libellé du type (jamais
    // le code brut).
    #[test]
    fn title_falls_back_to_label() {
        let t = decision_title("TA", None, Some("2026-05-29"), None);
        assert_eq!(t, "Tribunal administratif, 29 mai 2026");
    }

    // Spec : jurisdiction_name est assaini (« X , » → « X, ») et trimé.
    #[test]
    fn jurisdiction_sanitised() {
        assert_eq!(
            decision_jurisdiction("TA", Some("  Tribunal administratif de Paris ,  ")),
            "Tribunal administratif de Paris,"
        );
        assert_eq!(decision_jurisdiction("CE", Some("   ")), "Conseil d'État");
    }

    // Spec : format_eta « HhMMmSSs » zero-paddé minutes/secondes.
    #[test]
    fn eta_format() {
        assert_eq!(format_eta(0.0), "0h00m00s");
        assert_eq!(format_eta(3661.0), "1h01m01s");
        assert_eq!(format_eta(-5.0), "0h00m00s");
    }
}
