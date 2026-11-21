//! Déménagement d'une chronique LEGI vers sa fiche instrument : commande
//! `move-chronicle`. DILA consolide certains instruments sous le cid du décret qui
//! les publie ; la passe LEGI n'étant pas routée par l'arête `publie`, ces
//! versions atterrissent sur le véhicule et se déménagent à la main, sur liste
//! fermée vérifiée.

use anyhow::{anyhow, Result};

use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Commande `move-chronicle` : déménage la chronique LEGI d'un instrument
/// consolidée par DILA sous le cid du véhicule. Liste fermée de dossiers vérifiés
/// à la main ; `exclude` (`<num_key>@<YYYY-MM-DD>`) garde au véhicule les jumeaux
/// LEGI du dispositif quand le dossier mélange les deux couches. Re-runnable après
/// une retouche DILA du dossier.
pub async fn move_chronicle(vehicle: &str, exclude: &[String]) -> Result<()> {
    let exclusions: Vec<(String, chrono::NaiveDate)> = exclude
        .iter()
        .map(|e| {
            let (num, date) = e
                .split_once('@')
                .ok_or_else(|| anyhow!("exclusion invalide (attendu <num_key>@<date>) : {e}"))?;
            Ok((
                num.to_string(),
                chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                    .map_err(|err| anyhow!("date d'exclusion invalide {e}: {err}"))?,
            ))
        })
        .collect::<Result<_>>()?;

    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let instrument = repo
        .publie_vehicle_instrument_pairs()
        .await?
        .into_iter()
        .find_map(|(v, i)| (v == vehicle).then_some(i))
        .ok_or_else(|| {
            anyhow!("aucune arête publie ne cible {vehicle} — migrer l'identité d'abord")
        })?;
    let (moved, dupes, toc) = repo
        .move_chronicle_articles(vehicle, &instrument, &exclusions)
        .await?;
    let (ranked_texts, _) = repo.refresh_reading_ranks().await?;
    tracing::info!(
        vehicle,
        instrument,
        moved,
        dupes,
        toc,
        ranked_texts,
        "move-chronicle"
    );
    println!(
        "move-chronicle : {moved} versions LEGI {vehicle} → {instrument}, \
         {dupes} doublons purgés, {toc} arêtes TOC."
    );
    Ok(())
}
