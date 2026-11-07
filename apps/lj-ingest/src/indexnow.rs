//! Push IndexNow — notifie Bing/Yandex des URLs (re)publiées.
//!
//! IndexNow (<https://www.indexnow.org>): a JSON POST of URLs, with a key served
//! at `https://<host>/<key>.txt`, asks the participating engines (Bing, Yandex) to
//! crawl them. No OAuth, no Google quota. The protocol caps the URLs of a request.

use anyhow::{Context, Result};
use reqwest_middleware::ClientBuilder;
use reqwest_tracing::TracingMiddleware;
use serde::Serialize;

pub const INDEXNOW_ENDPOINT: &str = "https://api.indexnow.org/indexnow";
pub const INDEXNOW_HOST: &str = "librejustice.fr";
/// IndexNow protocol cap on the URLs of a request.
pub const MAX_URLS_PER_REQUEST: usize = 10_000;

/// URL publique où la clé doit être servie (fichier `<key>.txt`).
pub fn key_location(key: &str) -> String {
    format!("https://{INDEXNOW_HOST}/{key}.txt")
}

pub fn decision_url(path: &str) -> String {
    format!("https://{INDEXNOW_HOST}{path}")
}

pub fn notice_url(uid: &str) -> String {
    format!("https://{INDEXNOW_HOST}{}", lj_vocab::notice_path(uid))
}

/// JSON body of an IndexNow submission, at most [`MAX_URLS_PER_REQUEST`] URLs.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexNowPayload {
    pub host: String,
    pub key: String,
    pub key_location: String,
    pub url_list: Vec<String>,
}

/// Construit le corps JSON d'une soumission.
///
/// Lève une erreur si `urls` dépasse la limite protocole.
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

/// POSTs the URLs to IndexNow in batches of [`MAX_URLS_PER_REQUEST`]. Returns the
/// count submitted.
///
/// Lève une erreur si un lot échoue (4xx clé invalide, 5xx…).
pub async fn submit(urls: &[String], key: &str) -> Result<usize> {
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

    #[test]
    fn url_helpers() {
        assert_eq!(
            decision_url("/decision/fr/ce_12345"),
            "https://librejustice.fr/decision/fr/ce_12345"
        );
        assert_eq!(
            notice_url("marche:1a2b3c"),
            "https://librejustice.fr/notice/fr/marche:1a2b3c"
        );
        assert_eq!(key_location("abcdef"), "https://librejustice.fr/abcdef.txt");
    }

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
