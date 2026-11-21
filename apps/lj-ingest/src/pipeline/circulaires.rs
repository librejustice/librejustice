//! Circulaires (fond DILA CIRCULAIRES) : sync du fond complet
//! (stocks 2009-2014 + abrogations historiques + flux ≥ 2014) → `legal_text`
//! nature=`CIRCULAIRE`, identité `cir_<ID>`, NOR en cascade, état
//! V/A en `status`, DILA's summary as the provisional `body` piece.
//!
//! Bodies ([`sync_circulaires_bodies`]): a separate pass replays the PDF stocks
//! (`pdf/<année>_pdf.tar.gz`), then the feeds' companion PDFs, through
//! deterministic `pdftotext` with cache-first Mistral OCR for scans, and writes
//! the extracted text as the text's `body` piece. The metadata sync writes DILA's
//! capped summary as a provisional piece, which the body pass replaces when a
//! usable PDF exists.
//!
//! Rejeu : stocks (par année) → `abroge_txt.tar` (flips d'état historiques)
//! → flux en ordre chronologique (last-write-wins par `ID_CIRCULAIRE` —
//! une abrogation moderne arrive en re-export `ETAT=A`). Chaque fichier
//! ingéré est marqué au manifest (`mark_circulaire_done`) : un crash reprend
//! où il s'était arrêté. Idempotent : upsert par `text_uid`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{anyhow, Result};

use lj_bucket::{key, Bucket};
use lj_sources::circulaires::{CircKind, Circulaire};
use lj_store::repository::{BodyWrite, DecisionRepository};
use serde_json::Value;

use crate::config::Settings;
use crate::ocr::{self, Ocr};

/// Sync du fond CIRCULAIRES : planifie (listing + téléchargements), ingère
/// chaque fichier nouveau dans l'ordre de rejeu, marque le manifest, puis pose
/// les slugs des textes nouveaux.
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
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
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
        let (dir, name) = (dir.clone(), tarball.name.clone());
        tokio::task::spawn_blocking(move || {
            lj_sources::circulaires::mark_circulaire_done(&dir, &name)
        })
        .await
        .map_err(|e| anyhow!("tâche manifest circulaires: {e}"))?
        .map_err(|e| anyhow!("mark_circulaire_done {}: {e}", tarball.name))?;
    }

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

/// Passe corps du fond CIRCULAIRES : rejoue les tarballs PDF (stocks puis
/// flux, last-write-wins par id), extrait chaque PDF (`pdftotext` natif, OCR
/// Mistral cache-first en repli scanné) et pose la pièce `body` du texte. Un repli OCR
/// qui manque (clé absente/épuisée) dépose le PDF en file d'attente
/// (`ocr-pending/`), drainée en tête de chaque run : le retry coûte
/// O(fichiers manqués), jamais un re-stream de tarball — le tarball est donc
/// marqué au manifest inconditionnellement.
pub async fn sync_circulaires_bodies(ocr_tenant: &str) -> Result<()> {
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

    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let ocr_client = Ocr::tenants(&repo, ocr_tenant).await?;
    let scans = Scans {
        ocr: ocr_client.as_ref(),
        bucket: settings.bucket()?,
        dir: dir.clone(),
    };

    let mut total = PdfCounts::default();
    drain_pending_ocr(&repo, &scans, &mut total).await?;
    for tarball in &plan {
        let counts = ingest_circulaire_pdfs(&repo, &scans, &tarball.path).await?;
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
        gaps = total.gaps,
        "sync_circulaires_bodies"
    );
    Ok(())
}

/// Rewrites the body of every circulaire whose OCR answer the bucket keeps, from that answer.
pub async fn rebuild_circulaires_ocr() -> Result<()> {
    let settings = Settings::from_env()?;
    let bucket = settings.bucket()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let answers: Vec<String> = bucket
        .keys("circulaires/")
        .await?
        .into_iter()
        .filter(|k| !k.contains(".ocr/"))
        .collect();
    let mut written = 0usize;
    for answer in &answers {
        let id = answer
            .strip_prefix("circulaires/")
            .and_then(|k| k.strip_suffix(".ocr.json.zst"))
            .ok_or_else(|| anyhow!("{answer}: not a circulaire's OCR answer"))?;
        let pages: Vec<Value> = ocr::kept(&bucket, answer)
            .await?
            .ok_or_else(|| anyhow!("{answer}: gone from the bucket"))?;
        let body = lj_parse::clean_ocr_markdown(&ocr::text(&pages));
        written += repo
            .write_text_bodies(SOURCE, &[(id.to_string(), body)], BodyWrite::Replace)
            .await
            .map_err(|e| anyhow!("write_text_bodies {id}: {e}"))?
            .len();
    }
    tracing::info!(
        answers = answers.len(),
        written,
        "circulaires: bodies rebuilt from the OCR answers"
    );
    Ok(())
}

/// Empties the pages `list` names, `<id> <page>` a line, in the circulaires' kept answers and
/// rewrites their bodies; their PDFs stay at DILA, so the list was drawn beforehand.
pub async fn blank_circulaires_ocr(list: &Path, dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let bucket = settings.bucket()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let mut listed: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    for line in std::fs::read_to_string(list)?.lines() {
        let mut fields = line.split_whitespace();
        let (Some(id), Some(page), None) = (fields.next(), fields.next(), fields.next()) else {
            anyhow::bail!("{}: not `<id> <page>`: {line:?}", list.display());
        };
        listed
            .entry(id.to_string())
            .or_default()
            .insert(page.parse()?);
    }
    let mut tally = ocr::Emptied::default();
    for (id, numbers) in &listed {
        let answer = key::circulaire_ocr(id);
        let mut kept = ocr::kept_answer(&bucket, &answer).await?;
        tally.docs_read += 1;
        for n in numbers {
            let page = kept
                .0
                .get_mut(n)
                .ok_or_else(|| anyhow!("{answer}: page {n} not kept"))?;
            tally.empty(&ocr::markdown(page));
            *page = ocr::emptied(page);
        }
        tally.docs += 1;
        if dry_run {
            continue;
        }
        ocr::keep_answer(&bucket, &answer, &kept, numbers).await?;
        let pages: Vec<Value> = kept.0.into_values().collect();
        let body = lj_parse::clean_ocr_markdown(&ocr::text(&pages));
        repo.write_text_bodies(SOURCE, &[(id.clone(), body)], BodyWrite::Replace)
            .await
            .map_err(|e| anyhow!("write_text_bodies {id}: {e}"))?;
    }
    tally.log("circulaires", dry_run);
    Ok(())
}

/// Draine la file d'attente OCR (`ocr-pending/`) : les PDF scannés dont le
/// repli a manqué à un run précédent. Cache-first puis OCR live ; une entrée
/// résolue pose le corps et sort de la file, une entrée encore sans clé y
/// reste (re-comptée `ocr_missed`).
async fn drain_pending_ocr(
    repo: &DecisionRepository<'_>,
    scans: &Scans<'_>,
    total: &mut PdfCounts,
) -> Result<()> {
    let circ_dir = scans.dir.as_path();
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
    use futures::StreamExt;
    let read = futures::stream::iter(pending)
        .map(|(id, path)| async move {
            let bytes = tokio::fs::read(&path).await?;
            anyhow::Ok((scans.text(&id, &bytes).await, id))
        })
        .buffer_unordered(OCR_CONCURRENCY);
    let mut read = std::pin::pin!(read);
    while let Some(outcome) = read.next().await {
        let (text, id) = outcome?;
        let Some((md, refused)) = text else {
            total.ocr_missed += 1;
            continue;
        };
        total.ocr += 1;
        let body = lj_parse::clean_ocr_markdown(&md);
        if !repo
            .write_text_bodies(SOURCE, &[(id.clone(), body)], BodyWrite::Replace)
            .await
            .map_err(|e| anyhow!("write_text_bodies {id}: {e}"))?
            .is_empty()
        {
            total.updated += 1;
        } else {
            tracing::debug!(id = %id, "circulaire: orphan pdf (no legal_text row)");
            total.missing_row += 1;
        }
        if !refused.is_empty() {
            total.gaps += 1;
            continue;
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
    /// Scans written without the pages the OCR invented on twice, left in `ocr-pending`.
    gaps: usize,
}

impl PdfCounts {
    fn add(&mut self, o: &Self) {
        self.updated += o.updated;
        self.ocr += o.ocr;
        self.ocr_missed += o.ocr_missed;
        self.missing_row += o.missing_row;
        self.errors += o.errors;
        self.gaps += o.gaps;
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

/// Mistral OCR calls in flight at once. A doc key lives a fixed time from its first
/// use whatever the volume, so the window is saturated rather than left to
/// drain between two calls. [`Ocr`] paces the pages they send.
const OCR_CONCURRENCY: usize = 8;

/// `legal_article.source` of a circulaire's body.
const SOURCE: &str = "circulaire";

/// Un membre PDF résolu en corps (ou compté) par le pipeline concurrent.
enum PdfOutcome {
    /// Corps prêt à poser (`ocr` = passé par le repli OCR, cache compris).
    Body {
        id: String,
        body: String,
        ocr: bool,
        gaps: bool,
    },
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
    scans: &Scans<'_>,
    path: &Path,
) -> Result<PdfCounts> {
    use futures::StreamExt;

    let circ_dir = scans.dir.as_path();
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
                    tracing::warn!(member = %name, error = %e, "circulaire: unreadable pdf");
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
                gaps: false,
            },
            PdfMsg::Scanned(id, bytes) => match scans.text(&id, &bytes).await {
                Some((md, refused)) => {
                    if !refused.is_empty() {
                        if let Err(e) = lj_sources::circulaires::save_pending_circulaire_pdf(
                            circ_dir, &id, &bytes,
                        ) {
                            tracing::warn!(id = %id, error = %e, "circulaire: ocr-pending drop");
                        }
                    }
                    PdfOutcome::Body {
                        body: lj_parse::clean_ocr_markdown(&md),
                        id,
                        ocr: true,
                        gaps: !refused.is_empty(),
                    }
                }
                None => {
                    if let Err(e) =
                        lj_sources::circulaires::save_pending_circulaire_pdf(circ_dir, &id, &bytes)
                    {
                        tracing::warn!(id = %id, error = %e, "circulaire: ocr-pending drop");
                    }
                    PdfOutcome::OcrMissed
                }
            },
            PdfMsg::Unreadable => PdfOutcome::Unreadable,
        }
    })
    .buffer_unordered(OCR_CONCURRENCY);
    let mut outcomes = std::pin::pin!(outcomes);

    let mut counts = PdfCounts::default();
    while let Some(outcome) = outcomes.next().await {
        let (id, body) = match outcome {
            PdfOutcome::Body {
                id,
                body,
                ocr,
                gaps,
            } => {
                counts.ocr += usize::from(ocr);
                counts.gaps += usize::from(gaps);
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
        if !repo
            .write_text_bodies(SOURCE, &[(id.clone(), body)], BodyWrite::Replace)
            .await
            .map_err(|e| anyhow!("write_text_bodies {id}: {e}"))?
            .is_empty()
        {
            counts.updated += 1;
        } else {
            tracing::debug!(id = %id, "circulaire: orphan pdf (no legal_text row)");
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

/// Plancher moyen de caractères non-blancs par page au-dessus duquel la
/// couche texte `pdftotext` est réputée native. Un scan rend quasi rien ; la
/// moyenne par page évite qu'une page de garde native fasse passer un document
/// scanné pour du texte.
const PDF_TEXT_LAYER_FLOOR_PER_PAGE: usize = 100;

/// Corps depuis la couche texte `pdftotext` ; `None` = PDF réputé scanné.
pub(crate) fn body_from_text_layer(extracted: &str) -> Option<String> {
    let pages = extracted.matches('\x0c').count() + 1;
    let non_ws = extracted.chars().filter(|c| !c.is_whitespace()).count();
    if non_ws / pages < PDF_TEXT_LAYER_FLOOR_PER_PAGE {
        return None;
    }
    Some(extracted.replace('\x0c', "\n\n").trim().to_string())
}

/// The OCR fallback of a scanned PDF, and where its reading is kept.
struct Scans<'a> {
    ocr: Option<&'a Ocr>,
    bucket: Bucket,
    dir: std::path::PathBuf,
}

impl Scans<'_> {
    /// The markdown of a scanned PDF: from Mistral's answer the bucket keeps, else from the
    /// markdown cached before the answer was kept (`<circ_dir>/ocr/<id>.md`), else read by the
    /// OCR, its answer kept, with the pages it invented on twice. `None` when none of the three
    /// reads it: the caller counts an `ocr_missed` and queues the PDF.
    async fn text(&self, id: &str, pdf_bytes: &[u8]) -> Option<(String, Vec<u32>)> {
        let answer = key::circulaire_ocr(id);
        match ocr::kept::<Vec<Value>>(&self.bucket, &answer).await {
            Ok(Some(pages)) => return Some((ocr::text(&pages), Vec::new())),
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(id = %id, error = %e, "circulaire: kept OCR answer unread");
                return None;
            }
        }
        if let Ok(Some(md)) = lj_sources::circulaires::load_cached_circulaire_ocr(&self.dir, id) {
            return Some((md, Vec::new()));
        }
        match self
            .ocr?
            .document_kept(&self.bucket, &answer, pdf_bytes, id)
            .await
        {
            Ok(reading) => Some((reading.text(), reading.refused)),
            Err(e) => {
                tracing::warn!(id = %id, error = %e, "circulaire: scanned OCR fallback failed");
                None
            }
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
                    tracing::error!(member = %name, error = %e, "circulaire: parse failed");
                    Err(())
                }
            };
            tx.blocking_send(msg)
                .map_err(|_| anyhow!("canal circulaires fermé (consumer arrêté)"))
        })
    });

    let (mut rewritten, mut errors) = (Vec::new(), 0usize);
    while let Some(msg) = rx.recv().await {
        let c = match msg {
            Ok(c) => c,
            Err(()) => {
                errors += 1;
                continue;
            }
        };
        let (row, resume) = circulaire_row(*c);
        repo.upsert_legal_text(&row)
            .await
            .map_err(|e| anyhow!("upsert_legal_text {}: {e}", row.text_uid))?;
        if let Some(resume) = resume {
            repo.write_text_bodies(
                SOURCE,
                &[(row.text_uid.clone(), resume)],
                BodyWrite::IfAbsent,
            )
            .await
            .map_err(|e| anyhow!("write_text_bodies {}: {e}", row.text_uid))?;
        }
        rewritten.push(row.text_uid);
    }
    reader
        .await
        .map_err(|e| anyhow!("tâche lecture circulaires {}: {e}", path.display()))??;
    let upserted = rewritten.len();
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

/// The metadata as a `legal_text` row, and DILA's summary, the provisional body.
/// Dates are read leniently: the fond carries real aberrant values (`1190-…`),
/// which `NaiveDate` accepts; an unreadable format becomes `None` with a warning,
/// never an abort of the whole fond.
fn circulaire_row(c: Circulaire) -> (lj_store::repository::LegalTextRow, Option<String>) {
    let date = |s: Option<&str>| -> Option<chrono::NaiveDate> {
        let s = s?;
        match chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            Ok(d) => Some(d),
            Err(e) => {
                tracing::warn!(id = %c.id, value = s, error = %e, "circulaire: unreadable date");
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
            tracing::warn!(id = %c.id, etat = other, "circulaire: unknown state, kept raw");
            other
        }
    }
    .to_string();
    let row = lj_store::repository::LegalTextRow {
        text_uid: c.id,
        jurisdiction: "FR".to_string(),
        title: c.titre,
        title_key,
        nature: "CIRCULAIRE".to_string(),
        last_modified: date_publi.or(date_texte),
        date_texte,
        date_publi,
        // Cascade d'identité : le NOR (quand présent) collapse la manifestation
        // JORF éventuelle du même acte.
        eli: None,
        nor: c.nor,
        instrument_key: None,
        status: Some(status),
    };
    (row, c.resume)
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
        let garde = "République française — Ministère de la justice ".repeat(20);
        let brut = format!("{garde}{}", "\x0c \x0c \x0c \x0c \x0c \x0c \x0c \x0c \x0c");
        assert_eq!(body_from_text_layer(&brut), None);
    }
}
