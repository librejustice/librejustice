//! Curation d'identité de fiches par dataset : commande `rename-texts`. Lit un
//! JSONL `{from, to?, jurisdiction?, collapse?, delete?}` et applique, ligne à
//! ligne, l'une des trois opérations — re-frappe d'uid en cascade
//! ([`rename_legal_text`]), fusion dans une jumelle qui porte déjà le contenu
//! ([`collapse_legal_text`]), suppression d'une fiche sans objet
//! ([`delete_legal_text`]) —, puis rafraîchit la dénormalisation articles et
//! les rangs de lecture une fois pour le lot. Les citations des décisions
//! suivent au drain `reextract-fields --full --field legal_references`.
//! Résumable : une fiche déjà traitée est sautée.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Une ligne du dataset de re-frappe.
#[derive(Debug, Deserialize)]
struct RenameRecord {
    from: String,
    /// Cible : absente pour une suppression, requise sinon.
    to: Option<String>,
    /// Remplace la colonne `jurisdiction` si fournie ET différente (cas du
    /// re-routage `INTL` d'un instrument bilatéral mal classé).
    jurisdiction: Option<String>,
    /// `to` porte déjà le même contenu : fusion au lieu de renommage — le
    /// contenu du doublon est jeté, ses entrantes déménagent.
    #[serde(default)]
    collapse: bool,
    /// La fiche n'a pas d'objet et n'a pas de jumelle : elle disparaît.
    #[serde(default)]
    delete: bool,
}

pub async fn rename_texts(dataset: &Path) -> Result<()> {
    let raw = std::fs::read_to_string(dataset)
        .with_context(|| format!("lecture du dataset {}", dataset.display()))?;
    let records: Vec<RenameRecord> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).with_context(|| format!("ligne invalide : {l}")))
        .collect::<Result<_>>()?;

    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let (mut renamed, mut collapsed, mut deleted, mut skipped) = (0u64, 0u64, 0u64, 0u64);
    let mut articles = 0u64;
    for rec in &records {
        let target = || -> Result<&str> {
            rec.to
                .as_deref()
                .ok_or_else(|| anyhow!("{} : `to` requis hors suppression", rec.from))
        };
        let (done, verb) = if rec.delete {
            (repo.delete_legal_text(&rec.from).await?, "supprimée")
        } else if rec.collapse {
            (
                repo.collapse_legal_text(&rec.from, target()?).await?,
                "fusionnée",
            )
        } else {
            (
                repo.rename_legal_text(&rec.from, target()?, rec.jurisdiction.as_deref())
                    .await?,
                "renommée",
            )
        };
        match done {
            Some(counts) => {
                if rec.delete {
                    deleted += 1;
                } else if rec.collapse {
                    collapsed += 1;
                } else {
                    renamed += 1;
                    articles += counts.articles;
                }
                tracing::info!(from = rec.from, to = rec.to, ?counts, "fiche {verb}");
            }
            None => skipped += 1,
        }
    }
    let denorm = repo.refresh_article_denorm(None).await?;
    let (ranked_texts, _) = repo.refresh_reading_ranks().await?;
    tracing::info!(
        renamed,
        collapsed,
        deleted,
        skipped,
        articles,
        denorm,
        ranked_texts,
        "rename-texts"
    );
    println!(
        "rename-texts : {renamed} fiches renommées ({articles} articles suivis), \
         {collapsed} fusionnées, {deleted} supprimées, {skipped} déjà faites ; \
         {denorm} dénormalisés.\n\
         Suite : drain reextract-fields --full --field legal_references (citations décisions)."
    );
    Ok(())
}
