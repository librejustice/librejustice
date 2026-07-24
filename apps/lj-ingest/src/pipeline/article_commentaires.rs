//! Commentaires de norme curés (ADR 0212) : liens sortants `kind:"note"`
//! attachés à des articles de loi/traité, depuis un dataset curé sous
//! `<state_dir>/ingest/article-commentaires.json` (règle #17). Premier usage :
//! un billet de cabinet sur l'article 8 CEDH. Aucun corps stocké — que le lien.
//!
//! Chaque entrée `{ text_uid, num_key?, note{title,publisher,url,access,date?} }`
//! devient une ligne `article_commentaire` (`source='web-curated'`), idempotente
//! par checksum. `num_key` absent = commentaire du texte entier.

use std::fs;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use lj_store::repository::DecisionRepository;

use crate::config::Settings;

const SOURCE: &str = "web-curated";

pub async fn seed_article_commentaires() -> Result<()> {
    let settings = Settings::from_env()?;
    let path = settings
        .state_dir
        .join("ingest")
        .join("article-commentaires.json");

    let raw = fs::read_to_string(&path)
        .with_context(|| format!("lecture dataset curé: {}", path.display()))?;
    let entries: Vec<Value> = serde_json::from_str(&raw)
        .with_context(|| format!("dataset article-commentaires invalide: {}", path.display()))?;

    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let existing = repo.article_commentaire_checksums(SOURCE).await?;

    let (mut upserted, mut unchanged) = (0usize, 0usize);
    for entry in &entries {
        let text_uid = entry["text_uid"]
            .as_str()
            .ok_or_else(|| anyhow!("entrée sans text_uid: {entry}"))?;
        let num_key = entry["num_key"].as_str();
        let note = &entry["note"];
        let url = note["url"]
            .as_str()
            .ok_or_else(|| anyhow!("note sans url: {entry}"))?;

        // Bundle jsonb au même format que les commentaires de décision.
        let mut note_out = json!({ "kind": "note", "url": url });
        for k in ["title", "publisher", "access", "date", "author"] {
            if let Some(v) = note.get(k).and_then(Value::as_str) {
                note_out[k] = json!(v);
            }
        }
        let source_fields = json!({ "commentaires": [note_out] });

        let payload = serde_json::to_vec(&source_fields)?;
        let checksum = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(&payload));
        let url_key = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(url.as_bytes()));
        let source_uid = format!(
            "web-curated/{text_uid}/{}#{url_key}",
            num_key.unwrap_or("_")
        );

        if existing.get(&source_uid) == Some(&checksum) {
            unchanged += 1;
            continue;
        }
        repo.upsert_article_commentaire(
            text_uid,
            num_key,
            SOURCE,
            &source_uid,
            &checksum,
            &source_fields,
        )
        .await?;
        upserted += 1;
    }

    tracing::info!(
        entries = entries.len(),
        upserted,
        unchanged,
        "seed_article_commentaires terminé"
    );
    Ok(())
}
