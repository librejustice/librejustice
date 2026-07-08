//! Pipelines opendata (XML) et Judilibre (JSON) + refetch ciblé.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use lj_llm::backend::Embedder;
use lj_sources::downloader::Manifest;
use lj_sources::judilibre::JudilibreClient;
use lj_store::db::Connection;
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

use super::batch::{drain_batch, drain_batch_in_txn};
use super::embed::{build_embedder, build_vllm_strict};
use super::files::{collect_jsonl_gz, collect_zip_paths, read_jsonl_gz_lines};
use super::prepare::{classify_judilibre, classify_xml};
use super::{Candidate, IngestCounts, IngestMode, BATCH_SIZE};

/// Ingère les archives opendata d'un dossier (XML → DB).
///
/// Port de `pipelines/ingest.py` + `cli._ingest_opendata_conseil_etat` : itère
/// les `*.zip` sous `data_dir`, parse chaque XML, triage idempotent par batch,
/// chunk + extract + (embed) + upsert. `data_dir` est le dossier des ZIPs
/// (`<cache>/opendata_conseil_etat/zips/...` ou un chemin explicite).
pub async fn ingest_opendata(
    data_dir: &Path,
    with_embeddings: bool,
    mode: IngestMode,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;

    let embedder = if with_embeddings {
        Some(build_embedder(&settings).await?)
    } else {
        None
    };

    let zip_paths = collect_zip_paths(data_dir)?;
    if zip_paths.is_empty() {
        tracing::info!(dir = %data_dir.display(), "aucun ZIP opendata trouvé");
        return Ok(());
    }

    // Manifeste downloader : fast-skip des ZIPs déjà entièrement ingérés (port
    // de `cli._ingest_opendata_conseil_etat`). Sans manifeste (chemin explicite
    // hors `<cache>/opendata_conseil_etat`), aucun skip — full scan.
    let manifest_path = data_dir.join("manifest.json");
    let mut manifest = Manifest::load(&manifest_path).map_err(|e| anyhow!("manifest: {e}"))?;
    let key_by_path: HashMap<PathBuf, String> = manifest
        .entries
        .iter()
        .map(|(key, entry)| (data_dir.join(&entry.path), key.clone()))
        .collect();

    let mut total = IngestCounts::default();
    let mut manifest_skipped = 0usize;
    for zip_path in &zip_paths {
        let manifest_key = key_by_path.get(zip_path);

        // Fast skip : ZIP déjà entièrement ingéré selon le manifeste (taille
        // identique au download). Évite d'ouvrir le ZIP et de hasher les membres.
        // `fully_ingested` est invalidé par le downloader à chaque re-download.
        // Mode ALL : on bypasse le fast-skip (re-traitement total forcé).
        if let Some(entry) = manifest_key
            .filter(|_| mode != IngestMode::All)
            .and_then(|k| manifest.entries.get(k))
        {
            if entry.fully_ingested
                && (!with_embeddings || entry.embeddings_complete)
                && entry.size.is_some()
                && entry.size == Some(zip_path.metadata()?.len())
            {
                manifest_skipped += 1;
                tracing::info!(source = %zip_path.display(), "ingest_zip_manifest_skip");
                continue;
            }
        }

        let zip_counts = drain_zip(
            &conn,
            embedder.as_ref(),
            zip_path,
            None,
            with_embeddings,
            mode,
        )
        .await?;

        tracing::info!(
            source = %zip_path.display(),
            created = zip_counts.created,
            updated = zip_counts.updated,
            skipped = zip_counts.skipped,
            errors = zip_counts.errors,
            chunks = zip_counts.chunks_created,
            "ingest_zip"
        );
        total.merge(&zip_counts);

        // Marque le ZIP entièrement ingéré dans le manifeste — écrit
        // immédiatement pour survivre à un arrêt en cours de run.
        if zip_counts.errors == 0 {
            if let Some(entry) = manifest_key.and_then(|k| manifest.entries.get_mut(k)) {
                entry.fully_ingested = true;
                if with_embeddings {
                    entry.embeddings_complete = true;
                }
                manifest
                    .save(&manifest_path)
                    .map_err(|e| anyhow!("manifest save: {e}"))?;
            }
        }
    }

    tracing::info!(
        zips = zip_paths.len(),
        manifest_skipped,
        created = total.created,
        updated = total.updated,
        skipped = total.skipped,
        errors = total.errors,
        chunks = total.chunks_created,
        "ingest_total"
    );
    Ok(())
}

/// Lit un ZIP opendata, classe chaque membre XML, filtre éventuellement par
/// `only` (set de `source_uid` cibles), draine par batch. Cœur partagé par
/// [`ingest_opendata`] (sans filtre) et [`reingest_stale_opendata`] (ciblé).
/// `only = Some(set)` ne retient que les candidats dont le `source_uid`
/// (`{zip}/{member}`) est dans `set` — les autres membres du ZIP sont parsés puis
/// jetés (aucun chunk/embed).
async fn drain_zip<E: Embedder>(
    conn: &Connection,
    embedder: Option<&E>,
    zip_path: &Path,
    only: Option<&std::collections::HashSet<String>>,
    with_embeddings: bool,
    mode: IngestMode,
) -> Result<IngestCounts> {
    let archive_name = zip_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();

    let members = lj_sources::zip_reader::iter_decisions(zip_path)
        .map_err(|e| anyhow!("lecture {}: {e}", zip_path.display()))?;

    let mut candidates: Vec<Candidate> = Vec::new();
    let mut zip_counts = IngestCounts::default();

    for (member, raw) in members {
        if raw.is_empty() {
            zip_counts.skipped += 1;
            zip_counts.empty_skipped += 1;
            continue;
        }
        match classify_xml(raw, &member, &archive_name) {
            Some(cand) => {
                if only.is_none_or(|s| s.contains(&cand.decision.source_uid)) {
                    candidates.push(cand);
                }
            }
            None => zip_counts.errors += 1,
        }
        if candidates.len() >= BATCH_SIZE {
            let batch = std::mem::take(&mut candidates);
            drain_batch(
                conn,
                embedder,
                batch,
                with_embeddings,
                mode,
                &mut zip_counts,
            )
            .await?;
        }
    }
    if !candidates.is_empty() {
        drain_batch(
            conn,
            embedder,
            candidates,
            with_embeddings,
            mode,
            &mut zip_counts,
        )
        .await?;
    }
    Ok(zip_counts)
}

/// Re-ingest **ciblé** des décisions opendata dont l'autorité a basculé sur
/// opendata (rang 55 généré, ADR 0109) mais dont le `full_text` reste figé sur la
/// provenance rang 50 (jade/constit/cedh/cjue/cnda) qui gagnait quand opendata
/// valait 40 — ~61,7k décisions CAA/CE. Re-parse leur payload opendata depuis les
/// ZIPs (seuls ceux contenant une cible sont ouverts ; le `source_uid` porte le
/// nom du ZIP) en [`IngestMode::All`] : re-chunk + re-embed (vLLM **strict**,
/// jamais Cloudflare — op de maintenance, même classe que le re-split #29) +
/// réécriture du `full_text` canonique sur le texte opendata.
pub async fn reingest_stale_opendata(data_dir: &Path) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;

    let targets: std::collections::HashSet<String> = DecisionRepository::new(&conn)
        .opendata_source_uids_stale_authority()
        .await?
        .into_iter()
        .collect();
    if targets.is_empty() {
        tracing::info!("reingest_stale_opendata : aucune cible (rien à flipper)");
        return Ok(());
    }

    // ZIPs contenant ≥1 cible : `source_uid` = `{zip}/{member}` → préfixe = nom du
    // ZIP. On n'ouvre que ceux-là (évite de parser tout le corpus opendata).
    let target_zips: std::collections::HashSet<&str> =
        targets.iter().filter_map(|u| u.split('/').next()).collect();
    let zip_paths: Vec<PathBuf> = collect_zip_paths(data_dir)?
        .into_iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| target_zips.contains(n))
        })
        .collect();

    // Re-embed maintenance : vLLM strict, erreur franche si injoignable (pas de
    // repli Cloudflare payant — règle projet).
    let embedder = build_vllm_strict(&settings).await?;

    tracing::info!(
        targets = targets.len(),
        zips = zip_paths.len(),
        "reingest_stale_opendata : début"
    );
    let mut total = IngestCounts::default();
    for zip_path in &zip_paths {
        let zip_counts = drain_zip(
            &conn,
            Some(&embedder),
            zip_path,
            Some(&targets),
            true,
            IngestMode::All,
        )
        .await?;
        tracing::info!(
            source = %zip_path.display(),
            updated = zip_counts.updated,
            errors = zip_counts.errors,
            chunks = zip_counts.chunks_created,
            "reingest_stale_opendata_zip"
        );
        total.merge(&zip_counts);
    }

    tracing::info!(
        zips = zip_paths.len(),
        created = total.created,
        updated = total.updated,
        errors = total.errors,
        chunks = total.chunks_created,
        "reingest_stale_opendata : terminé"
    );
    Ok(())
}

/// Ingère les archives Judilibre d'un dossier (JSON → DB).
///
/// Port de `pipelines/ingest_judilibre.py` + `cli._ingest_judilibre` : itère les
/// `*/*.jsonl.gz` sous `data_dir`, parse chaque ligne JSON, même triage/chunk/
/// embed/upsert que l'opendata.
pub async fn ingest_judilibre(
    data_dir: &Path,
    with_embeddings: bool,
    mode: IngestMode,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;

    // Propage les suppressions Judilibre (`tombstones.jsonl`) vers la base avant
    // tout ingest : une décision retirée par la Cour de cassation doit cesser
    // d'être servie même si le manifeste fast-skip court-circuite la boucle
    // d'ingest (ADR 0087).
    crate::tombstones::prune_tombstones(data_dir, &DecisionRepository::new(&conn)).await?;

    let embedder = if with_embeddings {
        Some(build_embedder(&settings).await?)
    } else {
        None
    };

    let files = collect_jsonl_gz(data_dir)?;
    if files.is_empty() {
        tracing::info!(dir = %data_dir.display(), "aucun fichier Judilibre trouvé");
        return Ok(());
    }

    // Manifeste downloader : fast-skip + resume incrémental (port de
    // `cli._ingest_judilibre`). Mapping fichier → MonthState : `<jur>/<key>.jsonl.gz`
    // (`key` = YYYYMM ou `archive`).
    let manifest_path = data_dir.join("manifest.json");
    let mut manifest = Manifest::load(&manifest_path).map_err(|e| anyhow!("manifest: {e}"))?;

    let mut total = IngestCounts::default();
    let mut manifest_skipped = 0usize;
    for path in &files {
        let jur = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let month_key = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .trim_end_matches(".jsonl.gz")
            .to_string();
        let file_size = path.metadata()?.len();
        // Mode ALL : on ignore le manifeste (ni fast-skip ni resume), re-scan total.
        let month_state = manifest
            .jurisdictions
            .get(&jur)
            .and_then(|j| j.months.get(&month_key))
            .filter(|_| mode != IngestMode::All);

        // Fast skip : `.jsonl.gz` non modifié (taille identique) et déjà ingéré.
        // Évite d'ouvrir et de hasher chaque ligne — gain massif sur des Go
        // déjà ingérés.
        if let Some(state) = month_state {
            if state.fully_ingested
                && (!with_embeddings || state.embeddings_complete)
                && state.ingested_size == Some(file_size)
            {
                manifest_skipped += 1;
                tracing::info!(source = %path.display(), "ingest_judilibre_manifest_skip");
                continue;
            }
        }

        // Resume incrémental : si on a déjà ingéré N lignes de ce fichier, on
        // saute les N premières avant tout parse/hash/triage (le `.jsonl.gz`
        // étant append-only, l'offset par lignes est stable). Embeddings requis
        // mais incomplets sur le préfixe → full scan pour ré-embedder.
        let skip_first_lines = match month_state {
            Some(state) if !with_embeddings || state.embeddings_complete => {
                state.ingested_lines.unwrap_or(0).max(0) as usize
            }
            _ => 0,
        };

        let lines = read_jsonl_gz_lines(path)?;
        let lines_total = lines.len();
        if skip_first_lines > 0 {
            tracing::info!(
                source = %path.display(),
                skip_first_lines,
                lines_total,
                "ingest_judilibre_resume"
            );
        }
        let mut candidates: Vec<Candidate> = Vec::new();
        let mut file_counts = IngestCounts::default();

        for line in lines.into_iter().skip(skip_first_lines) {
            if line.is_empty() {
                continue;
            }
            match classify_judilibre(line) {
                Ok(Some(cand)) => candidates.push(cand),
                Ok(None) => file_counts.errors += 1,
                Err(e) => {
                    tracing::error!(source = %path.display(), error = %e, "parse Judilibre échec");
                    file_counts.errors += 1;
                }
            }
            if candidates.len() >= BATCH_SIZE {
                let batch = std::mem::take(&mut candidates);
                drain_batch(
                    &conn,
                    embedder.as_ref(),
                    batch,
                    with_embeddings,
                    mode,
                    &mut file_counts,
                )
                .await?;
            }
        }
        if !candidates.is_empty() {
            drain_batch(
                &conn,
                embedder.as_ref(),
                candidates,
                with_embeddings,
                mode,
                &mut file_counts,
            )
            .await?;
        }

        tracing::info!(
            source = %path.display(),
            created = file_counts.created,
            updated = file_counts.updated,
            skipped = file_counts.skipped,
            errors = file_counts.errors,
            chunks = file_counts.chunks_created,
            "ingest_judilibre"
        );
        total.merge(&file_counts);

        // Marque le mois entièrement ingéré dans le manifeste — écrit
        // immédiatement pour survivre à un arrêt en cours de run.
        if file_counts.errors == 0 {
            if let Some(state) = manifest
                .jurisdictions
                .get_mut(&jur)
                .and_then(|j| j.months.get_mut(&month_key))
            {
                state.fully_ingested = true;
                state.ingested_size = Some(file_size);
                state.ingested_lines = Some(lines_total as i64);
                if with_embeddings {
                    state.embeddings_complete = true;
                }
                manifest
                    .save(&manifest_path)
                    .map_err(|e| anyhow!("manifest save: {e}"))?;
            }
        }
    }

    tracing::info!(
        files = files.len(),
        manifest_skipped,
        created = total.created,
        updated = total.updated,
        skipped = total.skipped,
        errors = total.errors,
        chunks = total.chunks_created,
        "ingest_judilibre_total"
    );
    Ok(())
}

/// Re-fetch ciblé de décisions Judilibre par id (`/decision`) puis ingest.
///
/// Répare une poignée de décisions désynchronisées (ex. résurrection : supprimée
/// puis re-créée côté Judilibre, ADR 0087) **sans reculer le `history_watermark`
/// global** — qui re-traiterait des dizaines de milliers de transactions pour
/// quelques décisions. Réutilise le chemin d'ingest normal (parse → chunk →
/// extract → upsert), idempotent par `content_checksum` (`MissingHash` : un id
/// déjà à jour est skippé).
pub async fn refetch_judilibre(
    client: &JudilibreClient,
    ids: &[String],
    with_embeddings: bool,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;

    // Re-fetch de maintenance : re-embed vLLM local **strict**, jamais Cloudflare
    // (coût ; même classe que le re-split #29). Erreur franche si vLLM injoignable.
    let embedder = if with_embeddings {
        Some(build_vllm_strict(&settings).await?)
    } else {
        None
    };

    let mut candidates: Vec<Candidate> = Vec::new();
    for id in ids {
        let payload = client
            .decision(id)
            .await
            .map_err(|e| anyhow!("refetch {id}: {e}"))?;
        let line = serde_json::to_vec(&payload).context("refetch: sérialisation payload")?;
        match classify_judilibre(line)? {
            Some(candidate) => {
                tracing::info!(id = %id, uid = %candidate.decision.source_uid, "refetch_judilibre_fetched");
                candidates.push(candidate);
            }
            None => tracing::warn!(id = %id, "refetch: décision non reconnue, skip"),
        }
    }

    let mut counts = IngestCounts::default();
    drain_batch(
        &conn,
        embedder.as_ref(),
        candidates,
        with_embeddings,
        IngestMode::MissingHash,
        &mut counts,
    )
    .await?;
    tracing::info!(
        requested = ids.len(),
        created = counts.created,
        updated = counts.updated,
        skipped = counts.skipped,
        chunks = counts.chunks_created,
        "refetch_judilibre_total"
    );
    Ok(())
}

/// Re-fetch ciblé de quelques décisions Judilibre par ObjectId puis ré-ingest sur
/// une **connexion et un embedder fournis** (réutilisé par le re-split #29 :
/// chaque provenance scindée a déjà été re-pointée vers sa nouvelle décision, le
/// contenu doit y atterrir). [`IngestMode::All`] : UPDATE complet inconditionnel
/// — le triage résout l'id par `source_uid` (qui pointe désormais la décision
/// scindée). `require_embeddings = true` : re-chunk + re-embed ciblé (vLLM local,
/// l'embedder est construit hors Cloudflare par l'appelant). Renvoie une erreur si
/// **aucune** décision n'a été reconnue (toutes disparues côté Judilibre) — le
/// re-split traite ce cas en sautant le groupe.
pub(super) async fn refetch_into<E: Embedder>(
    conn: &Connection,
    client: &JudilibreClient,
    embedder: &E,
    ids: &[String],
) -> Result<()> {
    let mut candidates: Vec<Candidate> = Vec::new();
    for id in ids {
        let payload = client
            .decision(id)
            .await
            .map_err(|e| anyhow!("refetch {id}: {e}"))?;
        let line = serde_json::to_vec(&payload).context("refetch: sérialisation payload")?;
        match classify_judilibre(line)? {
            Some(candidate) => candidates.push(candidate),
            None => tracing::warn!(id = %id, "refetch: décision non reconnue, skip"),
        }
    }
    if candidates.is_empty() {
        return Err(anyhow!(
            "refetch_into: aucune décision reconnue ({} ids)",
            ids.len()
        ));
    }

    let mut counts = IngestCounts::default();
    drain_batch_in_txn(
        conn,
        Some(embedder),
        candidates,
        true,
        IngestMode::All,
        &mut counts,
    )
    .await?;
    Ok(())
}
