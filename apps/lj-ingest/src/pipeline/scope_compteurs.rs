//! Backfill `article_scope` depuis les compteurs d'applicabilité outre-mer.
//! Une passe par collectivité : les articles VIGUEUR qui étendent (`etend`) ou
//! écartent (`ecarte`) des articles du même code vers NC/PF/WF/TAAF.
//!
//! Sémantique des versions cibles :
//! - ancre « dans sa rédaction résultant de la loi n° X du D » → version de
//!   la cible de plus petit `date_debut >= D` ;
//! - sans ancre → version en vigueur à la date du compteur (gel de la
//!   spécialité : la rédaction alors courante, pas la suivante).
//!
//! Rejouable : chaque run supprime les lignes de son tag
//! (`compteur:geo:XX `) et réinsère. Dry-run par défaut, `--apply` écrit.

use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Result};
use chrono::NaiveDate;

use lj_norm::article_order::num_key_sort_key;
use lj_norm::compteurs::{compteur_has_table, parse_compteur, parse_compteur_table, TargetRef};
use lj_store::repository::{ArticleScopeRow, DecisionRepository};

use crate::config::Settings;

/// Collectivités de spécialité législative gérées : `(code CLI, nom dans le
/// texte, uid de portée)`.
const COLLECTIVITES: &[(&str, &str, &str)] = &[
    ("PF", "Polynésie française", "geo:PF"),
    ("NC", "Nouvelle-Calédonie", "geo:NC"),
    ("WF", "Wallis-et-Futuna", "geo:WF"),
    (
        "TF",
        "Terres australes et antarctiques françaises",
        "geo:TF",
    ),
];

/// Versions d'un texte, par clé d'article : `[(date_debut, date_fin)]`.
pub(super) type VersionsByKey = BTreeMap<String, Vec<(NaiveDate, Option<NaiveDate>)>>;

/// Cibles résolues d'un événement : `(num_key, date_debut de la version)`,
/// plus les compteurs de trous. Partagé entre le backfill compteurs
/// (outre-mer) et le backfill Alsace-Moselle.
pub(super) struct ResolvedTargets {
    pub keys: Vec<(String, NaiveDate)>,
    pub miss_anchor: usize,
    pub miss_current: usize,
    /// Clés en trou, pour l'échantillon du rapport (`clé@raison`).
    pub misses: Vec<String>,
}

/// Résout les cibles d'un événement contre les versions du texte porteur :
/// dépliage des plages (même rang de partie), soustraction des exclusions
/// d'article entier, choix de version — ancre de rédaction (`min date_debut
/// >= D`) ou gel à la date `at` du porteur.
pub(super) fn resolve_event_targets(
    event: &lj_norm::compteurs::CounterEvent,
    versions: &VersionsByKey,
    at: NaiveDate,
) -> ResolvedTargets {
    let excluded: Vec<&str> = event
        .exceptions
        .iter()
        .flat_map(|e| e.full_articles.iter().map(String::as_str))
        .collect();
    let anchor: Option<NaiveDate> = event
        .redaction_date
        .as_deref()
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());

    let mut keys: Vec<String> = Vec::new();
    for target in &event.targets {
        match target {
            TargetRef::Single(k) => {
                if !excluded.contains(&k.as_str()) {
                    keys.push(k.clone());
                }
            }
            TargetRef::Range(a, b) => {
                let (ka, kb) = (num_key_sort_key(a), num_key_sort_key(b));
                for k in versions.keys() {
                    let kk = num_key_sort_key(k);
                    if kk.0 == ka.0 && kk >= ka && kk <= kb && !excluded.contains(&k.as_str()) {
                        keys.push(k.clone());
                    }
                }
            }
        }
    }

    let mut out = ResolvedTargets {
        keys: Vec::new(),
        miss_anchor: 0,
        miss_current: 0,
        misses: Vec::new(),
    };
    for key in keys {
        let Some(vlist) = versions.get(&key) else {
            out.miss_current += 1;
            out.misses.push(format!("{key}@inconnue"));
            continue;
        };
        let picked = match anchor {
            // Aucune version ne démarre à/après l'ancre : l'acte ancré n'a
            // pas créé de version de CETTE cible (décret fondateur du code,
            // ou acte qui ne la touche pas) — sa rédaction résultante est
            // celle en vigueur à la date de l'acte.
            Some(d) => vlist
                .iter()
                .filter(|(debut, _)| *debut >= d)
                .min()
                .copied()
                .or_else(|| {
                    vlist
                        .iter()
                        .filter(|(debut, fin)| *debut <= d && fin.is_none_or(|f| f > d))
                        .max()
                        .copied()
                }),
            None => vlist
                .iter()
                .filter(|(debut, fin)| *debut <= at && fin.is_none_or(|f| f > at))
                .max()
                .copied(),
        };
        match picked {
            Some((v_debut, _)) => out.keys.push((key, v_debut)),
            None if anchor.is_some() => {
                out.miss_anchor += 1;
                out.misses.push(format!("{key}@ancre"));
            }
            None => {
                out.miss_current += 1;
                out.misses.push(format!("{key}@abrogée"));
            }
        }
    }
    out
}

/// La cible venait-elle d'une ancre de rédaction explicite ?
pub(super) fn event_anchored(event: &lj_norm::compteurs::CounterEvent) -> bool {
    event.redaction_date.is_some()
}

#[derive(Default)]
struct Stats {
    candidates: usize,
    with_table: usize,
    table_unopened: usize,
    table_divisions: usize,
    table_unrecognized: usize,
    no_event: usize,
    empty_scope_skipped: usize,
    foreign_owner: usize,
    miss_anchor: usize,
    miss_current: usize,
}

pub async fn scope_backfill_compteurs(
    collectivite: String,
    only_text_uid: Option<String>,
    apply: bool,
) -> Result<()> {
    let Some((_, nom, scope_uid)) = COLLECTIVITES.iter().find(|(c, _, _)| *c == collectivite)
    else {
        bail!(
            "collectivité inconnue: {collectivite} (attendu: {})",
            COLLECTIVITES
                .iter()
                .map(|(c, _, _)| *c)
                .collect::<Vec<_>>()
                .join(", ")
        );
    };
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let mut candidates = repo.compteur_candidates(nom).await?;
    if let Some(uid) = &only_text_uid {
        candidates.retain(|c| &c.text_uid == uid);
    }

    let mut stats = Stats {
        candidates: candidates.len(),
        ..Default::default()
    };
    let mut rows: Vec<ArticleScopeRow> = Vec::new();
    let mut versions_cache: BTreeMap<String, VersionsByKey> = BTreeMap::new();
    let mut miss_samples: Vec<String> = Vec::new();

    for compteur in &candidates {
        let mut events = parse_compteur(&compteur.texte);
        if compteur_has_table(&compteur.texte) {
            stats.with_table += 1;
            match parse_compteur_table(&compteur.texte) {
                Some(table) => {
                    stats.table_divisions += table.divisions;
                    if !table.unrecognized.is_empty() {
                        stats.table_unrecognized += table.unrecognized.len();
                        tracing::debug!(
                            text_uid = %compteur.text_uid,
                            num = %compteur.num,
                            lignes = ?table.unrecognized,
                            "table rows not recognised"
                        );
                    }
                    events.extend(table.events);
                }
                None => stats.table_unopened += 1,
            }
        }
        if events.is_empty() {
            stats.no_event += 1;
            continue;
        }
        // Héritage du scope implicite : uniquement si la collectivité du run
        // est la SEULE nommée par le compteur (un compteur multi-collectivités
        // rend l'héritage ambigu — on saute et on compte).
        let all_scopes: std::collections::BTreeSet<String> = events
            .iter()
            .flat_map(|e| e.scopes.iter().cloned())
            .collect();
        let inherit_ok = all_scopes.len() == 1 && all_scopes.contains(*scope_uid);

        if !versions_cache.contains_key(&compteur.text_uid) {
            let mut by_key = VersionsByKey::new();
            for (num_key, debut, fin) in repo.article_versions_of(&compteur.text_uid).await? {
                by_key.entry(num_key).or_default().push((debut, fin));
            }
            versions_cache.insert(compteur.text_uid.clone(), by_key);
        }
        let versions = &versions_cache[&compteur.text_uid];

        // Lignes du compteur courant, avec le drapeau « épinglée » : une
        // cible à ancre de rédaction explicite PRIME la même cible reprise
        // par une plage parapluie du même compteur (lex specialis).
        let mut local: Vec<(ArticleScopeRow, bool)> = Vec::new();

        for event in &events {
            if event.foreign_owner {
                stats.foreign_owner += 1;
                continue;
            }
            let explicit = event.scopes.iter().any(|s| s == scope_uid);
            if !(explicit || event.scopes.is_empty() && inherit_ok) {
                if event.scopes.is_empty() {
                    stats.empty_scope_skipped += 1;
                }
                continue;
            }

            let resolved = resolve_event_targets(event, versions, compteur.date_debut);
            stats.miss_anchor += resolved.miss_anchor;
            stats.miss_current += resolved.miss_current;
            for m in &resolved.misses {
                if miss_samples.len() < 15 {
                    miss_samples.push(format!("{}#{} → {m}", compteur.text_uid, compteur.num_key));
                }
            }

            let mut prose: Vec<String> = Vec::new();
            prose.extend(event.exceptions.iter().map(|e| e.prose.clone()));
            prose.extend(event.reserves.iter().cloned());
            let mut source = format!(
                "compteur:{scope_uid} {}#{}",
                compteur.text_uid, compteur.num_key
            );
            if !prose.is_empty() {
                let detail: String = prose.join(" ; ").chars().take(400).collect();
                source.push_str(" — ");
                source.push_str(&detail);
            }

            for (key, v_debut) in resolved.keys {
                let mode = if event.negated { "ecarte" } else { "etend" };
                local.push((
                    ArticleScopeRow {
                        text_uid: compteur.text_uid.clone(),
                        num_key: key,
                        date_debut: v_debut,
                        mode: mode.to_string(),
                        scope: (*scope_uid).to_string(),
                        s_debut: compteur.date_debut,
                        s_fin: compteur.date_fin,
                        source: source.clone(),
                    },
                    event_anchored(event),
                ));
            }
        }

        let pinned: std::collections::BTreeSet<&str> = local
            .iter()
            .filter(|(_, anchored)| *anchored)
            .map(|(r, _)| r.num_key.as_str())
            .collect();
        let pinned: std::collections::BTreeSet<String> =
            pinned.into_iter().map(str::to_string).collect();
        for (row, anchored) in local {
            if !anchored && pinned.contains(&row.num_key) {
                continue;
            }
            rows.push(row);
        }
    }

    // Dédoublonne (un même compteur peut énoncer deux fois la même cible).
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

    let n_ecarte = rows.iter().filter(|r| r.mode == "ecarte").count();
    println!(
        "compteurs {collectivite} : {} candidats, {} à tableau (dont {} sans en-tête, \
         {} divisions, {} lignes non reconnues), {} sans événement, \
         {} hors scope/ambigus, {} cibles d'un autre texte (skip), \
         {} lignes ({} etend, {} ecarte), \
         {} cibles sans version à l'ancre, {} sans version courante",
        stats.candidates,
        stats.with_table,
        stats.table_unopened,
        stats.table_divisions,
        stats.table_unrecognized,
        stats.no_event,
        stats.empty_scope_skipped,
        stats.foreign_owner,
        rows.len(),
        rows.len() - n_ecarte,
        n_ecarte,
        stats.miss_anchor,
        stats.miss_current,
    );
    for row in rows.iter().take(8) {
        println!(
            "  {} {} v{} {} [{} → {:?}] {}",
            row.text_uid,
            row.num_key,
            row.date_debut,
            row.mode,
            row.s_debut,
            row.s_fin,
            row.source.chars().take(90).collect::<String>()
        );
    }
    for m in &miss_samples {
        println!("  trou: {m}");
    }

    if apply {
        let tag = format!("compteur:{scope_uid} ");
        let (deleted, inserted) = repo.replace_article_scopes(&tag, &rows).await?;
        println!("article_scope: {deleted} deleted, {inserted} inserted");
    } else {
        println!("dry-run, re-run with --apply to write");
    }
    Ok(())
}
