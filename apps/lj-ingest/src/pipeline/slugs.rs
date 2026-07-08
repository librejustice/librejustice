//! Assignation des slugs de textes (ADR 0162) : unique écrivain de
//! `legal_text.slug`. Tout texte reçoit un slug déterministe et immuable —
//! `slugify(title)` tronqué, suffixé de l'uid en collision. La passe tourne en
//! fin de chaque ingest référentiel et s'expose en commande `assign-slugs`
//! (backfill). Idempotente (#7) : elle ne remplit que les `slug NULL`.

use anyhow::{anyhow, Result};
use lj_store::repository::DecisionRepository;
use std::collections::HashSet;

use crate::config::Settings;

/// Longueur max du slug nu (frontière de mot) — les titres de lois/arrêtés
/// font couramment 150-250 chars, l'URL n'a pas à porter tout l'intitulé.
const MAX_SLUG_LEN: usize = 80;

/// Slug candidat d'un titre : `slugify` des codes (ADR 0092), tronqué à
/// [`MAX_SLUG_LEN`] sur un tiret. Titre vide → chaîne vide (l'appelant
/// suffixe l'uid, qui devient le slug entier).
pub fn text_slug(title: &str) -> String {
    let full = lj_extract::legi::slugify_code(title);
    if full.len() <= MAX_SLUG_LEN {
        return full;
    }
    match full[..=MAX_SLUG_LEN].rfind('-') {
        Some(cut) if cut > 0 => full[..cut].to_string(),
        _ => full[..MAX_SLUG_LEN].to_string(),
    }
}

/// Remplit les `slug NULL` : candidats triés par `text_uid` (déterminisme),
/// collision contre l'existant ou le lot → suffixe `-{text_uid minuscules}`.
/// Renvoie le nombre de slugs posés.
pub async fn assign_text_slugs(repo: &DecisionRepository<'_>) -> Result<u64> {
    let pending = repo
        .texts_without_slug()
        .await
        .map_err(|e| anyhow!("texts_without_slug: {e}"))?;
    if pending.is_empty() {
        return Ok(0);
    }
    let mut taken: HashSet<String> = repo
        .existing_text_slugs()
        .await
        .map_err(|e| anyhow!("existing_text_slugs: {e}"))?
        .into_iter()
        .collect();

    let mut assign: Vec<(String, String)> = Vec::with_capacity(pending.len());
    for (uid, title) in pending {
        let bare = text_slug(&title);
        let slug = if !bare.is_empty() && !taken.contains(&bare) {
            bare
        } else {
            let suffixed = if bare.is_empty() {
                uid.to_lowercase()
            } else {
                format!("{bare}-{}", uid.to_lowercase())
            };
            // `text_uid` est unique → le suffixe l'est aussi (#12 : collision
            // résiduelle = hypothèse violée, erreur franche à l'index UNIQUE).
            suffixed
        };
        taken.insert(slug.clone());
        assign.push((uid, slug));
    }

    let mut written = 0u64;
    for chunk in assign.chunks(10_000) {
        written += repo
            .set_text_slugs(chunk)
            .await
            .map_err(|e| anyhow!("set_text_slugs: {e}"))?;
    }
    Ok(written)
}

/// Commande `assign-slugs` : backfill autonome (pool + migrations + passe).
pub async fn assign_slugs() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let n = assign_text_slugs(&repo).await?;
    tracing::info!(written = n, "slugs de textes assignés");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_court_intact_long_tronque_sur_tiret() {
        assert_eq!(text_slug("Code civil"), "code-civil");
        assert_eq!(
            text_slug("Arrêté du 12 janvier 2012"),
            "arrete-du-12-janvier-2012"
        );
        let long = text_slug(
            "LOI n° 79-587 du 11 juillet 1979 relative à la motivation des actes \
             administratifs et à l'amélioration des relations entre l'administration \
             et le public",
        );
        assert!(long.len() <= MAX_SLUG_LEN, "tronqué: {long}");
        assert!(!long.ends_with('-'), "frontière de mot: {long}");
        assert!(long.starts_with("loi-n-79-587-du-11-juillet-1979-relative-a-la-motivation"));
    }
}
