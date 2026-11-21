//! Cross-references and citations of the referential: one `doc_extract` pass
//! over every `legal_article` version feeds `text_case_citation` (norm to
//! decision) and `text_legal_citation` (norm to article, resolved spans, never
//! a self-reference). Written by replacement per `owner_text_uid`; pending
//! case keys are resolved at the end of the pass.

use anyhow::{anyhow, Result};
use chrono::NaiveDate;

use lj_store::repository::DecisionRepository;

use crate::config::Settings;
use lj_extract_rows::{build_declared, owner_rows, Declared, OwnerRows};

/// Versions d'articles par page keyset. Le CPU d'une page part en rayon.
const PAGE: i64 = 2_000;

/// One full extraction: article versions to `text_case_citation` and
/// `text_legal_citation`.
pub async fn extract_text_refs() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Mêmes réglages de session que le reextract décisions : la passe est
    // idempotente (un crash rejoue), les FK ciblent le catalogue lu de la
    // même base.
    conn.batch_execute(
        "SET statement_timeout = 0; \
         SET session_replication_role = replica; \
         SET synchronous_commit = off",
    )
    .await
    .map_err(|e| anyhow!("session setup: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let ctx = super::extract_ctx(&conn).await?;
    // Oracle DILA, hydraté une fois et partagé en lecture par les workers rayon —
    // d'où le `'static`, comme le contexte d'extraction.
    let declared_rows = repo.declared_article_cites().await?;
    let n_declared = declared_rows.len();
    let declared: &'static Declared = Box::leak(Box::new(build_declared(declared_rows)));
    tracing::info!(
        aretes = n_declared,
        cles = declared.len(),
        "declared cross-reference oracle loaded"
    );

    // Versions d'articles, pages keyset groupées par `text_uid` : flush
    // d'un texte quand la page suivante passe au suivant.
    // Sentinelle basse de la keyset (le `text_uid` vide domine la
    // comparaison de tuple ; `NaiveDate::MIN` déborde du DATE Postgres).
    let mut after = (
        String::new(),
        String::new(),
        NaiveDate::from_ymd_opt(1, 1, 1).unwrap(),
    );
    let mut current: Option<(String, OwnerRows)> = None;
    let mut versions = 0usize;
    let mut texts = 0usize;
    let mut spans = (0usize, 0usize); // (cases, cites)
    loop {
        let page = repo
            .legal_article_versions_page((&after.0, &after.1, after.2), PAGE)
            .await?;
        let Some(last) = page.last() else { break };
        after = (last.0.clone(), last.1.clone(), last.2);
        versions += page.len();

        let extracted: Vec<(String, String, NaiveDate, OwnerRows)> =
            tokio::task::spawn_blocking(move || {
                use rayon::prelude::*;
                page.into_par_iter()
                    .map(|(text_uid, num_key, date_debut, texte)| {
                        let rows = owner_rows(
                            &texte,
                            &text_uid,
                            Some(num_key.clone()),
                            Some(date_debut),
                            &ctx.link,
                            &ctx.vocab,
                            &ctx.chrono,
                            declared,
                        );
                        (text_uid, num_key, date_debut, rows)
                    })
                    .collect()
            })
            .await
            .map_err(|e| anyhow!("join extract articles: {e}"))?;

        for (text_uid, _num_key, _date_debut, rows) in extracted {
            match &mut current {
                Some((uid, acc)) if *uid == text_uid => {
                    acc.cases.extend(rows.cases);
                    acc.cites.extend(rows.cites);
                }
                _ => {
                    if let Some((uid, acc)) = current.take() {
                        spans = write_owner(&repo, &uid, acc, spans).await?;
                        texts += 1;
                    }
                    current = Some((text_uid, rows));
                }
            }
        }
        if versions % 100_000 < PAGE as usize {
            tracing::info!(versions, texts, "extract_text_refs running");
        }
    }
    if let Some((uid, acc)) = current.take() {
        spans = write_owner(&repo, &uid, acc, spans).await?;
        texts += 1;
    }

    let resolved = repo
        .resolve_pending_text_case_citations()
        .await
        .map_err(|e| anyhow!("resolve_pending_text_case_citations: {e}"))?;

    tracing::info!(
        versions,
        texts,
        case_spans = spans.0,
        cite_spans = spans.1,
        resolved,
        "extract_text_refs"
    );
    Ok(())
}

async fn write_owner(
    repo: &DecisionRepository<'_>,
    text_uid: &str,
    acc: OwnerRows,
    spans: (usize, usize),
) -> Result<(usize, usize)> {
    repo.replace_text_case_citations(text_uid, &acc.cases)
        .await
        .map_err(|e| anyhow!("replace_text_case_citations {text_uid}: {e}"))?;
    repo.replace_text_legal_citations(text_uid, &acc.cites)
        .await
        .map_err(|e| anyhow!("replace_text_legal_citations {text_uid}: {e}"))?;
    Ok((spans.0 + acc.cases.len(), spans.1 + acc.cites.len()))
}
