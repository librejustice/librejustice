//! Renvois et citations des corps du référentiel (ADR 0196 §5 / 0217) : UNE
//! passe `doc_extract` sur les corps ingérés — `legal_text.body` et toutes
//! les versions `legal_article` à corps — alimente `text_case_citation`
//! (norme→décision) et `text_legal_citation` (norme→article, spans résolus,
//! jamais l'auto-renvoi). Écriture par remplacement au grain
//! `owner_text_uid` (corps + articles d'un texte fusionnés au flush) ;
//! résolution des clés cases pendantes en fin de passe.

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use chrono::NaiveDate;

use lj_store::repository::{DecisionRepository, TextCaseCitationRow, TextLegalCitationRow};

use crate::config::Settings;

/// Versions d'articles par page keyset (émetteurs les plus nombreux : ~3,4 M
/// de versions à corps). Le CPU d'une page part en rayon.
const PAGE: i64 = 2_000;

/// Spans extraits d'UN corps émetteur, prêts à écrire.
struct OwnerRows {
    cases: Vec<TextCaseCitationRow>,
    cites: Vec<TextLegalCitationRow>,
}

/// Une extraction complète : corps monolithiques + versions d'articles →
/// `text_case_citation` + `text_legal_citation`.
pub async fn extract_text_refs() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
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
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let ctx = super::extract_ctx(&conn).await?;

    // 1. Corps monolithiques, bufferisés par texte : un texte peut émettre
    // depuis son corps ET ses articles (BOFiP : préambule + §§) — le writer
    // remplaçant au grain `owner_text_uid`, les deux flux se fusionnent au
    // flush du texte.
    let bodies = repo.legal_text_bodies().await?;
    let n_bodies = bodies.len();
    let mut body_rows: HashMap<String, OwnerRows> = tokio::task::spawn_blocking(move || {
        use rayon::prelude::*;
        bodies
            .into_par_iter()
            .map(|(text_uid, body)| {
                let rows = owner_rows(&body, &text_uid, None, None, ctx);
                (text_uid, rows)
            })
            .collect()
    })
    .await
    .map_err(|e| anyhow!("join extract bodies: {e}"))?;

    // 2. Versions d'articles, pages keyset groupées par `text_uid` : flush
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
                            ctx,
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
                        spans = flush_owner(&repo, &uid, acc, &mut body_rows, spans).await?;
                        texts += 1;
                    }
                    current = Some((text_uid, rows));
                }
            }
        }
        if versions % 100_000 < PAGE as usize {
            tracing::info!(versions, texts, "extract_text_refs en cours");
        }
    }
    if let Some((uid, acc)) = current.take() {
        spans = flush_owner(&repo, &uid, acc, &mut body_rows, spans).await?;
        texts += 1;
    }
    // Textes à corps sans articles (circulaires…) : restés dans le buffer.
    for (uid, acc) in body_rows.drain() {
        spans = write_owner(&repo, &uid, acc, spans).await?;
        texts += 1;
    }

    let resolved = repo
        .resolve_pending_text_case_citations()
        .await
        .map_err(|e| anyhow!("resolve_pending_text_case_citations: {e}"))?;

    tracing::info!(
        bodies = n_bodies,
        versions,
        texts,
        case_spans = spans.0,
        cite_spans = spans.1,
        resolved,
        "extract_text_refs"
    );
    Ok(())
}

/// Flush d'un texte émetteur : fusionne ses éventuels spans de corps
/// (préambule) puis écrit les deux tables.
async fn flush_owner(
    repo: &DecisionRepository<'_>,
    text_uid: &str,
    mut acc: OwnerRows,
    body_rows: &mut HashMap<String, OwnerRows>,
    spans: (usize, usize),
) -> Result<(usize, usize)> {
    if let Some(body) = body_rows.remove(text_uid) {
        acc.cases.extend(body.cases);
        acc.cites.extend(body.cites);
    }
    write_owner(repo, text_uid, acc, spans).await
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

/// Les deux flux d'UN corps émetteur (offsets codepoints sur le texte
/// émetteur, convention 0143). Renvois : seulement les cibles résolues
/// (ADR 0217), jamais l'auto-renvoi (cible = l'émetteur lui-même).
fn owner_rows(
    texte: &str,
    owner_text_uid: &str,
    owner_num_key: Option<String>,
    owner_date_debut: Option<NaiveDate>,
    ctx: &super::ExtractCtx,
) -> OwnerRows {
    let docx = lj_extract::compiled::doc_extract(texte, &ctx.vocab, &ctx.link, &ctx.chrono, None);
    let cases = docx
        .cases
        .into_iter()
        .map(|c| TextCaseCitationRow {
            owner_num_key: owner_num_key.clone(),
            owner_date_debut,
            char_start: c.char_start as i32,
            char_end: c.char_end as i32,
            target_ref: c.target_ref,
        })
        .collect();
    let cites = docx
        .citations
        .into_iter()
        .filter_map(|c| {
            let (ref_text_uid, ref_num_key) = match c.target.ref_text_uid {
                Some(uid) => (uid, c.target.ref_num_key),
                // Article NU dans un corps du référentiel (« en application
                // de l'article 21-2 » dans le code civil) : la cible est le
                // texte émetteur lui-même quand il possède l'article —
                // renvoi interne, la convention rédactionnelle des codes.
                // L'orphelin génitif (« du présent code », « de la loi
                // susvisée ») n'est PAS nu : il reste écarté.
                None => {
                    let ak = c.article_key.filter(|_| c.bare)?;
                    if !ctx.link.has_article(owner_text_uid, &ak) {
                        return None;
                    }
                    let num = ctx.link.num_key_for(owner_text_uid, Some(&ak));
                    (owner_text_uid.to_string(), num)
                }
            };
            if ref_text_uid == owner_text_uid && ref_num_key == owner_num_key {
                return None;
            }
            Some(TextLegalCitationRow {
                owner_num_key: owner_num_key.clone(),
                owner_date_debut,
                char_start: c.char_start as i32,
                char_end: c.char_end as i32,
                ref_text_uid,
                ref_num_key,
            })
        })
        .collect();
    OwnerRows { cases, cites }
}
