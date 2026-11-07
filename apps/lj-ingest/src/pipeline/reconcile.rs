//! Filet unique de dérive : la commande `lj-ingest reconcile` enchaîne TOUS les
//! filets du régime « dérivé à l'écriture » — rôles de textes + dénorm articles,
//! arrays légaux dénormalisés, liens/citations pendants (chronologie,
//! décision→décision, texte→décision). Chaque brique est idempotente ; en régime
//! les écrivains at-write ne laissent rien fuir et chaque passe est ~no-op. Un
//! compteur > 0 = fuite d'un écrivain à diagnostiquer.
//!
//! Le volet parties (relink + annuaire) vit dans `parties-relink`, collé au reload
//! hebdo du registre CNB dont il dépend. Tout nouveau filet de dérive DOIT
//! s'enregistrer ici.
//!
//! One exception to the weekly cadence: the two citation nets also run at the
//! tail of every ingest pass, through `resolve_pending_citations`.

use std::future::Future;
use std::time::Instant;

use anyhow::{anyhow, Result};
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Encadre un filet de son nom et de sa durée. Chaque passe ne logue que son
/// compte final, et plusieurs tiennent des minutes sur une seule requête : sans
/// ces bornes, leur silence ne se distingue pas d'un blocage.
async fn filet<T, E>(nom: &'static str, f: impl Future<Output = Result<T, E>>) -> Result<T, E> {
    tracing::info!(filet = nom, "drift net: start");
    let started = Instant::now();
    let out = f.await;
    tracing::info!(
        filet = nom,
        elapsed_s = started.elapsed().as_secs(),
        ok = out.is_ok(),
        "drift net: end"
    );
    out
}

async fn pending_citations(repo: &DecisionRepository<'_>) -> Result<()> {
    let resolved = filet(
        "citations jurisprudence",
        repo.resolve_pending_case_citations(),
    )
    .await?;
    tracing::info!(resolved, "pending case-law citations resolved");
    let resolved = filet(
        "citations texte→jurisprudence",
        repo.resolve_pending_text_case_citations(),
    )
    .await?;
    tracing::info!(resolved, "pending text-to-case-law citations resolved");
    Ok(())
}

/// The two citation nets alone, which the ingest pass runs at its tail: a pass
/// that dies mid-way leaves its citations pending until the weekly `reconcile`.
pub async fn resolve_pending_citations() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    pending_citations(&DecisionRepository::new(&conn)).await
}

/// Commande `reconcile` : rejoue tous les filets de dérive. Idempotent.
pub async fn reconcile_all() -> Result<()> {
    // Rôles des textes + dénorm articles (pleine table, dérive attendue ~0).
    filet("rôles de textes", super::backfill_text_roles()).await?;
    // Arrays légaux dénormalisés (garde IS DISTINCT FROM, warn si dérive > 0).
    filet(
        "arrays légaux",
        crate::citations_maintenance::resync_arrays(),
    )
    .await?;

    // Liens/citations pendants : rejoue les résolveurs pleine-table. En régime
    // ils sont résolus à l'écriture (côté source dans la transaction du lot,
    // côté cible par la sonde inversée `pipeline::attract`).
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let resolved = filet(
        "chronologie pendante",
        repo.resolve_pending_decision_links(),
    )
    .await?;
    tracing::info!(resolved, "pending chronology links resolved");
    pending_citations(&repo).await?;

    // Liens conseils devenus ambigus à date : le delta du reload CNB ne voit que le
    // voisinage des identités ajoutées, ce filet plein rattrape l'ambiguïté de stock.
    // Dé-liage puis re-résolution des SEULES clés libérées — nul en régime,
    // proportionnel à la dérive sinon. Une clé restée ambiguë demeure pendante ; une
    // clé dont le dé-liage révèle le bon porteur se relie ici.
    let freed = filet("conseils ambigus", repo.cnb_unlink_ambiguous()).await?;
    let relinked = filet(
        "conseils re-résolus",
        repo.resolve_pending_counsel_by_keys(&freed),
    )
    .await?;
    tracing::info!(
        keys = freed.len(),
        relinked,
        "ambiguous dated council links unlinked then re-resolved"
    );

    // A party named by initials, or a natural person by a name carrying one, links to
    // nothing: resolution no longer sets these links, this net removes those the stock holds.
    let unnamed = filet("parties sans nom", repo.party_unlink_unnamed()).await?;
    if unnamed > 0 {
        tracing::warn!(unnamed, "party links without a name unlinked");
        let annuaire = repo.refresh_annuaire().await?;
        tracing::info!(annuaire, "annuaire: counters refreshed");
    }

    // Commentaires de norme curés : le dataset sous `state_dir` est la source de
    // vérité, rejoué ici pour que le retrait d'une entrée et la correction d'une ancre
    // morte se propagent sans intervention. L'ancre étant le slug, la dérive attendue
    // est nulle ; `dangling > 0` = dataset à corriger.
    let report = filet(
        "commentaires de norme",
        super::reconcile_article_commentaires(),
    )
    .await?;
    tracing::info!(
        entries = report.entries,
        upserted = report.upserted,
        tombstoned = report.tombstoned,
        dangling = report.dangling,
        "curated norm comments reconciled"
    );

    // Versions d'article mal situées : sentinelle doublonnée, version qu'aucune
    // suivante ne ferme. L'upsert situe la version entre ses voisines à
    // l'écriture, donc cette passe est un no-op en régime — et ce qu'elle répare,
    // elle le signale en échec plutôt qu'en compteur, parce que le temps qu'elle
    // couvre, la lecture a servi une version sur deux sans le dire.
    let (purgees, fermees) = filet(
        "versions d'article mal situées",
        repo.situer_versions_articles(),
    )
    .await?;
    if purgees + fermees > 0 {
        return Err(anyhow!(
            "versions d'article mal situées et réparées ici : {purgees} sentinelles \
             doublonnées, {fermees} versions qu'aucune suivante ne fermait — \
             l'écriture aurait dû s'en charger, un écrivain a fui"
        ));
    }
    tracing::info!("article versions: nothing to place");
    Ok(())
}
