//! Commentaires de norme curés : liens sortants `kind:"note"` attachés à des
//! articles de loi/traité, depuis un dataset curé sous
//! `<state_dir>/ingest/article-commentaires.json`. Aucun corps stocké — que le
//! lien.
//!
//! Chaque entrée `{ slug, num_key?, note{title,publisher,url,access,date?} }`
//! devient une ligne `article_commentaire` (`source='web-curated'`), idempotente
//! par checksum. `num_key` absent = commentaire du texte entier.
//!
//! L'ancre stockée est le **slug** d'URL, pas le `text_uid` : ce dernier est une
//! identité physique de source qui bouge, et le traité CEDH a déjà changé de
//! `text_uid` sous ses commentaires.
//!
//! Deux régimes : à l'**écriture**, une ancre morte échoue franchement — un
//! commentaire qui ne s'affichera jamais est un défaut de dataset ; en **filet
//! de dérive** hebdomadaire, elle se compte et se `warn`.

use std::fs;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use lj_store::repository::DecisionRepository;

use crate::config::Settings;

const SOURCE: &str = "web-curated";

/// Issue d'une passe : ce que le filet hebdo publie comme compteurs de dérive.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CommentaireSeedReport {
    pub entries: usize,
    pub upserted: usize,
    pub unchanged: usize,
    pub tombstoned: u64,
    /// Ancres mortes (slug inconnu, article absent) — toujours 0 en mode strict, la
    /// passe ayant déjà échoué dessus.
    pub dangling: usize,
}

/// Commande CLI : mode strict, toute ancre morte est une erreur.
pub async fn seed_article_commentaires() -> Result<()> {
    let report = run(true).await?;
    tracing::info!(
        entries = report.entries,
        upserted = report.upserted,
        unchanged = report.unchanged,
        tombstoned = report.tombstoned,
        "seed_article_commentaires done"
    );
    Ok(())
}

/// Filet de dérive hebdo : rejoue la passe sans avorter sur une ancre morte.
/// Absence du dataset = rien à réconcilier (machine sans données curées), pas un
/// échec.
pub async fn reconcile_article_commentaires() -> Result<CommentaireSeedReport> {
    let report = run(false).await?;
    if report.dangling > 0 {
        tracing::warn!(
            dangling = report.dangling,
            "curated comments: dead anchors, dataset to fix"
        );
    }
    if report.tombstoned > 0 {
        tracing::warn!(
            tombstoned = report.tombstoned,
            "curated comments: rows outside the dataset removed"
        );
    }
    Ok(report)
}

async fn run(strict: bool) -> Result<CommentaireSeedReport> {
    let settings = Settings::from_env()?;
    let path = settings
        .state_dir
        .join("ingest")
        .join("article-commentaires.json");

    if !strict && !path.exists() {
        return Ok(CommentaireSeedReport::default());
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("lecture dataset curé: {}", path.display()))?;
    let entries: Vec<Value> = serde_json::from_str(&raw)
        .with_context(|| format!("dataset article-commentaires invalide: {}", path.display()))?;

    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let existing = repo.article_commentaire_checksums(SOURCE).await?;

    let mut report = CommentaireSeedReport {
        entries: entries.len(),
        ..Default::default()
    };
    let mut seen: Vec<String> = Vec::with_capacity(entries.len());
    for entry in &entries {
        let slug = entry["slug"]
            .as_str()
            .ok_or_else(|| anyhow!("entrée sans slug: {entry}"))?;
        let num_key = entry["num_key"].as_str();
        let note = &entry["note"];
        let url = note["url"]
            .as_str()
            .ok_or_else(|| anyhow!("note sans url: {entry}"))?;

        // Le rendu lit par (slug, num_key) : une ancre qui ne désigne aucun article est
        // un commentaire invisible. Strict ⇒ erreur ; filet ⇒ compté, l'entrée est
        // laissée de côté.
        if let Some(dead) = dangling_reason(&repo, slug, num_key).await? {
            if strict {
                anyhow::bail!("{dead}");
            }
            report.dangling += 1;
            continue;
        }

        // Bundle jsonb au même format que les commentaires de décision.
        let mut note_out = json!({ "kind": "note", "url": url });
        for k in ["title", "publisher", "date", "author"] {
            if let Some(v) = note.get(k).and_then(Value::as_str) {
                note_out[k] = json!(v);
            }
        }
        let source_fields = json!({ "commentaires": [note_out] });

        let payload = serde_json::to_vec(&source_fields)?;
        let checksum = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(&payload));
        let url_key = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(url.as_bytes()));
        // Identité bâtie sur le slug : re-clé du référentiel ⇒ même entrée curée.
        let source_uid = format!("web-curated/{slug}/{}#{url_key}", num_key.unwrap_or("_"));
        seen.push(source_uid.clone());

        if existing.get(&source_uid) == Some(&checksum) {
            report.unchanged += 1;
            continue;
        }
        repo.upsert_article_commentaire(
            slug,
            num_key,
            SOURCE,
            &source_uid,
            &checksum,
            &source_fields,
        )
        .await?;
        report.upserted += 1;
    }

    // Le dataset est la source de vérité : ce qui n'y est plus sort de la fiche.
    // Une entrée écartée pour ancre morte n'est pas dans `seen`, donc ses lignes
    // sont retirées.
    report.tombstoned = repo
        .tombstone_absent_article_commentaires(SOURCE, &seen)
        .await?;

    Ok(report)
}

/// `Some(raison)` si l'ancre ne désigne aucun article affichable.
async fn dangling_reason(
    repo: &DecisionRepository<'_>,
    slug: &str,
    num_key: Option<&str>,
) -> Result<Option<String>> {
    let Some((text_uid, _, _, _)) = repo.referential_ref(slug, None).await? else {
        return Ok(Some(format!(
            "slug de texte inconnu au référentiel: {slug}"
        )));
    };
    if let Some(num) = num_key {
        if repo.law_article_versions(&text_uid, num).await?.is_empty() {
            return Ok(Some(format!(
                "aucun article {num} dans le texte {slug} ({text_uid})"
            )));
        }
    }
    Ok(None)
}
