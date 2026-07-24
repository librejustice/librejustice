//! ADDE (`adde-association.org`) → commentaires doctrine web (ADR 0204, plan
//! généralisé) : une analyse de l'ADDE = un **lien sortant** `kind: "note"`
//! rattaché à la ou aux décisions qu'elle commente. Premier canal doctrine web
//! (réutilisable pour GISTI et d'autres éditeurs…).
//!
//! Déroulé : énumération REST des posts catégorie *jurisprudence* → parse pur
//! de la citation du titre (juridiction, date, n°s de dossier) → résolution par
//! (dossier, date) → upsert `source = 'adde'`, `source_uid =
//! adde/<slug>#<public_id>` (une ligne par (article × décision) : une décision
//! jointe matchée par plusieurs n° ne donne qu'une ligne, et deux décisions
//! homonymes sur (n°, date) gardent chacune la leur). Aucun corps stocké —
//! seulement le lien. Idempotence (#7) par checksum du bundle. Volume faible :
//! pas de manifeste ni de cache.

use std::collections::HashSet;

use anyhow::{anyhow, Result};

use lj_core::parsing::{build_adde_source_fields, parse_adde_title};
use lj_sources::adde;
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

#[derive(Default)]
struct Stats {
    posts: usize,
    hors_perimetre: usize,
    upserted: usize,
    unchanged: usize,
    orphans: usize,
}

/// Sync ADDE : rattache les analyses de jurisprudence de l'ADDE en commentaires
/// doctrine web des décisions déjà en base.
pub async fn sync_adde() -> Result<()> {
    let settings = Settings::from_env()?;

    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let existing = repo.adde_checksums().await?;

    let client = adde::http_client();
    let posts = adde::fetch_jurisprudence_posts(&client).await?;
    tracing::info!(
        posts = posts.len(),
        bundles_en_base = existing.len(),
        "sync_adde démarré"
    );

    let mut stats = Stats::default();
    for post in &posts {
        stats.posts += 1;
        let Some(citation) = parse_adde_title(&post.title) else {
            // Post de la catégorie sans titre-citation exploitable — hors périmètre.
            stats.hors_perimetre += 1;
            tracing::debug!(title = %post.title, "post ADDE sans citation, ignoré");
            continue;
        };

        let source_fields = build_adde_source_fields(&post.link, &post.date);
        let payload = serde_json::to_vec(&source_fields)?;
        let checksum = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(&payload));
        let slug = post
            .link
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default();

        let mut seen_ids: HashSet<i64> = HashSet::new();
        for docket in &citation.dockets {
            let matches = repo
                .decisions_by_docket_date(docket, &citation.date_iso)
                .await?;
            for (id, public_id) in matches {
                if !seen_ids.insert(id) {
                    continue;
                }
                let source_uid = format!("adde/{slug}#{public_id}");
                if existing.get(&source_uid) == Some(&checksum) {
                    stats.unchanged += 1;
                    continue;
                }
                repo.upsert_decision_source(id, &source_uid, &checksum, "json", &source_fields)
                    .await?;
                stats.upserted += 1;
            }
        }
        if seen_ids.is_empty() {
            // Décision commentée absente du stock (récente/juridiction non
            // ingérée) — durable, se rattachera quand la décision arrivera.
            stats.orphans += 1;
            tracing::warn!(title = %post.title, "analyse ADDE sans décision en base");
        }
    }

    tracing::info!(
        posts = stats.posts,
        hors_perimetre = stats.hors_perimetre,
        upserted = stats.upserted,
        unchanged = stats.unchanged,
        orphans = stats.orphans,
        "sync_adde terminé"
    );
    Ok(())
}
