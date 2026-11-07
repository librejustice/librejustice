//! Source scrapée non-bulk : CNDA (asile, fiche HTML + PDF Word).

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use lj_bucket::key;
use std::collections::HashSet;

use lj_parse::{clean_ocr_markdown, parse_cnda, reflow_cnda_pdf_text, CndaParsed};
use lj_store::repository::ExtractedFields;

use crate::config::Settings;
use crate::ocr::Ocr;

use super::batch::drain_batch;
use super::europe::europe_pool;
use super::{
    content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode, BATCH_SIZE,
};

/// Borne haute du crawl de pagination CNDA : au-delà de ce nombre de pages
/// consécutives sans aucune fiche, on s'arrête. Garde-fou contre une boucle
/// infinie si le DOM change et rend toujours des liens.
const CNDA_MAX_PAGES: u32 = 200;

/// Construit le candidat CNDA depuis un [`CndaParsed`] : `source_fields`
/// préconstruits par le parser (`prebuilt_source_fields`), `payload_format` `pdf`
/// (décision avec PDF) / `html` (fiche-only). Le `content_checksum` porte sur les
/// octets source bruts (PDF, ou HTML de fiche si fiche-only).
///
/// `solution_uid` (best-effort sur le dispositif, connaissance CNDA que
/// l'extracteur générique n'a pas) déborde de `Decision` ; on le pose via
/// `prebuilt_extracted`, superposé à l'extraction routée complète au prepare
/// (`merge_prebuilt` : le préconstruit prime, l'extraction fournit le reste).
pub(super) fn cnda_candidate(
    parsed: CndaParsed,
    raw_source: &[u8],
    payload_format: &str,
) -> Candidate {
    let extracted = ExtractedFields {
        // `parse_cnda` normalise déjà la date FR (`3 avril 2025`) en ISO
        // `YYYY-MM-DD` sur `Decision.date_lecture`/`date_audience` ; on les parse ici
        // en `NaiveDate` pour la colonne. Le texte FR libre reste en `source_fields`
        // (`lecture_date`/`audience_date`).
        date_lecture: parsed
            .decision
            .date_lecture
            .as_deref()
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()),
        date_audience: parsed
            .decision
            .date_audience
            .as_deref()
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()),
        docket_numbers: parsed.decision.numero_dossiers.clone().unwrap_or_default(),
        publication_codes: parsed.decision.publication_codes.clone(),
        solution_uid: parsed.solution_uid.clone(),
        ..Default::default()
    };
    let extracted = lj_extract_rows::with_facet_uids(extracted, Some("CNDA"));
    Candidate {
        decision_id: None,
        public_id: generate_public_id(),
        content_checksum: content_checksum(raw_source),
        prebuilt_source_fields: Some(parsed.source_fields),
        prebuilt_extracted: Some(extracted),
        decision: parsed.decision,
        raw_payload: Vec::new(),
        payload_format: payload_format.to_string(),
        write_mode: super::WriteMode::Full,
        passages: true,
        distinct: false,
    }
}

/// Plancher de caractères non-blancs au-dessus duquel un PDF est réputé
/// natif-texte (couche texte exploitable par `pdftotext`). Un PDF scanné rend
/// quasi rien ; une vraie décision CNDA dépasse largement.
const PDF_TEXT_LAYER_FLOOR: usize = 600;

/// Vrai si l'extraction `pdftotext` a rendu une couche texte exploitable (≠ PDF
/// scanné/image). Compte les caractères non-blancs.
pub(super) fn pdf_has_text_layer(extracted: &str) -> bool {
    extracted.chars().filter(|c| !c.is_whitespace()).count() >= PDF_TEXT_LAYER_FLOOR
}

async fn scanned_ocr_fallback(
    client: Option<&Ocr>,
    cache_dir: &std::path::Path,
    numero: &str,
    pdf_bytes: &[u8],
) -> Option<String> {
    use lj_sources::cnda::{load_cached_ocr, save_cached_ocr};
    if let Ok(Some(md)) = load_cached_ocr(cache_dir, numero) {
        return Some(md);
    }
    let client = client?;
    match client.document(pdf_bytes, numero).await {
        Ok(markdown) => {
            if let Err(e) = save_cached_ocr(cache_dir, numero, &markdown) {
                tracing::warn!(numero = %numero, error = %e, "cnda ocr cache write");
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            Some(markdown)
        }
        Err(e) => {
            tracing::warn!(numero = %numero, error = %e, "cnda scanned OCR fallback failed");
            None
        }
    }
}

/// Crawl + ingest des décisions CNDA, calqué sur [`run_cedh`] : pagination de la
/// liste, puis par fiche parse HTML → PDF → extraction déterministe
/// (`pdftotext`, OCR en repli pour les scannés) → `parse_cnda` → `drain_batch`.
/// PDF inaccessible ⇒ fiche-only. Idempotent, manifeste en dernière page +
/// dernier numéro.
///
/// `start_page` vaut toujours 1 : la liste est antichrono. `stop_at` est le
/// watermark d'early-exit du sync incrémental — on s'arrête dès qu'on le
/// retombe, le reste étant plus ancien. `None` balaye jusqu'à la première page
/// sans fiche, borné par [`CNDA_MAX_PAGES`].
///
/// [`run_cedh`]: super::europe
async fn run_cnda(
    start_page: u32,
    mode: IngestMode,
    stop_at: Option<String>,
    only: Option<HashSet<String>>,
) -> Result<()> {
    use lj_sources::cnda::{
        enumerate_fiche_urls, manifest_path, numero_from_slug, parse_fiche, CndaClient,
        CndaManifest, THROTTLE,
    };

    let settings = Settings::from_env()?;
    let bucket = settings.bucket()?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;

    let client = CndaClient::new();
    let ocr_client = Ocr::from_pool(&lj_store::repository::DecisionRepository::new(&conn)).await?;
    let cache_dir = settings.cache_dir();
    let manifest_path = manifest_path(&cache_dir);
    let mut manifest = CndaManifest::load(&manifest_path).map_err(|e| anyhow!("manifest: {e}"))?;

    let mut total = IngestCounts::default();
    let mut fiche_only = 0usize;
    let mut last_page = start_page.saturating_sub(1);
    // Mode ciblé (`--only`) : on ne traite/persiste que ces numéros, on crawle la
    // liste jusqu'à les avoir tous trouvés (le numéro n'étant pas dans le slug de
    // liste, il faut fetcher chaque fiche pour le connaître), puis on s'arrête.
    // Le watermark n'est jamais touché.
    let mut remaining = only;
    // Sync incrémental : la première décision croisée (tête de liste antichrono)
    // devient le nouveau watermark ; on s'arrête dès qu'on retombe sur `stop_at`.
    let mut newest: Option<String> = None;
    let mut reached_watermark = false;
    for page in start_page..start_page.saturating_add(CNDA_MAX_PAGES) {
        let Some(list_html) = client
            .list_page(page)
            .await
            .map_err(|e| anyhow!("cnda list page={page}: {e}"))?
        else {
            // 404 = pagination épuisée (page au-delà de la liste curée) : on s'arrête.
            break;
        };
        let fiche_urls = enumerate_fiche_urls(&list_html);
        if fiche_urls.is_empty() {
            // Liste vide (cas dégénéré) : pagination épuisée également.
            break;
        }

        let mut counts = IngestCounts::default();
        let mut candidates: Vec<Candidate> = Vec::new();
        let mut last_numero: Option<String> = None;

        for fiche_url in &fiche_urls {
            tokio::time::sleep(THROTTLE).await;
            let fiche_html = client
                .fiche_html(fiche_url)
                .await
                .map_err(|e| anyhow!("cnda fiche {fiche_url}: {e}"))?;
            let fiche = parse_fiche(&fiche_html, fiche_url)
                .map_err(|e| anyhow!("parse_fiche {fiche_url}: {e}"))?;

            // Numéro = clé robuste (≥ 6 chiffres), par fiabilité : slug PDF
            // `…-n-NNNNNNNN-c`, sinon slug de la fiche (même format, ex.
            // `cnda-6-mai-2015-…-n-15001156-c`), sinon le corps de la fiche
            // (`n°NNNNNNNN`, les fiches modernes ayant un slug descriptif et un lien
            // `/documents/…` sans numéro). Skip non fatal seulement si aucune source ne le
            // donne. Calculé avant le fetch PDF : c'est la clé du cache local.
            let Some(numero) = fiche
                .pdf_url
                .as_deref()
                .and_then(numero_from_slug)
                .or_else(|| numero_from_slug(fiche_url))
                .or_else(|| fiche.numero.clone())
            else {
                tracing::warn!(fiche = %fiche_url, "cnda record without a readable decision number, skip");
                counts.skipped += 1;
                continue;
            };

            // Mode ciblé : tout numéro hors liste `--only` est ignoré en silence, avant
            // le fetch PDF/OCR coûteux.
            if remaining.as_ref().is_some_and(|r| !r.contains(&numero)) {
                continue;
            }

            // Early-exit du sync : la liste est antichrono. Dès qu'on retombe sur la
            // décision la plus récente du run précédent, tout ce qui suit est plus ancien
            // donc déjà ingéré — on arrête le crawl, avant le fetch PDF/OCR.
            if newest.is_none() {
                newest = Some(numero.clone());
            }
            if stop_at.as_deref() == Some(numero.as_str()) {
                reached_watermark = true;
                break;
            }

            // The linked file is read from the bucket first, so that a re-run or a
            // re-extraction does not crawl cnda.fr again. A dead link (`None`) leaves
            // the editorial abstract alone.
            let pdf_bytes = match fiche.pdf_url.as_deref() {
                Some(slug) => match bucket.get(&key::cnda(&numero)).await? {
                    Some(stored) => Some(stored.to_vec()),
                    None => {
                        tokio::time::sleep(THROTTLE).await;
                        let fetched = client
                            .fetch_pdf(slug)
                            .await
                            .map_err(|e| anyhow!("cnda pdf {slug}: {e}"))?;
                        if let Some(bytes) = &fetched {
                            bucket.put(&key::cnda(&numero), bytes.clone()).await?;
                        }
                        fetched
                    }
                },
                None => None,
            };

            let (pdf_text, payload_format, raw_source): (String, &str, Vec<u8>) = match &pdf_bytes {
                // Le payload « PDF » est parfois un DOCX Word (certaines décisions CNDA
                // sont publiées en .docx). Born-digital ⇒ texte extrait du
                // `word/document.xml` sans OCR, Mistral OCR rejetant le DOCX (400). Le
                // markdown DOCX passe par le même nettoyage que l'OCR (`clean_ocr_markdown` :
                // 1 paragraphe par ligne).
                Some(bytes) if lj_sources::docx::is_zip_container(bytes) => {
                    match lj_sources::docx::extract_docx_text(bytes) {
                        Ok(text) => (clean_ocr_markdown(&text), "docx", bytes.clone()),
                        Err(e) => {
                            tracing::warn!(numero = %numero, fiche = %fiche_url, error = %e,
                                "cnda DOCX unreadable, decision skipped");
                            counts.skipped += 1;
                            continue;
                        }
                    }
                }
                Some(bytes) => {
                    // Natif-texte : recollage déterministe, aucun appel réseau. Un PDF sans
                    // couche texte (scanné) OU que `pdftotext` n'arrive pas à lire (corrompu,
                    // chiffré, code ≠ 0) bascule au même repli : OCR cache-first, puis skip si
                    // indisponible. Pas de `?` ici : une décision illisible n'avorte pas le run,
                    // et un skip préserve le `full_text` en base.
                    let native = lj_sources::pdf::pdftotext_extract(bytes)
                        .ok()
                        .filter(|raw| pdf_has_text_layer(raw));
                    match native {
                        Some(raw) => (reflow_cnda_pdf_text(&raw), "pdf", bytes.clone()),
                        None => match scanned_ocr_fallback(
                            ocr_client.as_ref(),
                            &cache_dir,
                            &numero,
                            bytes,
                        )
                        .await
                        {
                            Some(markdown) => (clean_ocr_markdown(&markdown), "pdf", bytes.clone()),
                            None => {
                                tracing::warn!(
                                    numero = %numero,
                                    fiche = %fiche_url,
                                    "cnda PDF scanned/unreadable with no OCR available, decision skipped (retry later)"
                                );
                                counts.skipped += 1;
                                continue;
                            }
                        },
                    }
                }
                // Fiche-only : pas de PDF lié ; le checksum porte sur l'HTML de fiche
                // (payload source brut).
                None => {
                    fiche_only += 1;
                    (String::new(), "html", fiche_html.into_bytes())
                }
            };

            // Échec de parse non fatal : une décision qu'on ne peut ni lire (PDF
            // illisible → fiche-only sans texte) ni identifier (fiche-only sans date de
            // lecture → ECLI infabricable) est tracée et skippée, pas une cause d'abandon
            // du bootstrap. Un bug de parser systématique ressortirait comme un afflux de
            // warns + le récap skipped final.
            let parsed = match parse_cnda(&pdf_text, &fiche.to_value(), &numero) {
                Ok(parsed) => parsed,
                Err(e) => {
                    tracing::warn!(
                        numero = %numero,
                        fiche = %fiche_url,
                        error = %e,
                        "cnda parse impossible, skip (targeted re-fetch later)"
                    );
                    counts.skipped += 1;
                    continue;
                }
            };
            if let Some(rem) = remaining.as_mut() {
                rem.remove(&numero);
            }
            last_numero = Some(numero);
            candidates.push(cnda_candidate(parsed, &raw_source, payload_format));

            // Mode ciblé : toutes les cibles traitées → on draine puis on sort.
            if remaining.as_ref().is_some_and(|r| r.is_empty()) {
                break;
            }

            if candidates.len() >= BATCH_SIZE {
                let batch = std::mem::take(&mut candidates);
                drain_batch(&conn, batch, mode, &mut counts).await?;
            }
        }
        if !candidates.is_empty() {
            drain_batch(&conn, candidates, mode, &mut counts).await?;
        }

        last_page = page;
        // Bootstrap (`stop_at == None`) : watermark = page + dernier numéro drainé,
        // point de reprise d'un crawl interrompu. En sync, le watermark (numéro le
        // plus récent) est posé une fois en fin de run, hors de la boucle. En mode
        // ciblé (`remaining.is_some()`), jamais.
        if stop_at.is_none() && remaining.is_none() {
            if let Some(numero) = last_numero {
                manifest.mark(page, numero);
                manifest
                    .save(&manifest_path)
                    .map_err(|e| anyhow!("manifest save: {e}"))?;
            }
        }
        tracing::info!(
            page,
            fiches = fiche_urls.len(),
            created = counts.created,
            updated = counts.updated,
            skipped = counts.skipped,
            "ingest_cnda_page"
        );
        total.merge(&counts);
        if reached_watermark {
            break;
        }
        // Mode ciblé : toutes les cibles trouvées → fin du crawl.
        if remaining.as_ref().is_some_and(|r| r.is_empty()) {
            break;
        }
    }

    // Mode ciblé : signale les numéros demandés jamais croisés dans la liste
    // (retirés du corpus, ou numéro erroné).
    if let Some(rem) = &remaining {
        if !rem.is_empty() {
            tracing::warn!(introuvables = ?rem, "cnda --only: numbers not found in the list");
        }
    }

    // Sync : pose le nouveau watermark = décision la plus récente vue ce run.
    if stop_at.is_some() {
        if let Some(numero) = &newest {
            manifest.mark_newest(numero);
            manifest
                .save(&manifest_path)
                .map_err(|e| anyhow!("manifest save: {e}"))?;
        }
    }

    tracing::info!(
        start_page,
        last_page,
        created = total.created,
        updated = total.updated,
        skipped = total.skipped,
        fiche_only,
        "ingest_cnda_total"
    );
    Ok(())
}

/// Bootstrap CNDA : balaye la liste jurisprudentielle depuis la première page.
/// Idempotent (`source_uid`/`content_checksum`) — re-jouable sans dupliquer (le
/// manifeste n'évite que le re-fetch réseau).
pub async fn ingest_cnda(mode: IngestMode, only: Vec<String>) -> Result<()> {
    if only.is_empty() {
        return run_cnda(1, mode, None, None).await;
    }
    // Ciblé : ré-extraction forcée (`All`) — les numéros visés ont par définition
    // un PDF/texte qu'on veut re-traiter, même à `content_checksum` inchangé.
    run_cnda(1, IngestMode::All, None, Some(only.into_iter().collect())).await
}

/// Sync incrémental CNDA : crawl antichrono depuis la page 1 (les nouvelles
/// décisions arrivent en tête de la liste curée), avec early-exit dès qu'on
/// retombe sur la décision la plus récente du run précédent (`last_numero`). On
/// ne paie l'OCR que pour le vrai incrément. Watermark absent (1ᵉʳ run après
/// bascule) ⇒ balaye toute la liste une fois — idempotent (cache PDF/OCR +
/// `content_checksum`) — puis se cale sur la tête.
pub async fn sync_cnda() -> Result<()> {
    use lj_sources::cnda::{manifest_path, CndaManifest};
    let settings = Settings::from_env()?;
    let manifest = CndaManifest::load(&manifest_path(&settings.cache_dir()))
        .map_err(|e| anyhow!("manifest: {e}"))?;
    run_cnda(1, IngestMode::MissingHash, manifest.last_numero, None).await
}
