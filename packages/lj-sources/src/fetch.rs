//! Téléchargement HTTP générique d'un document (PDF/HTML) au bord I/O (#1).
//!
//! Utilisé par le track d'ingest des corps d'accords/traités (ADR 0109 Addendum) :
//! le texte des accords/conventions vit hors du bulk JORF (PDF born-digital
//! consolidés, pages HTML, XHTML EUR-Lex). Frontière I/O unique : les octets bruts
//! repartent vers l'extraction texte plat (`pdf::extract_pdf_text` /
//! `html_strip::strip_html`) puis le segmenteur pur de `lj-core`.

use crate::error::{Result, SourceError};

/// GET un document et renvoie ses octets bruts. En-tête `User-Agent` explicite
/// (certains hôtes refusent les requêtes sans UA). Erreur franche (#12) sur statut
/// non-2xx ou transport.
pub async fn fetch_bytes(url: &str) -> Result<Vec<u8>> {
    fetch_bytes_with_headers(url, &[]).await
}

/// Comme [`fetch_bytes`] avec des en-têtes additionnels. Sert l'endpoint cellar de
/// l'Office des publications UE (`publications.europa.eu/resource/celex/…`), qui
/// exige `Accept-Language: fra` pour servir le français et `Accept:
/// application/xhtml+xml`, puis répond en 303 vers le document (reqwest suit les
/// redirections par défaut).
pub async fn fetch_bytes_with_headers(url: &str, headers: &[(&str, &str)]) -> Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .user_agent("librejustice-ingest/1.0 (+https://librejustice.fr)")
        .build()
        .map_err(|e| SourceError::Invalid(format!("client HTTP: {e}")))?;
    let mut req = client.get(url);
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| SourceError::Invalid(format!("GET {url}: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(SourceError::Invalid(format!(
            "GET {url}: statut {}",
            status.as_u16()
        )));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| SourceError::Invalid(format!("corps {url}: {e}")))?;
    Ok(bytes.to_vec())
}
