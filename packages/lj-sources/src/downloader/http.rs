//! Helpers HTTP synchrones partagés : GET retentés (corps en RAM ou streamé sur
//! disque) et un util de chemin. Le streaming sur disque sert aussi les stocks
//! DILA multi-Go (`crate::dila`).

use crate::error::{Result, SourceError};
use std::fs;
use std::path::Path;

/// GET retenté avec backoff, **envoi ET lecture du corps comme une unité**.
///
/// Sur statut 200, lit le corps et le renvoie ; sinon (304/404/autre) renvoie le
/// statut sans corps. Le point clé : une coupure en plein stream remonte une
/// erreur reqwest `kind=Decode` **après** que `send()` a renvoyé `Ok` — un retry
/// qui n'envelopperait que `send()` la laisserait passer (c'est le crash du
/// 2026-06-09). Ici le `bytes()` est dans la même tentative, donc retenté aussi.
/// Renvoie `(status, last_modified, corps si 200)`.
pub(crate) fn get_with_body_retrying(
    url: &str,
    mut attempt: impl FnMut() -> reqwest::Result<reqwest::blocking::Response>,
) -> reqwest::Result<(u16, Option<String>, Option<Vec<u8>>)> {
    const MAX_TRIES: u32 = 4;
    let read = |resp: reqwest::blocking::Response| -> reqwest::Result<_> {
        let status = resp.status().as_u16();
        if status != 200 {
            return Ok((status, None, None));
        }
        let last_mod = resp
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body = resp.bytes()?.to_vec();
        Ok((status, last_mod, Some(body)))
    };
    let mut backoff = std::time::Duration::from_millis(500);
    let mut last_err = None;
    for try_no in 1..=MAX_TRIES {
        match attempt().and_then(read) {
            Ok(v) => return Ok(v),
            Err(e) => {
                tracing::warn!(url, try_no, error = %e, "GET échoué (envoi ou corps), retry");
                last_err = Some(e);
                if try_no < MAX_TRIES {
                    std::thread::sleep(backoff);
                    backoff *= 2;
                }
            }
        }
    }
    Err(last_err.expect("MAX_TRIES >= 1 → au moins une erreur capturée"))
}

/// GET retenté avec backoff, **corps streamé directement sur disque** (RAM
/// ~constante quelle que soit la taille — stocks DILA multi-Go, ZIP opendata).
///
/// Même invariant de retry que [`get_with_body_retrying`] : l'envoi ET le stream
/// du corps sont dans la MÊME tentative (une coupure en plein stream est une
/// erreur post-`send()` → retentée). `dst` est (re)créé/tronqué à chaque
/// tentative — un stream partiel d'une tentative ratée est jeté. Sur 200 :
/// streame le corps dans `dst`, renvoie `(200, last_modified, octets_écrits)`.
/// Sinon (304/404/…) : n'écrit rien, renvoie `(status, last_modified, 0)`. Les
/// erreurs réseau (reqwest) comme disque (io) sont retentées (les deux portent
/// `#[from]` sur `SourceError`).
pub(crate) fn get_to_file_retrying(
    url: &str,
    dst: &Path,
    mut attempt: impl FnMut() -> reqwest::Result<reqwest::blocking::Response>,
) -> Result<(u16, Option<String>, u64)> {
    const MAX_TRIES: u32 = 4;
    let stream = |resp: reqwest::blocking::Response| -> Result<(u16, Option<String>, u64)> {
        let status = resp.status().as_u16();
        let last_mod = resp
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        if status != 200 {
            return Ok((status, last_mod, 0));
        }
        let mut resp = resp;
        let mut file = fs::File::create(dst)?;
        let n = resp.copy_to(&mut file)?;
        Ok((status, last_mod, n))
    };
    let mut backoff = std::time::Duration::from_millis(500);
    let mut last_err: Option<SourceError> = None;
    for try_no in 1..=MAX_TRIES {
        match attempt().map_err(SourceError::from).and_then(&stream) {
            Ok(v) => return Ok(v),
            Err(e) => {
                tracing::warn!(url, try_no, error = %e, "GET→fichier échoué (envoi/corps/disque), retry");
                last_err = Some(e);
                if try_no < MAX_TRIES {
                    std::thread::sleep(backoff);
                    backoff *= 2;
                }
            }
        }
    }
    Err(last_err.expect("MAX_TRIES >= 1 → au moins une erreur capturée"))
}

/// `<chemin>.<ext>` (équivalent de `Path.with_suffix(suffix + ".ext")` Python
/// qui *ajoute* l'extension, ne la remplace pas).
pub(super) fn path_with_added_extension(path: &Path, added: &str) -> std::path::PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".");
    s.push(added);
    std::path::PathBuf::from(s)
}
