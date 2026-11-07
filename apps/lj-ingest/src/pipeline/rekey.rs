//! Rebasage one-shot des clés d'article vers la clé publique slug
//! (`lj_norm::article_key`) : mapping Rust, fusion des doublons PK, rebasage
//! colonne par colonne, dédup TOC, resync des composites décision/chunks depuis
//! `legal_citation`. Idempotent : `article_key` est un point fixe.

use anyhow::{anyhow, Result};
use lj_store::repository::{DecisionRepository, StarConflict};

use crate::config::Settings;

/// Colonnes qui portent une clé d'article, hors blobs `legal_citation.spans`.
const KEY_COLUMNS: [(&str, &str); 10] = [
    ("legal_article", "num_key"),
    ("legal_article_usage", "num_key"),
    ("article_scope", "num_key"),
    ("article_commentaire", "num_key"),
    ("legal_toc_edge", "child_num_key"),
    ("legal_link", "owner_num_key"),
    ("legal_link", "target_num_key"),
    ("text_legal_citation", "owner_num_key"),
    ("text_legal_citation", "ref_num_key"),
    ("text_case_citation", "owner_num_key"),
];

/// Strips the Légifrance asterisk from the article keys in the database,
/// carrying columns and citation blobs included; the target key comes from
/// `lj_norm::article_key`. **Dry run by default**: without `--applique`, the
/// pass returns its merge plan and writes nothing. Idempotent.
pub async fn destar_article_keys(applique: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let pairs: Vec<(String, String)> = repo
        .starred_article_keys()
        .await?
        .into_iter()
        .filter_map(|old| {
            let new = lj_norm::article_key::article_key(&old);
            (new != old).then_some((old, new))
        })
        .collect();
    if let Some((old, _)) = pairs.iter().find(|(_, new)| new.is_empty()) {
        anyhow::bail!("la clé `{old}` ne laisse rien une fois désétoilée");
    }
    if pairs.is_empty() {
        println!("destar: no starred key in the database.");
        return Ok(());
    }
    repo.create_rekey_map(&pairs).await?;
    println!("destar: {} keys to unstar.", pairs.len());
    for (table, column) in KEY_COLUMNS {
        let n = repo.count_rekey_column(table, column).await?;
        if n > 0 {
            println!("  {table}.{column}: {n} rows");
        }
    }
    println!(
        "  legal_citation.spans: {} decisions",
        repo.count_rekey_citation_spans().await?
    );

    let conflicts = repo.star_pk_conflicts().await?;
    let losers = versions_a_supprimer(&conflicts)?;
    for c in &conflicts {
        let perdante = losers
            .iter()
            .any(|(u, k, d)| *u == c.text_uid && *k == c.num_key && *d == c.date_debut);
        println!(
            "  {} {} -> {} [{} .. {}] {} {}",
            c.text_uid,
            c.num,
            c.new_key,
            c.date_debut,
            c.date_fin.as_deref().unwrap_or("ouvert"),
            c.status,
            if perdante { "DELETED" } else { "kept" }
        );
    }

    if !applique {
        println!("destar: dry-run, nothing written. `--applique` to run it.");
        return Ok(());
    }

    conn.batch_execute("BEGIN")
        .await
        .map_err(|e| anyhow!("begin: {e}"))?;
    let ecriture = async {
        for (table, n) in repo.drop_article_versions(&losers).await? {
            tracing::info!(table, lignes = n, "rows of the losing version");
        }
        let ranks = repo.invalidate_rekeyed_reading_ranks().await?;
        for (table, column) in KEY_COLUMNS {
            let n = repo.rekey_column(table, column).await?;
            tracing::info!(table, column, lignes = n, "column unstarred");
        }
        let decisions = repo.rekey_citation_spans().await?;
        tracing::info!(decisions, "citation spans unstarred");
        let toc = repo.rekey_dedup_toc_edges().await?;
        tracing::info!(toc, ranks, "TOC edges deduplicated, orders dirtied");
        Ok::<u64, anyhow::Error>(decisions)
    }
    .await;
    let decisions = match ecriture {
        Ok(d) => {
            conn.batch_execute("COMMIT")
                .await
                .map_err(|e| anyhow!("commit: {e}"))?;
            d
        }
        Err(e) => {
            let _ = conn.batch_execute("ROLLBACK").await;
            return Err(e);
        }
    };

    let (textes, _) = repo.refresh_reading_ranks().await?;
    tracing::info!(textes, "reading orders recomputed");

    let Some((_, max_id)) = repo.decision_id_bounds().await? else {
        return Ok(());
    };
    const BATCH: i64 = 20_000;
    let (mut d_total, mut c_total) = (0i64, 0i64);
    let mut lo = 0i64;
    while lo <= max_id {
        let (d, c) = repo.resync_legal_arrays_range(lo, lo + BATCH).await?;
        d_total += d;
        c_total += c;
        lo += BATCH;
    }
    println!(
        "destar : {} clés, {} versions fusionnées, {decisions} décisions, \
         composites {d_total}/{c_total}.",
        pairs.len(),
        losers.len()
    );
    Ok(())
}

/// Versions à supprimer pour que le désétoilage passe la PK : les intervalles
/// dégénérés d'abord, puis ceux qui chevauchent une autre version de la clé
/// cible. Un groupe qui ne laisse pas exactement une survivante est une erreur
/// dure.
fn versions_a_supprimer(conflicts: &[StarConflict]) -> Result<Vec<(String, String, String)>> {
    let mut groupes: std::collections::BTreeMap<(&str, &str, &str), Vec<&StarConflict>> =
        std::collections::BTreeMap::new();
    for c in conflicts {
        groupes
            .entry((&c.text_uid, &c.new_key, &c.date_debut))
            .or_default()
            .push(c);
    }
    let mut losers = Vec::new();
    for ((text_uid, new_key, date_debut), rows) in groupes {
        let mut restantes = rows;
        for critere in [
            |c: &StarConflict| c.degenerate,
            |c: &StarConflict| c.overlaps,
        ] {
            if restantes.len() == 1 {
                break;
            }
            let gardees: Vec<&StarConflict> =
                restantes.iter().copied().filter(|c| !critere(c)).collect();
            if gardees.is_empty() {
                break;
            }
            for c in &restantes {
                if critere(c) {
                    losers.push((c.text_uid.clone(), c.num_key.clone(), c.date_debut.clone()));
                }
            }
            restantes = gardees;
        }
        if restantes.len() != 1 {
            anyhow::bail!(
                "{text_uid} {new_key} {date_debut} : {} versions en collision ({}), \
                 aucune règle ne les sépare",
                restantes.len(),
                restantes
                    .iter()
                    .map(|c| c.num.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
    Ok(losers)
}

/// One-shot rebase of the `num_key`s onto the IDENTITY key:
/// `legal_article.num_key` (+ `legal_link.owner_num_key`, which follows it) and
/// `legal_toc_edge.child_num_key`, recomputed from the source `num`/`label` by
/// `lj_norm::article_key::identity_key`. A row whose target PK is already taken
/// is skipped and logged, and keeps its old key.
/// Idempotent: `identity_key` is a fixed point.
pub async fn rekey_identity_keys() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    const PAGE: i64 = 50_000;
    let (mut scanned, mut updated, mut skipped, mut links) = (0u64, 0u64, 0u64, 0u64);
    let mut after_id = 0i64;
    loop {
        let page = repo.article_identity_page(after_id, PAGE).await?;
        let Some(last) = page.last() else { break };
        after_id = last.0;
        scanned += page.len() as u64;
        // Lignes à rebaser, dédupées sur la cible PK à l'intérieur du lot : la
        // première gagne, la seconde est sautée.
        let mut targets = std::collections::HashSet::new();
        let (mut rows, mut new_keys) = (Vec::new(), Vec::new());
        for row in page {
            let new_key = lj_norm::article_key::identity_key(&row.2);
            if new_key == row.3 {
                continue;
            }
            if targets.insert((row.1.clone(), new_key.clone(), row.4.clone())) {
                rows.push(row);
                new_keys.push(new_key);
            } else {
                skipped += 1;
                tracing::warn!(
                    id = row.0,
                    num = %row.2,
                    "duplicate target in the batch, key kept"
                );
            }
        }
        if !rows.is_empty() {
            let (u, s, l) = repo.apply_article_identity_keys(&rows, &new_keys).await?;
            updated += u;
            skipped += s;
            links += l;
        }
        if scanned % 500_000 == 0 {
            tracing::info!(
                scanned,
                updated,
                skipped,
                links,
                "identity rekey (articles)"
            );
        }
    }
    tracing::info!(
        scanned,
        updated,
        skipped,
        links,
        "identity rekey: articles done"
    );

    let (mut toc_scanned, mut toc_updated) = (0u64, 0u64);
    let mut after: Option<(String, i32)> = None;
    loop {
        let page = repo
            .toc_identity_page(after.as_ref().map(|(o, s)| (o.as_str(), *s)), PAGE)
            .await?;
        let Some(last) = page.last() else { break };
        after = Some((last.0.clone(), last.1));
        toc_scanned += page.len() as u64;
        let (mut owners, mut seqs, mut keys) = (Vec::new(), Vec::new(), Vec::new());
        for (owner, seq, label, old_key) in &page {
            let new_key = lj_norm::article_key::identity_key(label);
            if new_key != *old_key {
                owners.push(owner.as_str());
                seqs.push(*seq);
                keys.push(new_key);
            }
        }
        if !owners.is_empty() {
            toc_updated += repo.apply_toc_identity_keys(&owners, &seqs, &keys).await?;
        }
    }
    tracing::info!(toc_scanned, toc_updated, "identity rekey: TOC edges done");
    println!(
        "rekey-identity : {updated} articles rebasés ({skipped} sautés, {links} liens suivis), \
         {toc_updated} arêtes TOC."
    );
    Ok(())
}

pub async fn rekey_article_keys() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let keys = repo.distinct_article_keys().await?;
    let pairs: Vec<(String, String)> = keys
        .into_iter()
        .filter_map(|old| {
            let new = lj_norm::article_key::article_key(&old);
            (new != old).then_some((old, new))
        })
        .collect();
    tracing::info!(a_rebaser = pairs.len(), "article key mapping");
    if pairs.is_empty() {
        tracing::info!("keys already in the public alphabet, nothing to do");
        return Ok(());
    }
    repo.create_rekey_map(&pairs).await?;

    let (articles_fusionnes, liens_fusionnes) = repo.rekey_merge_duplicate_articles().await?;
    tracing::info!(articles_fusionnes, liens_fusionnes, "PK duplicates merged");

    for (table, column) in [
        ("legal_article", "num_key"),
        ("legal_toc_edge", "child_num_key"),
        ("legal_link", "owner_num_key"),
        ("legal_link", "target_num_key"),
        ("text_case_citation", "owner_num_key"),
    ] {
        let n = repo.rekey_column(table, column).await?;
        tracing::info!(table, column, lignes = n, "column rebased");
    }
    let n = repo.rekey_citation_spans().await?;
    tracing::info!(decisions = n, "legal_citation spans rebased");

    let toc_dedup = repo.rekey_dedup_toc_edges().await?;
    tracing::info!(toc_dedup, "TOC edges deduplicated");

    let Some((_, max_id)) = repo.decision_id_bounds().await? else {
        return Ok(());
    };
    const BATCH: i64 = 20_000;
    let (mut d_total, mut c_total) = (0i64, 0i64);
    let mut lo = 0i64;
    while lo <= max_id {
        let (d, c) = repo.resync_legal_arrays_range(lo, lo + BATCH).await?;
        d_total += d;
        c_total += c;
        lo += BATCH;
        if (lo / BATCH) % 25 == 0 {
            tracing::info!(lo, max_id, d_total, c_total, "resync composites");
        }
    }
    tracing::info!(d_total, c_total, "composites resynchronised");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ligne(
        num: &str,
        debut: &str,
        fin: Option<&str>,
        degenerate: bool,
        overlaps: bool,
    ) -> StarConflict {
        StarConflict {
            text_uid: "T".into(),
            new_key: "r441-22".into(),
            num: num.into(),
            num_key: num.to_lowercase(),
            date_debut: debut.into(),
            date_fin: fin.map(str::to_string),
            status: "MODIFIE".into(),
            degenerate,
            overlaps,
        }
    }

    /// L'intervalle dégénéré tombe le premier, et il tombe même quand c'est la
    /// forme nue qui le porte : la règle juge la chaîne, pas l'étoile.
    #[test]
    fn le_degenere_tombe_avant_le_chevauchant() {
        let groupe = [
            ligne("*R441-22", "2002-01-06", Some("2009-01-01"), false, true),
            ligne("R441-22", "2002-01-06", Some("2002-01-06"), true, true),
        ];
        let losers = versions_a_supprimer(&groupe).unwrap();
        assert_eq!(
            losers,
            [("T".into(), "r441-22".into(), "2002-01-06".into())]
        );
    }

    /// Sans ligne dégénérée, c'est le chevauchement d'une autre version qui
    /// départage : la survivante est celle qui chaîne.
    #[test]
    fn le_chevauchant_tombe_a_defaut() {
        let groupe = [
            ligne("ANNEXE*", "1995-06-19", Some("1999-02-20"), false, true),
            ligne("ANNEXE", "1995-06-19", Some("1996-07-04"), false, false),
        ];
        let losers = versions_a_supprimer(&groupe).unwrap();
        assert_eq!(losers.len(), 1);
        assert_eq!(losers[0].1, "annexe*");
    }

    #[test]
    fn deux_versions_qu_aucune_regle_ne_separe_sont_une_erreur() {
        let groupe = [
            ligne("R441-22", "2002-01-06", Some("2009-01-01"), false, false),
            ligne("R*441-22", "2002-01-06", Some("2009-01-01"), false, false),
        ];
        assert!(versions_a_supprimer(&groupe).is_err());
    }
}
