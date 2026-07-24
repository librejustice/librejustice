//! DILA bulk (JADE / CONSTIT / CNIL, ADR 0093/0185) : ingest tarball streamé + sync.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{anyhow, Result};

use lj_core::parsing::{build_source_fields_dila, parse_dila_doc, DilaDoc, DilaFond};
use lj_llm::backend::AnyEmbedder;
use lj_store::db::Connection;
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

use super::batch::drain_batch;
use super::embed::build_embedder_opt;
use super::files::collect_tar_gz;
use super::{
    content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode, BATCH_SIZE,
};

/// Fond bulk DILA, vue côté ingest : porte les deux représentations du fond — le
/// `DilaFond` du parser pur (`lj-core`) et celui du bord I/O (`lj-sources`,
/// downloader + `repair_dila`) — disjointes par construction (AGENTS.md #1). La
/// clap `Command::IngestDila { fond }` construit cette valeur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Fond {
    Jade,
    Constit,
    /// Délibérations/décisions de la CNIL (ADR 0185).
    Cnil,
}

impl Fond {
    /// Préfixe `source_uid` du fond (`dila-jade`/`dila-constit`/`dila-cnil`).
    /// `parse_dila_xml` forme `source_uid = "{member_path}/{ID}"` → on lui passe
    /// ce préfixe comme `member_path` pour obtenir `dila-<fond>/<ID DILA>`
    /// (clé pivot stable, ADR 0093 ; mappée par `source_from_source_uid`).
    fn source_prefix(self) -> &'static str {
        match self {
            Fond::Jade => "dila-jade",
            Fond::Constit => "dila-constit",
            Fond::Cnil => "dila-cnil",
        }
    }

    /// Fond du parser pur (`lj-core`).
    fn core(self) -> DilaFond {
        match self {
            Fond::Jade => DilaFond::Jade,
            Fond::Constit => DilaFond::Constit,
            Fond::Cnil => DilaFond::Cnil,
        }
    }

    /// Fond du bord I/O (`lj-sources` : downloader + `repair_dila`).
    pub(super) fn source(self) -> lj_sources::dila::DilaFond {
        match self {
            Fond::Jade => lj_sources::dila::DilaFond::Jade,
            Fond::Constit => lj_sources::dila::DilaFond::Constit,
            Fond::Cnil => lj_sources::dila::DilaFond::Cnil,
        }
    }
}

/// ID DILA d'un chemin de membre tar : dernier segment sans l'extension `.xml`
/// (ex. `…/inedit/2007/CETATEXT000007612345.xml` → `CETATEXT000007612345`).
/// `None` si le membre n'est pas un `.xml`.
pub(super) fn dila_member_id(name: &str) -> Option<&str> {
    let file = name.rsplit('/').next()?;
    file.strip_suffix(".xml")
        .or_else(|| file.strip_suffix(".XML"))
}

/// Doublon publie/inedit à skipper (#36) : un membre sous `/inedit/` dont l'ID
/// existe aussi sous `/publie/` du même tarball. Winner déterministe = `publie/`
/// (texte « publié » = version de référence) ; sans cette garde le même
/// `source_uid` reçoit deux checksums et ping-ponge à chaque ingest (40 doublons
/// mesurés sur le stock JADE global, note 2026-06-15) → re-chunk/re-embed inutile.
fn is_inedit_dup(name: &str, publie_ids: &HashSet<String>) -> bool {
    name.contains("/inedit/") && dila_member_id(name).is_some_and(|id| publie_ids.contains(id))
}

/// Pré-passe noms-seuls : IDs présents sous `/publie/` dans un tarball DILA — la
/// matière du winner publie/inedit ([`is_inedit_dup`], #36). O(noms), pas de
/// lecture du contenu.
fn collect_publie_ids(path: &Path) -> Result<HashSet<String>> {
    let mut set = HashSet::new();
    lj_sources::tar_reader::for_each_member_name(path, |name| {
        if name.contains("/publie/") && name.to_lowercase().ends_with(".xml") {
            if let Some(id) = dila_member_id(name) {
                set.insert(id.to_string());
            }
        }
        Ok(())
    })?;
    Ok(set)
}

/// Issue du classement d'un membre DILA : décision à **texte intégral** (chemin
/// d'ingest normal) ou enregistrement **analyse-seule** (#33, ADR 0105) routé à
/// part — enrichissement `source_fields` d'une décision existante OU orpheline
/// créée, jamais d'écrasement d'un texte/chunks réels.
pub(super) enum ClassifiedDila {
    Full(Candidate),
    Analysis(Candidate),
}

/// Parse + classe un membre XML bulk DILA réparé (miroir de [`classify_xml`]).
///
/// `raw_repaired` = octets déjà passés par `repair_dila` (bord lj-sources) ;
/// `content_checksum` est calculé sur `raw_brut` (pré-repair, idempotence #7).
/// `None` ⇒ skip non fatal : juridiction JADE non routée. `Some(Full)` = texte
/// intégral ; `Some(Analysis)` = analyse-seule (CONTENU absent, SOMMAIRE ANA/SCT
/// comme contenu).
///
/// [`classify_xml`]: super::prepare::classify_xml
pub(super) fn classify_dila(
    raw_repaired: Vec<u8>,
    raw_brut: &[u8],
    fond: Fond,
) -> Result<Option<ClassifiedDila>> {
    let doc = parse_dila_doc(&raw_repaired, fond.source_prefix(), fond.core())
        .map_err(|e| anyhow!("parse_dila_doc ({}): {e}", fond.source_prefix()))?;

    // Membre sans corps (ni CONTENU ni SOMMAIRE) → skip non fatal : nominal pour
    // CNIL (fiches de registre sans texte, ADR 0185), pas une erreur de parsing.
    let (decision, is_analysis) = match doc {
        Some(DilaDoc::Full(d)) => (d, false),
        Some(DilaDoc::Analysis(d)) => (d, true),
        None => return Ok(None),
    };

    if decision.jurisdiction_type.is_none() {
        tracing::warn!(uid = %decision.source_uid, "UID DILA non routé, skip");
        return Ok(None);
    }

    let cand = Candidate {
        decision_id: None,
        public_id: generate_public_id(),
        decision,
        content_checksum: content_checksum(raw_brut),
        raw_payload: raw_repaired,
        payload_format: "dila-xml".to_string(),
        write_mode: super::WriteMode::Full,
        dila_fond: Some(fond.core()),
        prebuilt_source_fields: None,
        prebuilt_extracted: None,
    };
    Ok(Some(if is_analysis {
        ClassifiedDila::Analysis(cand)
    } else {
        ClassifiedDila::Full(cand)
    }))
}

/// Rattache les enregistrements DILA **analyse-seule** (#33, ADR 0105). Chemin
/// **séparé** du drain normal, sous garde anti-écrasement (comme judilibre) : on ne
/// re-chunke, ne ré-embedde et ne réécrit JAMAIS le texte d'une décision existante.
/// Par enregistrement (dédup intra-lot par `source_uid`, last-wins) :
///
/// - `source_uid` déjà connu + checksum inchangé → skip (idempotent #7) ;
/// - `source_uid` connu (changé/tombstoné) → on rafraîchit **seulement**
///   `source_fields` (ANA/SCT à jour) via `upsert_decision_source` ;
/// - identité (`ecli`/`canonical_ref`) trouvée → **case a** : enrichissement
///   `source_fields` de la décision existante (texte réel d'une source de rang
///   supérieur — opendata 55 / judilibre 60 > dila-jade 50, jamais autorité ⇒ jamais
///   d'overwrite du texte canonique, ADR 0098 §3) ;
/// - sinon → **case b** : décision **orpheline** créée + chunkée (+ embeddée si
///   `require_embeddings`) via le drain normal, où l'analyse EST le contenu cherchable.
///   Si un texte réel arrive plus tard (même `canonical_ref`), le merge cross-source
///   le promeut autorité et écrase proprement l'analyse — l'abstrat reste en
///   `source_fields` dila-jade.
async fn apply_dila_analyses(
    conn: &Connection,
    embedder: Option<&AnyEmbedder>,
    require_embeddings: bool,
    fond: Fond,
    analysis: Vec<Candidate>,
    counts: &mut IngestCounts,
) -> Result<()> {
    if analysis.is_empty() {
        return Ok(());
    }
    // Dédup intra-lot par source_uid (last-wins), comme `triage_candidates`.
    let total = analysis.len();
    let mut by_uid: std::collections::HashMap<String, Candidate> =
        std::collections::HashMap::with_capacity(total);
    for c in analysis {
        by_uid.insert(c.decision.source_uid.clone(), c);
    }
    counts.dedup_in_batch += total - by_uid.len();

    let repo = DecisionRepository::new(conn);
    let mut orphans: Vec<Candidate> = Vec::new();
    let (mut enriched, mut skipped) = (0usize, 0usize);
    for (uid, cand) in by_uid {
        match repo.find_provenance(&uid).await? {
            Some((_, checksum, active)) if active && checksum == cand.content_checksum => {
                counts.skipped += 1;
                skipped += 1;
            }
            Some((id, _, _)) => {
                let sf = build_source_fields_dila(&cand.raw_payload, fond.core());
                repo.upsert_decision_source(id, &uid, &cand.content_checksum, "dila-xml", &sf)
                    .await?;
                counts.updated += 1;
                enriched += 1;
            }
            None => match repo
                .resolve_identity(
                    &cand.decision,
                    lj_ingest::extract::canonical_ref(&cand.decision).as_deref(),
                )
                .await?
            {
                Some(existing_id) => {
                    let sf = build_source_fields_dila(&cand.raw_payload, fond.core());
                    repo.upsert_decision_source(
                        existing_id,
                        &uid,
                        &cand.content_checksum,
                        "dila-xml",
                        &sf,
                    )
                    .await?;
                    counts.updated += 1;
                    enriched += 1;
                }
                None => orphans.push(cand),
            },
        }
    }

    let orphaned = orphans.len();
    if !orphans.is_empty() {
        // Création neuve (aucune décision existante à écraser) → chunk + embed sûrs.
        drain_batch(
            conn,
            embedder,
            orphans,
            require_embeddings,
            IngestMode::MissingHash,
            counts,
        )
        .await?;
    }
    tracing::info!(
        fond = ?fond,
        enriched,
        orphaned,
        skipped,
        "apply_dila_analyses (#33)"
    );
    Ok(())
}

/// Ingère un fond bulk DILA (`tar.gz` locaux sous `<cache>/dila/<fond>/tarballs/`,
/// ADR 0093). Calqué sur [`ingest_legi_tarball`] (lecture tar STREAMÉE via canal
/// borné, RAM ~constante) + [`ingest_opendata`] (triage idempotent par batch,
/// upsert ECLI-first déjà actif côté repository).
///
/// Par archive : un thread lecteur (`for_each_member`) fait `repair_dila` (bord
/// I/O) → `classify_dila` (`parse_dila_xml` + filtre CONSTIT AN/SEN) et pousse les
/// `Candidate` sur un canal borné ; le consumer async batch → `drain_batch`
/// (`payload_format = "dila-xml"`). Les `.dat` (suppressions) sont accumulés puis
/// appliqués APRÈS les upserts via `repo.delete(source_uid)` (provenance-aware,
/// ADR 0080/0087). Idempotent (`content_checksum` brut #7). Sans embeddings
/// (backfill séparé). `sync_dila` auto-switch dépose stock global + incréments
/// dans le même dossier (ingérés par ordre de nom).
///
/// [`ingest_legi_tarball`]: super::legi
/// [`ingest_opendata`]: super::opendata::ingest_opendata
pub async fn ingest_dila(fond: Fond) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;

    let tarballs_dir = lj_sources::dila::tarballs_dir(&settings.cache_dir(), fond.source());
    let mut tarballs = collect_tar_gz(&tarballs_dir)?;
    tarballs.sort();
    if tarballs.is_empty() {
        tracing::info!(dir = %tarballs_dir.display(), fond = ?fond, "aucun tar.gz DILA trouvé");
        return Ok(());
    }

    let (embedder, require_embeddings) = build_embedder_opt(&settings).await?;
    ingest_dila_paths(
        &conn,
        fond,
        &tarballs,
        embedder.as_ref(),
        require_embeddings,
    )
    .await
}

/// Ingère une liste d'archives DILA (stock et/ou incréments) dans l'ordre fourni,
/// puis logge le total. Partagé par [`ingest_dila`] (toutes les archives sur disque)
/// et [`sync_dila`] (uniquement les archives fraîchement téléchargées, comme
/// [`sync_legi`] — pas de re-stream de l'historique à chaque run).
///
/// [`sync_legi`]: super::legi::sync_legi
async fn ingest_dila_paths(
    conn: &Connection,
    fond: Fond,
    paths: &[std::path::PathBuf],
    embedder: Option<&AnyEmbedder>,
    require_embeddings: bool,
) -> Result<()> {
    let mut total = IngestCounts::default();
    let mut deleted_total: u64 = 0;
    for path in paths {
        let (counts, deleted) =
            ingest_dila_tarball(conn, fond, path, embedder, require_embeddings).await?;
        deleted_total += deleted;
        total.merge(&counts);
    }

    tracing::info!(
        fond = ?fond,
        tarballs = paths.len(),
        created = total.created,
        updated = total.updated,
        skipped = total.skipped,
        errors = total.errors,
        chunks = total.chunks_created,
        deleted = deleted_total,
        "ingest_dila_total"
    );
    Ok(())
}

/// Ingère UNE archive DILA (stock ou incrément) en streaming, renvoie
/// `(counts, deleted)`.
///
/// Lecture+repair+parse (gzip+tar+XML = CPU sync) en thread bloquant, BORNÉE par
/// un canal : RAM ~constante (stocks globaux jade/constit = centaines de k
/// membres → JAMAIS tout en mémoire, cf. `tar_reader::for_each_member`). Le DB
/// reste côté async (la connexion n'est pas déplaçable dans `spawn_blocking`).
/// Backpressure : canal plein (flush DB en cours) ⇒ le lecteur attend. Les `.dat`
/// (suppressions) sont accumulés puis appliqués APRÈS les upserts.
async fn ingest_dila_tarball(
    conn: &Connection,
    fond: Fond,
    path: &Path,
    embedder: Option<&AnyEmbedder>,
    require_embeddings: bool,
) -> Result<(IngestCounts, u64)> {
    // Messages remontés par le thread lecteur (membres parsés). `Candidate` boxé
    // (gros : `Decision` + payload). `Delete` = IDs DILA d'un `.dat` de suppression.
    enum DilaMsg {
        Candidate(Box<Candidate>),
        /// Enregistrement analyse-seule (CONTENU absent, #33) — traité à part.
        Analysis(Box<Candidate>),
        /// Membre écarté non fatal (juridiction JADE non routée).
        Skip,
        /// Doublon publie/inedit écarté (winner = publie, #36).
        DupSkip,
        Empty,
        ParseErr,
        Delete(Vec<String>),
    }

    // Pré-passe : IDs sous `/publie/` (winner publie/inedit #36). Faite avant le
    // stream principal pour être churn-free quel que soit l'ordre des membres
    // (l'inedit/ précède publie/ alphabétiquement → sans pré-passe, l'inedit
    // déclencherait un UPDATE transitoire à chaque re-run).
    let publie_ids = {
        let p = path.to_path_buf();
        tokio::task::spawn_blocking(move || collect_publie_ids(&p))
            .await
            .map_err(|e| anyhow!("pré-passe noms DILA {}: {e}", path.display()))??
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel::<DilaMsg>(BATCH_SIZE * 4);
    let tar_path = path.to_path_buf();
    let reader = tokio::task::spawn_blocking(move || -> Result<()> {
        lj_sources::tar_reader::for_each_member(&tar_path, |name, raw| {
            let lower = name.to_lowercase();
            // Doublon publie/inedit : on ne garde que la version publie/ (#36).
            if lower.ends_with(".xml") && is_inedit_dup(&name, &publie_ids) {
                return tx
                    .blocking_send(DilaMsg::DupSkip)
                    .map_err(|_| anyhow!("canal DILA fermé (consumer arrêté)"));
            }
            // Suppressions : `.dat` (lignes = chemin tar ou ID DILA nu) ; on
            // retient le dernier segment (= ID `CETATEXT…`/`CONSTEXT…`/`JURITEXT…`).
            if lower.ends_with(".dat") {
                let ids: Vec<String> = String::from_utf8_lossy(&raw)
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(|l| l.rsplit('/').next().unwrap_or(l).to_string())
                    .collect();
                if !ids.is_empty() {
                    tx.blocking_send(DilaMsg::Delete(ids))
                        .map_err(|_| anyhow!("canal DILA fermé (consumer arrêté)"))?;
                }
                return Ok(());
            }
            // Seuls les `.xml` sont des décisions ; le reste (PDF présentation) ignoré.
            if !lower.ends_with(".xml") {
                return Ok(());
            }
            let msg = if raw.is_empty() {
                DilaMsg::Empty
            } else {
                let repaired = lj_sources::dila::repair_dila(&raw, fond.source());
                match classify_dila(repaired, &raw, fond) {
                    Ok(Some(ClassifiedDila::Full(cand))) => DilaMsg::Candidate(Box::new(cand)),
                    Ok(Some(ClassifiedDila::Analysis(cand))) => DilaMsg::Analysis(Box::new(cand)),
                    // JADE non routé : skip non fatal.
                    Ok(None) => DilaMsg::Skip,
                    Err(e) => {
                        tracing::error!(member = %name, error = %e, "parse DILA échec");
                        DilaMsg::ParseErr
                    }
                }
            };
            tx.blocking_send(msg)
                .map_err(|_| anyhow!("canal DILA fermé (consumer arrêté)"))
        })
    });

    let mut candidates: Vec<Candidate> = Vec::new();
    let mut analysis: Vec<Candidate> = Vec::new();
    let mut counts = IngestCounts::default();
    let mut dat_ids: Vec<String> = Vec::new();
    let mut dup_skipped = 0usize;

    while let Some(msg) = rx.recv().await {
        match msg {
            // Analyses-seules accumulées à part : traitées après les textes intégraux
            // (#33), pour qu'un éventuel texte du même lot crée la décision d'abord.
            DilaMsg::Analysis(cand) => analysis.push(*cand),
            DilaMsg::Candidate(cand) => {
                candidates.push(*cand);
                if candidates.len() >= BATCH_SIZE {
                    let batch = std::mem::take(&mut candidates);
                    // Embeddings selon `with_embeddings` (None = ingest rapide
                    // sans vecteurs ; backfill via re-run --with-embeddings, chemin
                    // re-embed de drain_batch). Mode MissingHash (idempotent #7).
                    drain_batch(
                        conn,
                        embedder,
                        batch,
                        require_embeddings,
                        IngestMode::MissingHash,
                        &mut counts,
                    )
                    .await?;
                }
            }
            DilaMsg::Skip => {
                counts.skipped += 1;
            }
            DilaMsg::DupSkip => {
                counts.skipped += 1;
                dup_skipped += 1;
            }
            DilaMsg::Empty => {
                counts.skipped += 1;
                counts.empty_skipped += 1;
            }
            DilaMsg::ParseErr => counts.errors += 1,
            DilaMsg::Delete(ids) => dat_ids.extend(ids),
        }
    }
    // Canal fermé = lecteur fini : remonter une éventuelle erreur de lecture/parse.
    reader
        .await
        .map_err(|e| anyhow!("tâche lecture DILA {}: {e}", path.display()))??;

    if !candidates.is_empty() {
        drain_batch(
            conn,
            embedder,
            candidates,
            require_embeddings,
            IngestMode::MissingHash,
            &mut counts,
        )
        .await?;
    }

    // Analyses-seules (#33) : APRÈS les textes intégraux du lot (rattachement à une
    // décision existante OU création orpheline), AVANT les suppressions.
    apply_dila_analyses(
        conn,
        embedder,
        require_embeddings,
        fond,
        analysis,
        &mut counts,
    )
    .await?;

    // Suppressions appliquées APRÈS les upserts (un incrément ajoute ET retire,
    // #7) via `delete` provenance-aware (ADR 0080/0087) sur le `source_uid`
    // reconstruit `dila-<fond>/<ID>`.
    let repo = DecisionRepository::new(conn);
    let mut deleted_total: u64 = 0;
    for dila_id in dat_ids {
        let source_uid = format!("{}/{dila_id}", fond.source_prefix());
        if repo
            .delete(&source_uid)
            .await
            .map_err(|e| anyhow!("delete {source_uid}: {e}"))?
        {
            deleted_total += 1;
        }
    }

    tracing::info!(
        source = %path.display(),
        created = counts.created,
        updated = counts.updated,
        skipped = counts.skipped,
        errors = counts.errors,
        chunks = counts.chunks_created,
        dup_skipped,
        "ingest_dila"
    );
    Ok((counts, deleted_total))
}

/// Sync d'un fond DILA, **auto-switch cold ↔ warm** en un seul point d'entrée
/// (mirroir de [`sync_legi`]) : [`lj_sources::dila::sync_dila`] télécharge le stock
/// global au 1er run puis les incréments postérieurs au watermark sous
/// `<cache>/dila/<fond>/tarballs/`, PUIS [`ingest_dila_paths`] ingère UNIQUEMENT
/// les archives fraîchement téléchargées (pas de re-stream de l'historique). Le
/// watermark avance à chaque téléchargement → idempotent et reprenable.
///
/// [`sync_legi`]: super::legi::sync_legi
pub async fn sync_dila(fond: Fond) -> Result<()> {
    let settings = Settings::from_env()?;
    // `lj_sources::dila::sync_dila` s'appuie sur un client reqwest **bloquant** : le
    // Drop de son runtime interne panique s'il tombe dans le contexte async d'un
    // worker tokio (« Cannot drop a runtime… »). Il DOIT donc tourner sur un thread
    // bloquant dédié.
    let (cache_dir, src) = (settings.cache_dir(), fond.source());
    let downloaded =
        tokio::task::spawn_blocking(move || lj_sources::dila::sync_dila(&cache_dir, src))
            .await
            .map_err(|e| anyhow!("sync_dila join {:?}: {e}", fond))?
            .map_err(|e| anyhow!("sync_dila {:?}: {e}", fond))?;
    if downloaded.is_empty() {
        tracing::info!(fond = ?fond, "sync_dila : rien de neuf (≤ watermark)");
        return Ok(());
    }

    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;

    tracing::info!(fond = ?fond, downloaded = downloaded.len(), "sync_dila : ingestion des archives fraîches");
    let (embedder, require_embeddings) = build_embedder_opt(&settings).await?;
    ingest_dila_paths(
        &conn,
        fond,
        &downloaded,
        embedder.as_ref(),
        require_embeddings,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_id_strips_path_and_xml() {
        assert_eq!(
            dila_member_id("jade/global/inedit/2007/sub/CETATEXT000007612345.xml"),
            Some("CETATEXT000007612345")
        );
        // Pas un .xml → pas d'ID (PDF de présentation, .dat…).
        assert_eq!(dila_member_id("jade/global/publie/foo.pdf"), None);
    }

    // Spec #36 : winner publie/inedit. Un membre inedit/ dont l'ID est aussi en
    // publie/ est écarté ; un inedit/ unique (ID absent de publie/) est gardé ;
    // un publie/ n'est jamais écarté.
    #[test]
    fn inedit_dup_skipped_only_when_publie_twin_exists() {
        let publie: HashSet<String> = ["CETATEXT000000000001".to_string()].into_iter().collect();
        // inedit/ avec jumeau publie/ → skip.
        assert!(is_inedit_dup(
            "jade/global/inedit/2007/CETATEXT000000000001.xml",
            &publie
        ));
        // inedit/ sans jumeau publie/ → gardé.
        assert!(!is_inedit_dup(
            "jade/global/inedit/2007/CETATEXT000000000099.xml",
            &publie
        ));
        // publie/ → jamais écarté (même ID présent dans le set).
        assert!(!is_inedit_dup(
            "jade/global/publie/2007/CETATEXT000000000001.xml",
            &publie
        ));
    }
}
