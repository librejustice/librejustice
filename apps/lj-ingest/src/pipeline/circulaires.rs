//! Circulaires (fond DILA CIRCULAIRES, ADR 0196) : sync du fond complet
//! (stocks 2009-2014 + abrogations historiques + flux ≥ 2014) → `legal_text`
//! nature=`CIRCULAIRE`, identité `cir_<ID>`, NOR en cascade ADR 0115, état
//! V/A en `status`, résumé en `body`.
//!
//! **Corps** (ADR 0222, [`sync_circulaires_bodies`]) : passe séparée qui
//! rejoue les stocks PDF (`pdf/<année>_pdf.tar.gz`) puis les PDF compagnons
//! des flux — `pdftotext` déterministe, OCR Mistral cache-first en repli
//! scanné (pattern ADR 0124) — et pose `body` = corps extrait. Le sync
//! métadonnées écrit le résumé DILA (≤ 5 000 caractères) en `body`
//! provisoire ; la passe corps le remplace quand un PDF exploitable existe.
//!
//! Rejeu : stocks (par année) → `abroge_txt.tar` (flips d'état historiques)
//! → flux en ordre chronologique (**last-write-wins par `ID_CIRCULAIRE`** —
//! une abrogation moderne arrive en re-export `ETAT=A`). Chaque fichier
//! ingéré est marqué au manifest (`mark_circulaire_done`) : un crash reprend
//! où il s'était arrêté. Idempotent (#7) : upsert par `text_uid`.

use std::path::Path;

use anyhow::{anyhow, Result};

use lj_llm::mistral::{ocr_with_retry, MistralClient};
use lj_sources::circulaires::{CircKind, Circulaire};
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Sync du fond CIRCULAIRES : planifie (listing + téléchargements), ingère
/// chaque fichier nouveau dans l'ordre de rejeu, marque le manifest, puis pose
/// les slugs des textes nouveaux (ADR 0162).
pub async fn sync_circulaires() -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.cache_dir().join("circulaires");

    let plan = {
        let dir = dir.clone();
        tokio::task::spawn_blocking(move || lj_sources::circulaires::plan_circulaires_sync(&dir))
            .await
            .map_err(|e| anyhow!("tâche plan circulaires: {e}"))?
            .map_err(|e| anyhow!("plan_circulaires_sync: {e}"))?
    };

    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let (mut upserted, mut abroges, mut errors) = (0usize, 0u64, 0usize);
    for tarball in &plan {
        match tarball.kind {
            CircKind::Stock | CircKind::Flux => {
                let (u, e) = ingest_circulaires_tarball(&repo, &tarball.path).await?;
                upserted += u;
                errors += e;
            }
            CircKind::Abroge => {
                abroges += apply_abroge_tar(&repo, &tarball.path).await?;
            }
        }
        // Fichier rejoué avec succès → marqué (repris au prochain run sinon).
        let (dir, name) = (dir.clone(), tarball.name.clone());
        tokio::task::spawn_blocking(move || {
            lj_sources::circulaires::mark_circulaire_done(&dir, &name)
        })
        .await
        .map_err(|e| anyhow!("tâche manifest circulaires: {e}"))?
        .map_err(|e| anyhow!("mark_circulaire_done {}: {e}", tarball.name))?;
    }

    // Slugs des textes nouveaux (ADR 0162) — pas de refresh code_title : la
    // famille n'a pas d'articles.
    let slugged = super::slugs::assign_text_slugs(&repo).await?;

    tracing::info!(
        files = plan.len(),
        upserted,
        abroges,
        errors,
        slugged,
        "sync_circulaires"
    );
    Ok(())
}

/// Passe corps du fond CIRCULAIRES (ADR 0222) : rejoue les tarballs PDF
/// (stocks puis flux, last-write-wins par id), extrait chaque PDF
/// (`pdftotext` natif, OCR Mistral cache-first en repli scanné) et pose
/// `legal_text.body`. Un repli OCR qui manque (clé absente/épuisée) dépose
/// le PDF en file d'attente (`ocr-pending/`), drainée en tête de chaque run :
/// le retry coûte O(fichiers manqués), jamais un re-stream de tarball — le
/// tarball est donc marqué au manifest inconditionnellement.
pub async fn sync_circulaires_bodies() -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.cache_dir().join("circulaires");

    let plan = {
        let dir = dir.clone();
        tokio::task::spawn_blocking(move || {
            lj_sources::circulaires::plan_circulaires_pdf_sync(&dir)
        })
        .await
        .map_err(|e| anyhow!("tâche plan circulaires pdf: {e}"))?
        .map_err(|e| anyhow!("plan_circulaires_pdf_sync: {e}"))?
    };

    // Repli OCR marginal, pool standard. Client **collant** : on consomme une
    // clé seule puis on bascule à l'épuisement (ADR 0124).
    let ocr_client = if settings.mistral_api_keys.is_empty() {
        None
    } else {
        Some(MistralClient::new_sticky(
            settings.mistral_api_keys.clone(),
            "mistral-ocr-latest".to_string(),
        )?)
    };

    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let mut total = PdfCounts::default();
    drain_pending_ocr(&repo, ocr_client.as_ref(), &dir, &mut total).await?;
    for tarball in &plan {
        let counts =
            ingest_circulaire_pdfs(&repo, ocr_client.as_ref(), &dir, &tarball.path).await?;
        total.add(&counts);
        let (dir, name) = (dir.clone(), tarball.name.clone());
        tokio::task::spawn_blocking(move || {
            lj_sources::circulaires::mark_circulaire_pdf_done(&dir, &name)
        })
        .await
        .map_err(|e| anyhow!("tâche manifest circulaires pdf: {e}"))?
        .map_err(|e| anyhow!("mark_circulaire_pdf_done {}: {e}", tarball.name))?;
    }

    tracing::info!(
        files = plan.len(),
        updated = total.updated,
        ocr = total.ocr,
        ocr_missed = total.ocr_missed,
        missing_row = total.missing_row,
        errors = total.errors,
        "sync_circulaires_bodies"
    );
    Ok(())
}

/// Draine la file d'attente OCR (`ocr-pending/`) : les PDF scannés dont le
/// repli a manqué à un run précédent. Cache-first puis OCR live ; une entrée
/// résolue pose le corps et sort de la file, une entrée encore sans clé y
/// reste (re-comptée `ocr_missed`).
async fn drain_pending_ocr(
    repo: &DecisionRepository<'_>,
    ocr_client: Option<&MistralClient>,
    circ_dir: &Path,
    total: &mut PdfCounts,
) -> Result<()> {
    let pending = {
        let dir = circ_dir.to_path_buf();
        tokio::task::spawn_blocking(move || {
            lj_sources::circulaires::list_pending_circulaire_pdfs(&dir)
        })
        .await
        .map_err(|e| anyhow!("tâche liste ocr-pending: {e}"))?
        .map_err(|e| anyhow!("list_pending_circulaire_pdfs: {e}"))?
    };
    if pending.is_empty() {
        return Ok(());
    }
    tracing::info!(pending = pending.len(), "circulaires: drainage ocr-pending");
    for (id, path) in pending {
        let bytes = tokio::fs::read(&path).await?;
        let Some(md) = scanned_ocr_fallback(ocr_client, circ_dir, &id, &bytes).await else {
            total.ocr_missed += 1;
            continue;
        };
        total.ocr += 1;
        let body = lj_core::parsing::clean_ocr_markdown(&md);
        if repo
            .set_legal_text_body(&id, &body)
            .await
            .map_err(|e| anyhow!("set_legal_text_body {id}: {e}"))?
        {
            total.updated += 1;
        } else {
            tracing::debug!(id = %id, "circulaire: pdf orphelin (pas de ligne legal_text)");
            total.missing_row += 1;
        }
        if let Err(e) = lj_sources::circulaires::remove_pending_circulaire_pdf(circ_dir, &id) {
            tracing::warn!(id = %id, error = %e, "circulaire: retrait ocr-pending");
        }
    }
    Ok(())
}

/// Compteurs de la passe corps, agrégés par tarball puis au total.
#[derive(Debug, Default)]
struct PdfCounts {
    /// `body` posé.
    updated: usize,
    /// Scannés passés par l'OCR (cache compris).
    ocr: usize,
    /// Scannés sans OCR possible (pas de clé / échec) — en file `ocr-pending`.
    ocr_missed: usize,
    /// PDF sans ligne `legal_text` (orphelin du fond, jamais créé).
    missing_row: usize,
    /// PDF illisibles (`pdftotext` en échec : corrompu, chiffré).
    errors: usize,
}

impl PdfCounts {
    fn add(&mut self, o: &Self) {
        self.updated += o.updated;
        self.ocr += o.ocr;
        self.ocr_missed += o.ocr_missed;
        self.missing_row += o.missing_row;
        self.errors += o.errors;
    }
}

/// Un membre PDF classé par le lecteur (l'extraction `pdftotext` tourne sur
/// le thread bloquant de lecture, pas sur l'exécuteur async).
enum PdfMsg {
    /// Couche texte native exploitable → corps prêt.
    Native(String, String),
    /// PDF scanné (couche texte sous le plancher) → octets pour l'OCR.
    Scanned(String, Vec<u8>),
    /// `pdftotext` en échec (corrompu/chiffré) — loggé côté lecteur.
    Unreadable,
}

/// Appels OCR Mistral en vol simultanément. Une clé doc ne vit que ~30 min
/// après sa 1ʳᵉ utilisation depuis l'IP datacenter, quel que soit le volume :
/// on sature la fenêtre au lieu de la laisser s'écouler entre deux appels.
/// Les 429 éventuels sont absorbés par le back-off d'[`ocr_with_retry`].
const OCR_CONCURRENCY: usize = 8;

/// Un membre PDF résolu en corps (ou compté) par le pipeline concurrent.
enum PdfOutcome {
    /// Corps prêt à poser (`ocr` = passé par le repli OCR, cache compris).
    Body { id: String, body: String, ocr: bool },
    /// Scanné sans OCR possible (pas de clé / échec).
    OcrMissed,
    /// `pdftotext` en échec (corrompu, chiffré).
    Unreadable,
}

/// Rejoue un tarball PDF : chaque membre `cir_N.pdf` → corps → UPDATE `body`.
/// Les replis OCR tournent à [`OCR_CONCURRENCY`] appels en vol (spike : on
/// tire le maximum de la fenêtre de vie de la clé) ; les écritures DB restent
/// séquentielles sur la connexion unique.
async fn ingest_circulaire_pdfs(
    repo: &DecisionRepository<'_>,
    ocr_client: Option<&MistralClient>,
    circ_dir: &Path,
    path: &Path,
) -> Result<PdfCounts> {
    use futures::StreamExt;

    let (tx, rx) = tokio::sync::mpsc::channel::<PdfMsg>(8);
    let tar_path = path.to_path_buf();
    let reader = tokio::task::spawn_blocking(move || -> Result<()> {
        lj_sources::tar_reader::for_each_member(&tar_path, |name, raw| {
            let Some(id) = lj_sources::circulaires::circulaire_pdf_member_id(&name) else {
                return Ok(());
            };
            let msg = match lj_sources::pdf::pdftotext_extract(&raw) {
                Ok(text) => match body_from_text_layer(&text) {
                    Some(body) => PdfMsg::Native(id, body),
                    None => PdfMsg::Scanned(id, raw),
                },
                Err(e) => {
                    tracing::warn!(member = %name, error = %e, "circulaire: pdf illisible");
                    PdfMsg::Unreadable
                }
            };
            tx.blocking_send(msg)
                .map_err(|_| anyhow!("canal circulaires pdf fermé (consumer arrêté)"))
        })
    });

    let outcomes = futures::stream::unfold(
        rx,
        |mut rx| async move { rx.recv().await.map(|m| (m, rx)) },
    )
    .map(|msg| async move {
        match msg {
            PdfMsg::Native(id, body) => PdfOutcome::Body {
                id,
                body,
                ocr: false,
            },
            PdfMsg::Scanned(id, bytes) => {
                match scanned_ocr_fallback(ocr_client, circ_dir, &id, &bytes).await {
                    Some(md) => PdfOutcome::Body {
                        body: lj_core::parsing::clean_ocr_markdown(&md),
                        id,
                        ocr: true,
                    },
                    None => {
                        if let Err(e) = lj_sources::circulaires::save_pending_circulaire_pdf(
                            circ_dir, &id, &bytes,
                        ) {
                            tracing::warn!(id = %id, error = %e, "circulaire: dépôt ocr-pending");
                        }
                        PdfOutcome::OcrMissed
                    }
                }
            }
            PdfMsg::Unreadable => PdfOutcome::Unreadable,
        }
    })
    .buffer_unordered(OCR_CONCURRENCY);
    let mut outcomes = std::pin::pin!(outcomes);

    let mut counts = PdfCounts::default();
    while let Some(outcome) = outcomes.next().await {
        let (id, body) = match outcome {
            PdfOutcome::Body { id, body, ocr } => {
                counts.ocr += usize::from(ocr);
                (id, body)
            }
            PdfOutcome::OcrMissed => {
                counts.ocr_missed += 1;
                continue;
            }
            PdfOutcome::Unreadable => {
                counts.errors += 1;
                continue;
            }
        };
        if repo
            .set_legal_text_body(&id, &body)
            .await
            .map_err(|e| anyhow!("set_legal_text_body {id}: {e}"))?
        {
            counts.updated += 1;
        } else {
            tracing::debug!(id = %id, "circulaire: pdf orphelin (pas de ligne legal_text)");
            counts.missing_row += 1;
        }
    }
    reader
        .await
        .map_err(|e| anyhow!("tâche lecture circulaires pdf {}: {e}", path.display()))??;
    tracing::info!(
        source = %path.display(),
        updated = counts.updated,
        ocr = counts.ocr,
        ocr_missed = counts.ocr_missed,
        missing_row = counts.missing_row,
        errors = counts.errors,
        "ingest_circulaire_pdfs"
    );
    Ok(counts)
}

/// Plancher moyen de caractères non-blancs **par page** au-dessus duquel la
/// couche texte `pdftotext` est réputée native (ADR 0222). Un scan rend quasi
/// rien ; la moyenne par page évite qu'une page de garde native fasse passer
/// un document scanné pour du texte.
const PDF_TEXT_LAYER_FLOOR_PER_PAGE: usize = 100;

/// Corps depuis la couche texte `pdftotext` ; `None` = PDF réputé scanné.
fn body_from_text_layer(extracted: &str) -> Option<String> {
    let pages = extracted.matches('\x0c').count() + 1;
    let non_ws = extracted.chars().filter(|c| !c.is_whitespace()).count();
    if non_ws / pages < PDF_TEXT_LAYER_FLOOR_PER_PAGE {
        return None;
    }
    Some(extracted.replace('\x0c', "\n\n").trim().to_string())
}

/// Repli OCR pour un PDF scanné, cache-first (`<circ_dir>/ocr/<id>.md`) —
/// zéro appel live si déjà OCR-isé ; sinon OCR Mistral si une clé est
/// disponible (puis cache). `None` si pas de cache ET pas de clé/échec →
/// l'appelant compte un `ocr_missed`, le tarball sera rejoué.
async fn scanned_ocr_fallback(
    client: Option<&MistralClient>,
    circ_dir: &Path,
    id: &str,
    pdf_bytes: &[u8],
) -> Option<String> {
    use lj_sources::circulaires::{load_cached_circulaire_ocr, save_cached_circulaire_ocr};
    if let Ok(Some(md)) = load_cached_circulaire_ocr(circ_dir, id) {
        return Some(md);
    }
    let client = client?;
    match ocr_with_retry(client, pdf_bytes, id).await {
        Ok(markdown) => {
            if let Err(e) = save_cached_circulaire_ocr(circ_dir, id, &markdown) {
                tracing::warn!(id = %id, error = %e, "circulaire ocr cache write");
            }
            Some(markdown)
        }
        Err(e) => {
            tracing::warn!(id = %id, error = %e, "circulaire: repli OCR scanné échoué");
            None
        }
    }
}

/// Ingère un tarball du fond (stock `MM/cir_N.xml` ou flux `xml/YYYY/MM/…`) :
/// chaque membre XML → upsert `legal_text`. Renvoie `(upsertés, erreurs)`.
async fn ingest_circulaires_tarball(
    repo: &DecisionRepository<'_>,
    path: &Path,
) -> Result<(usize, usize)> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<std::result::Result<Box<Circulaire>, ()>>(256);
    let tar_path = path.to_path_buf();
    let reader = tokio::task::spawn_blocking(move || -> Result<()> {
        lj_sources::tar_reader::for_each_member(&tar_path, |name, raw| {
            if !lj_sources::circulaires::is_circulaire_xml_member(&name) {
                return Ok(());
            }
            let msg = match lj_sources::circulaires::parse_circulaire_xml(&raw) {
                Ok(c) => Ok(Box::new(c)),
                Err(e) => {
                    tracing::error!(member = %name, error = %e, "circulaire: parse échec");
                    Err(())
                }
            };
            tx.blocking_send(msg)
                .map_err(|_| anyhow!("canal circulaires fermé (consumer arrêté)"))
        })
    });

    let (mut upserted, mut errors) = (0usize, 0usize);
    while let Some(msg) = rx.recv().await {
        let c = match msg {
            Ok(c) => c,
            Err(()) => {
                errors += 1;
                continue;
            }
        };
        let row = circulaire_row(*c);
        repo.upsert_legal_text(&row)
            .await
            .map_err(|e| anyhow!("upsert_legal_text {}: {e}", row.text_uid))?;
        upserted += 1;
    }
    reader
        .await
        .map_err(|e| anyhow!("tâche lecture circulaires {}: {e}", path.display()))??;
    tracing::info!(source = %path.display(), upserted, errors, "ingest_circulaires_tarball");
    Ok((upserted, errors))
}

/// Applique `abroge_txt.tar` : chaque membre `.txt` liste des documents à
/// passer `ABROGE` (les `Supprime` — « retirés de la diffusion » — sont
/// traités pareil : on garde la fiche avec son état, jamais de trou dans le
/// graphe de citations). Renvoie le nombre de textes basculés.
async fn apply_abroge_tar(repo: &DecisionRepository<'_>, path: &Path) -> Result<u64> {
    let tar_path = path.to_path_buf();
    let ids = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
        let mut ids = Vec::new();
        lj_sources::tar_reader::for_each_member(&tar_path, |name, raw| {
            if name.ends_with(".txt") {
                ids.extend(lj_sources::circulaires::parse_abroge_list(&raw));
            }
            Ok(())
        })?;
        ids.sort();
        ids.dedup();
        Ok(ids)
    })
    .await
    .map_err(|e| anyhow!("tâche abroge circulaires: {e}"))??;

    let n = repo
        .set_legal_texts_status("CIRCULAIRE", &ids, "ABROGE")
        .await
        .map_err(|e| anyhow!("set_legal_texts_status: {e}"))?;
    tracing::info!(
        listed = ids.len(),
        flipped = n,
        "circulaires abroge historique"
    );
    Ok(n)
}

/// Mapping métadonnées → `legal_text` (ADR 0196 §7). Dates ISO tolérantes :
/// le fond porte des valeurs aberrantes réelles (`1190-…`) — `NaiveDate` les
/// accepte ; un format illisible devient `None` (warn), jamais un abort du
/// fond entier.
fn circulaire_row(c: Circulaire) -> lj_store::repository::LegalTextRow {
    let date = |s: Option<&str>| -> Option<chrono::NaiveDate> {
        let s = s?;
        match chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            Ok(d) => Some(d),
            Err(e) => {
                tracing::warn!(id = %c.id, value = s, error = %e, "circulaire: date illisible");
                None
            }
        }
    };
    let date_texte = date(c.date_signature.as_deref());
    let date_publi = date(c.date_publi.as_deref());
    let title_key = lj_extract::extract::normalize_instrument(&c.titre);
    let status = match c.etat.as_str() {
        "V" => "VIGUEUR",
        "A" => "ABROGE",
        other => {
            tracing::warn!(id = %c.id, etat = other, "circulaire: état inconnu, gardé brut");
            other
        }
    }
    .to_string();
    lj_store::repository::LegalTextRow {
        text_uid: c.id,
        jurisdiction: "FR".to_string(),
        title: c.titre,
        title_key,
        nature: "CIRCULAIRE".to_string(),
        last_modified: date_publi.or(date_texte),
        date_texte,
        date_publi,
        // Cascade d'identité ADR 0115 : le NOR (quand présent) collapse la
        // manifestation JORF éventuelle du même acte.
        eli: None,
        nor: c.nor,
        instrument_key: None,
        body: c.resume,
        status: Some(status),
    }
}

#[cfg(test)]
mod tests {
    use super::body_from_text_layer;

    #[test]
    fn couche_texte_native_devient_corps() {
        let page = "Le ministre de l'intérieur informe les préfets des modalités ".repeat(4);
        let brut = format!("{page}\x0c{page}");
        let corps = body_from_text_layer(&brut).expect("couche native");
        assert!(corps.contains("\n\n"), "saut de page → paragraphe");
        assert!(!corps.contains('\x0c'));
    }

    #[test]
    fn scan_sous_le_plancher_par_page_bascule_ocr() {
        // Page de garde native + 9 pages muettes : la moyenne par page doit
        // classer scanné, pas la seule présence de texte.
        let garde = "République française — Ministère de la justice ".repeat(20);
        let brut = format!("{garde}{}", "\x0c \x0c \x0c \x0c \x0c \x0c \x0c \x0c \x0c");
        assert_eq!(body_from_text_layer(&brut), None);
    }
}
