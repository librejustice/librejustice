//! BOFiP-Impôts (ADR 0196) : sync du snapshot open data `bofip-vigueur`
//! (doctrine fiscale DGFiP opposable, ~9 k documents) → `legal_text`
//! (nature=`BOFIP`, préambule en `body`) + `legal_article` (**un § numéroté =
//! une ligne**, l'unité citée par les décisions : « paragraphe n° 130 du
//! BOI-… ») + `legal_toc_edge` (plan du document dérivé des intertitres
//! `h1`–`h6`, via `corpus_toc` — sommaire réel et vue-lecture structurée).
//!
//! Source **snapshot vivant** (publications en vigueur), pas un fond
//! incrémental : chaque sync re-télécharge l'export JSONL entier ;
//! l'idempotence vit dans l'upsert par `content_checksum` (xxh3-64 du texte
//! parsé du §, #7) et la purge des versions/§ hors snapshot
//! (`delete_legal_articles_versions_except`). Le versionnage historique
//! complet (tarballs `bofip-impots`) est un track ultérieur (ADR 0196 §7).

use std::path::Path;

use anyhow::{anyhow, Result};

use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Sync BOFiP : télécharge l'export `bofip-vigueur` puis l'ingère.
pub async fn sync_bofip() -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.cache_dir().join("bofip");
    let path = tokio::task::spawn_blocking(move || lj_sources::bofip::fetch_bofip_vigueur(&dir))
        .await
        .map_err(|e| anyhow!("tâche fetch bofip: {e}"))?
        .map_err(|e| anyhow!("fetch bofip-vigueur: {e}"))?;
    ingest_bofip(&path).await
}

/// Ingère un export JSONL `bofip-vigueur` local (bootstrap manuel ou suite du
/// sync). Parse + upsert en streaming, puis refresh `code_title` (ADR 0114) et
/// slugs (ADR 0162) une fois en fin de run.
pub async fn ingest_bofip(path: &Path) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Le refresh `code_title` final est un UPDATE-jointure pleine table → hors
    // statement_timeout (batch offline, même règle que KALI/LEGI).
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let (docs, upserted, skipped, errors) = ingest_bofip_file(&repo, path).await?;

    // Titre du document dénormalisé sur ses § (`search_title`, ADR 0114).
    let retitled = repo
        .refresh_article_code_titles()
        .await
        .map_err(|e| anyhow!("refresh_article_code_titles: {e}"))?;
    // Slugs des textes nouveaux (ADR 0162).
    let slugged = super::slugs::assign_text_slugs(&repo).await?;

    tracing::info!(
        source = %path.display(),
        docs,
        upserted,
        skipped,
        errors,
        retitled,
        slugged,
        "ingest_bofip"
    );
    Ok(())
}

/// Boucle documents → upsert. Renvoie `(docs, §_upsertés, §_skippés, erreurs)`.
///
/// La lecture/parse (fichier ~centaines de Mo) tourne en thread bloquant,
/// bornée par un canal ; le DB reste côté async (même mécanique que KALI).
async fn ingest_bofip_file(
    repo: &DecisionRepository<'_>,
    path: &Path,
) -> Result<(usize, usize, usize, usize)> {
    let (tx, mut rx) =
        tokio::sync::mpsc::channel::<std::result::Result<lj_sources::bofip::BofipDoc, ()>>(64);
    let jsonl = path.to_path_buf();
    let reader = tokio::task::spawn_blocking(move || -> Result<()> {
        use std::io::BufRead;
        let file = std::fs::File::open(&jsonl)
            .map_err(|e| anyhow!("ouverture {}: {e}", jsonl.display()))?;
        for line in std::io::BufReader::new(file).lines() {
            let line = line.map_err(|e| anyhow!("lecture {}: {e}", jsonl.display()))?;
            if line.trim().is_empty() {
                continue;
            }
            let msg = match lj_sources::bofip::parse_bofip_record(&line) {
                // Hors périmètre (Actualité) → rien à envoyer.
                Ok(None) => continue,
                Ok(Some(doc)) => Ok(doc),
                Err(e) => {
                    tracing::error!(error = %e, "bofip: record invalide");
                    Err(())
                }
            };
            tx.blocking_send(msg)
                .map_err(|_| anyhow!("canal BOFiP fermé (consumer arrêté)"))?;
        }
        Ok(())
    });

    let (mut docs, mut upserted, mut skipped, mut errors) = (0usize, 0usize, 0usize, 0usize);
    let today = chrono::Utc::now().date_naive();
    while let Some(msg) = rx.recv().await {
        let doc = match msg {
            Ok(v) => v,
            Err(()) => {
                errors += 1;
                continue;
            }
        };
        docs += 1;
        let (u, s) = ingest_bofip_doc(repo, doc, today).await?;
        upserted += u;
        skipped += s;
    }
    reader
        .await
        .map_err(|e| anyhow!("tâche lecture BOFiP {}: {e}", path.display()))??;
    Ok((docs, upserted, skipped, errors))
}

/// Upsert d'un document BOFiP : `legal_text` + ses § en `legal_article` +
/// plan en `legal_toc_edge` (intertitres), puis purge des versions/§ hors
/// snapshot. Renvoie `(§_upsertés, §_skippés)`.
async fn ingest_bofip_doc(
    repo: &DecisionRepository<'_>,
    doc: lj_sources::bofip::BofipDoc,
    today: chrono::NaiveDate,
) -> Result<(usize, usize)> {
    let debut = chrono::NaiveDate::parse_from_str(&doc.debut_de_validite, "%Y-%m-%d")
        .map_err(|e| anyhow!("date BOFiP invalide {:?}: {e}", doc.debut_de_validite))?;
    let title_key = lj_extract::extract::normalize_instrument(&doc.titre);
    repo.upsert_legal_text(&lj_store::repository::LegalTextRow {
        text_uid: doc.identifiant.clone(),
        jurisdiction: "FR".to_string(),
        title: doc.titre.clone(),
        title_key,
        nature: "BOFIP".to_string(),
        last_modified: Some(debut),
        date_texte: None,
        date_publi: Some(debut),
        // Identité = l'identifiant juridique BOI (pas la cascade ELI/NOR ADR 0115).
        eli: None,
        nor: None,
        instrument_key: None,
        body: doc.preambule,
        // Le snapshot `bofip-vigueur` ne contient que des publications en vigueur.
        status: Some("VIGUEUR".to_string()),
    })
    .await
    .map_err(|e| anyhow!("upsert_legal_text {}: {e}", doc.identifiant))?;

    let (mut upserted, mut skipped) = (0usize, 0usize);
    let mut keep_num_keys: Vec<String> = Vec::with_capacity(doc.paragraphs.len());
    let mut toc_articles: Vec<super::corpus_toc::TocArticle> =
        Vec::with_capacity(doc.paragraphs.len());
    for (i, p) in doc.paragraphs.into_iter().enumerate() {
        let num_key = lj_core::article_key::identity_key(&p.num);
        keep_num_keys.push(num_key.clone());
        toc_articles.push(super::corpus_toc::TocArticle {
            num: p.num.clone(),
            num_key: num_key.clone(),
            versions: vec![super::corpus_toc::TocVersion {
                source_uid: format!("{}#{}", doc.identifiant, p.num),
                status: "VIGUEUR".to_string(),
                date_debut: None,
                date_fin: None,
            }],
            title_path: (!p.section_path.is_empty()).then(|| p.section_path.join(" > ")),
        });
        let checksum = xxhash_rust::xxh3::xxh3_64(p.texte.as_bytes());
        let row = lj_store::repository::LegalArticleRow {
            text_uid: doc.identifiant.clone(),
            num: p.num.clone(),
            num_key,
            // Ordre de lecture réel = ordre des § dans le document.
            position: Some(i as i32),
            title_path: Some(doc.titre.clone()),
            status: "VIGUEUR".to_string(),
            date_debut: Some(debut),
            date_fin: None,
            // § porte-numéro d'un intertitre : ancre citable sans corps.
            texte: (!p.texte.is_empty()).then_some(p.texte),
            // de référence FR officielle, monolingue (ADR 0116).
            texte_original: None,
            lang_original: None,
            translation: "officiel".to_string(),
            nota: None,
            content_checksum: checksum,
            source: "bofip".to_string(),
            source_uid: format!("{}#{}", doc.identifiant, p.num),
            // Permalien officiel versionné (numéro PGP opaque, non dérivable).
            source_url: doc.permalien.clone(),
            // Snapshot re-téléchargé à chaque sync → fraîcheur = date de get.
            source_asof: Some(today),
            source_upstream_url: None,
        };
        if repo
            .upsert_legal_article(&row)
            .await
            .map_err(|e| anyhow!("upsert_legal_article {}: {e}", row.source_uid))?
        {
            upserted += 1;
        } else {
            skipped += 1;
        }
    }
    // Versions remplacées et § disparus du snapshot → purge (rejouable).
    repo.delete_legal_articles_versions_except("bofip", &doc.identifiant, debut, &keep_num_keys)
        .await
        .map_err(|e| anyhow!("purge versions {}: {e}", doc.identifiant))?;
    // Plan du document (ADR 0186) : purge autoritaire puis réécriture de
    // l'arbre dérivé des intertitres (vide = document à plat).
    repo.delete_toc_edges_by_text(&doc.identifiant)
        .await
        .map_err(|e| anyhow!("delete_toc_edges_by_text {}: {e}", doc.identifiant))?;
    let toc = super::corpus_toc::derive_corpus_toc(&doc.identifiant, &toc_articles);
    repo.replace_toc_edges(&toc)
        .await
        .map_err(|e| anyhow!("replace_toc_edges {}: {e}", doc.identifiant))?;
    Ok((upserted, skipped))
}
