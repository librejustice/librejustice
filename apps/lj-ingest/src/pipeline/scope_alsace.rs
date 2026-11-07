//! Backfill Alsace-Moselle : les clauses des trois départements (Bas-Rhin,
//! Haut-Rhin, Moselle) posent le domaine implicite du droit local.
//!
//! Seul le NÉGATIF s'écrit. Sous l'identité législative, une clause positive
//! (« les articles X sont applicables dans les départements… ») est indécidable
//! par gabarit — le corpus mélange trois natures : restriction de régime local
//! (CSS livre III), modalité d'extension du droit national aux communes locales
//! (CGCT L2541-22, loi de 1968 sur les incapables majeurs « lorsqu'elles ne se
//! rapportent pas à des matières soumises à des dispositions particulières »),
//! et prose incidente (clause de nationalité de 1945). Ces candidats positifs —
//! y compris les textes entièrement locaux candidats à
//! `legal_text.default_scope` — sortent en RAPPORT de curation, jamais en
//! écriture automatique : seules des valeurs vérifiées se posent.
//!
//! S'écrit donc : « n'est pas applicable dans les départements… » — texte
//! entier → `ecarte` sur chaque article du texte ; article auto-référent ou refs
//! cibles niées → `ecarte` résolu comme les compteurs. Le grain DIVISION attend
//! le dépliage par TOC (compté).

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};

use lj_norm::compteurs::{parse_compteur, self_scope, SelfGrain};
use lj_store::repository::{ArticleScopeRow, DecisionRepository};

use crate::config::Settings;

use super::scope_compteurs::{event_anchored, resolve_event_targets, VersionsByKey};

const SCOPE: &str = "geo:FR-ALS";

#[derive(Default)]
struct Stats {
    candidates: usize,
    text_ecarte: usize,
    division_skipped: usize,
    no_event: usize,
    foreign_owner: usize,
    miss_anchor: usize,
    miss_current: usize,
}

pub async fn scope_backfill_alsace(apply: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let candidates = repo.compteur_candidates("Bas-Rhin|Haut-Rhin").await?;
    let mut stats = Stats {
        candidates: candidates.len(),
        ..Default::default()
    };
    let mut rows: Vec<ArticleScopeRow> = Vec::new();
    // Rapport de curation : (text_uid#num, nature) des positifs non écrits.
    let mut curation: Vec<(String, &'static str)> = Vec::new();
    let mut versions_cache: BTreeMap<String, VersionsByKey> = BTreeMap::new();

    for cand in &candidates {
        let source = format!("alsace:{SCOPE} {}#{}", cand.text_uid, cand.num_key);
        let ident = format!("{}#{}", cand.text_uid, cand.num_key);

        if let Some((grain, scopes, negated)) = self_scope(&cand.texte) {
            if !scopes.iter().any(|s| s == SCOPE) {
                continue;
            }
            match (grain, negated) {
                (SelfGrain::Text, false) => {
                    curation.push((ident, "texte entier positif (default_scope ?)"));
                    continue;
                }
                (SelfGrain::Text, true) => {
                    // Texte écarté des trois départements : ecarte par
                    // article, version en vigueur à la date de la clause.
                    if !versions_cache.contains_key(&cand.text_uid) {
                        let mut by_key = VersionsByKey::new();
                        for (k, d, f) in repo.article_versions_of(&cand.text_uid).await? {
                            by_key.entry(k).or_default().push((d, f));
                        }
                        versions_cache.insert(cand.text_uid.clone(), by_key);
                    }
                    for (key, vlist) in &versions_cache[&cand.text_uid] {
                        let Some((v_debut, _)) = vlist
                            .iter()
                            .filter(|(d, f)| {
                                *d <= cand.date_debut && f.is_none_or(|f| f > cand.date_debut)
                            })
                            .max()
                        else {
                            continue;
                        };
                        stats.text_ecarte += 1;
                        rows.push(ArticleScopeRow {
                            text_uid: cand.text_uid.clone(),
                            num_key: key.clone(),
                            date_debut: *v_debut,
                            mode: "ecarte".to_string(),
                            scope: SCOPE.to_string(),
                            s_debut: cand.date_debut,
                            s_fin: cand.date_fin,
                            source: source.clone(),
                        });
                    }
                    continue;
                }
                (SelfGrain::Article, false) => {
                    curation.push((ident, "article auto-référent positif"));
                    continue;
                }
                (SelfGrain::Article, true) => {
                    rows.push(ArticleScopeRow {
                        text_uid: cand.text_uid.clone(),
                        num_key: cand.num_key.clone(),
                        date_debut: cand.date_debut,
                        mode: "ecarte".to_string(),
                        scope: SCOPE.to_string(),
                        s_debut: cand.date_debut,
                        s_fin: cand.date_fin,
                        source: source.clone(),
                    });
                    continue;
                }
                (SelfGrain::Division, _) => {
                    stats.division_skipped += 1;
                    continue;
                }
            }
        }

        // Refs cibles : seuls les événements NIÉS s'écrivent (ecarte) ; les
        // positifs partent en curation.
        let events: Vec<_> = parse_compteur(&cand.texte)
            .into_iter()
            .filter(|e| e.scopes.iter().any(|s| s == SCOPE))
            .collect();
        if events.is_empty() {
            stats.no_event += 1;
            continue;
        }
        if events.iter().any(|e| !e.negated) {
            curation.push((ident.clone(), "refs cibles positives"));
        }
        let negated_events: Vec<_> = events.into_iter().filter(|e| e.negated).collect();
        if negated_events.is_empty() {
            continue;
        }
        if !versions_cache.contains_key(&cand.text_uid) {
            let mut by_key = VersionsByKey::new();
            for (k, d, f) in repo.article_versions_of(&cand.text_uid).await? {
                by_key.entry(k).or_default().push((d, f));
            }
            versions_cache.insert(cand.text_uid.clone(), by_key);
        }
        let versions = &versions_cache[&cand.text_uid];

        let mut local: Vec<(ArticleScopeRow, bool)> = Vec::new();
        for event in &negated_events {
            if event.foreign_owner {
                stats.foreign_owner += 1;
                continue;
            }
            let resolved = resolve_event_targets(event, versions, cand.date_debut);
            stats.miss_anchor += resolved.miss_anchor;
            stats.miss_current += resolved.miss_current;
            let mut src = source.clone();
            let prose: Vec<String> = event
                .exceptions
                .iter()
                .map(|e| e.prose.clone())
                .chain(event.reserves.iter().cloned())
                .collect();
            if !prose.is_empty() {
                let detail: String = prose.join(" ; ").chars().take(400).collect();
                src.push_str(" — ");
                src.push_str(&detail);
            }
            for (key, v_debut) in resolved.keys {
                local.push((
                    ArticleScopeRow {
                        text_uid: cand.text_uid.clone(),
                        num_key: key,
                        date_debut: v_debut,
                        mode: "ecarte".to_string(),
                        scope: SCOPE.to_string(),
                        s_debut: cand.date_debut,
                        s_fin: cand.date_fin,
                        source: src.clone(),
                    },
                    event_anchored(event),
                ));
            }
        }
        let pinned: std::collections::BTreeSet<String> = local
            .iter()
            .filter(|(_, a)| *a)
            .map(|(r, _)| r.num_key.clone())
            .collect();
        for (row, anchored) in local {
            if !anchored && pinned.contains(&row.num_key) {
                continue;
            }
            rows.push(row);
        }
    }

    rows.sort_by(|a, b| {
        (&a.text_uid, &a.num_key, a.date_debut, &a.mode, a.s_debut).cmp(&(
            &b.text_uid,
            &b.num_key,
            b.date_debut,
            &b.mode,
            b.s_debut,
        ))
    });
    rows.dedup_by(|a, b| {
        a.text_uid == b.text_uid
            && a.num_key == b.num_key
            && a.date_debut == b.date_debut
            && a.mode == b.mode
            && a.s_debut == b.s_debut
    });

    println!(
        "alsace : {} candidats — {} lignes ecarte (dont {} par texte écarté), \
         {} positifs en curation (non écrits), {} divisions skippées, {} sans événement, \
         {} cibles d'un autre texte, {} sans version",
        stats.candidates,
        rows.len(),
        stats.text_ecarte,
        curation.len(),
        stats.division_skipped,
        stats.no_event,
        stats.foreign_owner,
        stats.miss_anchor + stats.miss_current,
    );
    for (ident, nature) in curation.iter().take(12) {
        println!("  curation: {ident} — {nature}");
    }
    for row in rows.iter().take(6) {
        println!(
            "  {} {} v{} {} {}",
            row.text_uid,
            row.num_key,
            row.date_debut,
            row.mode,
            row.source.chars().take(80).collect::<String>()
        );
    }

    if apply {
        let (deleted, inserted) = repo
            .replace_article_scopes(&format!("alsace:{SCOPE} "), &rows)
            .await?;
        println!("article_scope: {deleted} deleted, {inserted} inserted");
    } else {
        println!("dry-run, re-run with --apply to write");
    }
    Ok(())
}
