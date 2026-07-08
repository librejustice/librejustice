//! Push IndexNow — notifie Bing/Yandex des URLs (re)publiées (port de
//! `apps/ingest/librejustice/indexnow.py`).
//!
//! IndexNow (<https://www.indexnow.org>) : un POST JSON avec une liste d'URLs et
//! une clé publiée à `https://<host>/<key>.txt` suffit à signaler aux moteurs
//! participants (Bing, Yandex…) de venir crawler. Pas d'OAuth, pas de quota
//! Google. Limite protocole : 10 000 URLs par requête.
//!
//! Découplé de la boucle d'ingest (cf. ADR 0044) : la commande `librejustice
//! indexnow` interroge `decisions` par `updated_at` et soumet par lots — aucun
//! ralentissement du pipeline d'ingestion.

use anyhow::{Context, Result};
use reqwest_middleware::ClientBuilder;
use reqwest_tracing::TracingMiddleware;
use serde::Serialize;

pub const INDEXNOW_ENDPOINT: &str = "https://api.indexnow.org/indexnow";
pub const INDEXNOW_HOST: &str = "librejustice.fr";
/// Limite protocole IndexNow : 10 000 URLs par requête.
pub const MAX_URLS_PER_REQUEST: usize = 10_000;

/// URL publique où la clé doit être servie (fichier `<key>.txt`).
pub fn key_location(key: &str) -> String {
    format!("https://{INDEXNOW_HOST}/{key}.txt")
}

/// URL canonique d'une décision.
pub fn decision_url(public_id: &str) -> String {
    format!("https://{INDEXNOW_HOST}/decision/{public_id}")
}

/// Corps JSON d'une soumission IndexNow. `urls` doit faire ≤ 10 000 entrées.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexNowPayload {
    pub host: String,
    pub key: String,
    pub key_location: String,
    pub url_list: Vec<String>,
}

/// Construit le corps JSON d'une soumission (port de `build_payload`).
///
/// Lève une erreur si `urls` dépasse la limite protocole — pas de silent
/// fallback (AGENTS.md règle #12).
pub fn build_payload(key: &str, urls: Vec<String>) -> Result<IndexNowPayload> {
    if urls.len() > MAX_URLS_PER_REQUEST {
        anyhow::bail!(
            "IndexNow: {} URLs > limite protocole {MAX_URLS_PER_REQUEST}",
            urls.len()
        );
    }
    Ok(IndexNowPayload {
        host: INDEXNOW_HOST.to_string(),
        key: key.to_string(),
        key_location: key_location(key),
        url_list: urls,
    })
}

/// POST les URLs décision à IndexNow par lots de 10 000. Retourne le nb soumis.
///
/// Lève une erreur si un lot échoue (4xx clé invalide, 5xx…) — pas de silent
/// fallback (AGENTS.md règle #12). Port de `submit`.
pub async fn submit(public_ids: &[String], key: &str) -> Result<usize> {
    let urls: Vec<String> = public_ids.iter().map(|pid| decision_url(pid)).collect();
    if urls.is_empty() {
        tracing::info!("indexnow_noop aucune_url");
        return Ok(0);
    }
    let client = ClientBuilder::new(
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("indexnow: construction du client HTTP")?,
    )
    .with(TracingMiddleware::default())
    .build();
    let total = urls.len();
    for batch in urls.chunks(MAX_URLS_PER_REQUEST) {
        let payload = build_payload(key, batch.to_vec())?;
        let response = client
            .post(INDEXNOW_ENDPOINT)
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&payload)
            .send()
            .await
            .context("indexnow: POST")?;
        let status = response.status();
        let response = response
            .error_for_status()
            .with_context(|| format!("indexnow: lot rejeté (status {status})"))?;
        tracing::info!(soumis = batch.len(), status = %response.status(), "indexnow_batch");
    }
    tracing::info!(total, "indexnow_done");
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec : URLs canoniques (host en dur, schéma /decision/<id>).
    #[test]
    fn url_helpers() {
        assert_eq!(
            decision_url("ce_12345"),
            "https://librejustice.fr/decision/ce_12345"
        );
        assert_eq!(key_location("abcdef"), "https://librejustice.fr/abcdef.txt");
    }

    // Spec : build_payload sérialise en camelCase (keyLocation/urlList).
    #[test]
    fn payload_camel_case() {
        let payload =
            build_payload("k", vec!["https://librejustice.fr/decision/a".into()]).unwrap();
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["host"], "librejustice.fr");
        assert_eq!(json["key"], "k");
        assert_eq!(json["keyLocation"], "https://librejustice.fr/k.txt");
        assert_eq!(json["urlList"][0], "https://librejustice.fr/decision/a");
    }

    // Spec : dépassement de la limite protocole = erreur franche.
    #[test]
    fn payload_rejects_overflow() {
        let urls = vec!["x".to_string(); MAX_URLS_PER_REQUEST + 1];
        assert!(build_payload("k", urls).is_err());
    }

    #[test]
    fn payload_accepts_limit_exactly() {
        let urls = vec!["x".to_string(); MAX_URLS_PER_REQUEST];
        assert!(build_payload("k", urls).is_ok());
    }
}
