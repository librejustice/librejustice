//! Rejeu chronologique des snapshots CNB publiés depuis octobre 2022.
//!
//! Le schéma des snapshots mensuels est plus maigre que l'actuel, mais porte
//! déjà barreau, nom, prénom, `cbSiretSiren` et `acDateSerment` : des années de
//! présence observée que le reload courant ne peut pas reconstituer, ne voyant
//! qu'un instant.
//!
//! Deux évidences en sortent, sous la provenance `cnb-hist` : des spans
//! d'activité et des arêtes d'affiliation datées. Le rejeu est une FONCTION des
//! snapshots — chaque run recalcule ses évidences en entier et remplace les
//! précédentes, donc relançable sans borne. Dry-run par défaut.
//!
//! Le rejeu ne résout pas son propre delta : il crée des milliers d'identités
//! d'un coup, là où la sonde at-write du reload est taillée pour les arrivées
//! d'un snapshot. `reconcile` dé-lie ce que ces identités rendent ambigu,
//! `parties-relink --full` lie ce qu'elles attirent.

use std::collections::HashMap;

use anyhow::{anyhow, Result};

use lj_sources::error::SourceError;
use lj_sources::registries::{
    datagouv_resources, download_resource, for_each_csv_record, RegistryResource,
};
use lj_store::repository::{
    AffiliationEdge, CnbEntityState, CnbEntityUpdate, DecisionRepository, EntityWriteItem,
};

use super::registries::{
    fetch_avocat_rows, field, parse_yyyymmdd, slug, snapshot_date_from_title, surname_key, CnbRows,
};
use crate::config::Settings;

/// Identité observée à un snapshot : ce que le schéma maigre en dit.
pub(super) struct HistRow {
    pub(super) barreau: String,
    pub(super) barreau_slug: String,
    pub(super) nom: String,
    pub(super) prenom: String,
    /// `acDateSerment` — entrée dans la profession, déclarée et
    /// rétroactivement valide (présente dès le snapshot de 2022).
    pub(super) serment: Option<chrono::NaiveDate>,
    /// `cbSiretSiren` — structure d'exercice au moment du snapshot.
    pub(super) siren: Option<String>,
}

/// Une période continue de présence, en indices de snapshots.
#[derive(Debug, PartialEq, Eq)]
struct Run {
    first: usize,
    last: usize,
}

/// Plie une suite d'indices croissants en périodes continues : un trou est
/// une absence observée, donc une fin de période et un nouveau départ.
fn runs(mut idx: Vec<usize>) -> Vec<Run> {
    idx.sort_unstable();
    idx.dedup();
    let mut out: Vec<Run> = Vec::new();
    for i in idx {
        match out.last_mut() {
            Some(r) if r.last + 1 == i => r.last = i,
            _ => out.push(Run { first: i, last: i }),
        }
    }
    out
}

/// Index par nom des colonnes utiles — le nom de la colonne barreau change
/// en cours d'historique (`NomBarreau` jusqu'en octobre 2025, `Barreau`
/// ensuite), d'où la résolution par alias.
fn col(headers: &[String], names: &[&str]) -> Result<usize, SourceError> {
    names
        .iter()
        .find_map(|n| headers.iter().position(|h| h == n))
        .ok_or_else(|| SourceError::Invalid(format!("aucune colonne {names:?} dans {headers:?}")))
}

pub(super) fn parse_snapshot(path: &std::path::Path) -> Result<Vec<HistRow>> {
    let mut rows = Vec::new();
    for_each_csv_record(path, b';', |headers, rec| {
        let barreau = col(headers, &["Barreau", "NomBarreau"])?;
        let nom = col(headers, &["avNom"])?;
        let prenom = col(headers, &["avPrenom"])?;
        let siren = col(headers, &["cbSiretSiren"])?;
        let serment = col(headers, &["acDateSerment"])?;
        let nom = field(rec, nom);
        let barreau = field(rec, barreau);
        if nom.is_empty() || barreau.is_empty() {
            return Ok(());
        }
        rows.push(HistRow {
            barreau_slug: slug(&barreau),
            barreau,
            prenom: field(rec, prenom),
            serment: parse_yyyymmdd(&field(rec, serment)),
            siren: Some(field(rec, siren))
                .filter(|s| s.len() == 9 && s.bytes().all(|b| b.is_ascii_digit())),
            nom,
        });
        Ok(())
    })?;
    Ok(rows)
}

/// Identité accumulée sur toute la fenêtre historique.
#[derive(Default)]
struct HistIdentity {
    /// Indices des snapshots où l'identité est présente.
    seen: Vec<usize>,
    /// Indices de présence par siren de structure.
    by_siren: HashMap<String, Vec<usize>>,
    /// Date de serment la plus ancienne rencontrée (déclarative).
    serment: Option<chrono::NaiveDate>,
    /// Dernier libellé rencontré — dénomination des identités à créer.
    nom: String,
    prenom: String,
    barreau: String,
    barreau_slug: String,
}

/// Un span `cnb` OUVERT et daté couvre tout ce que l'observation dirait de
/// plus tard : inutile de réécrire l'évidence. Le placeholder sans `du`
/// (réhydratation) ne couvre rien, il attend justement d'être remplacé par du
/// connu.
fn covered_by_open_span(activity: Option<&serde_json::Value>, du: chrono::NaiveDate) -> bool {
    let Some(spans) = activity.and_then(|a| a.as_array()) else {
        return false;
    };
    spans.iter().any(|s| {
        s.get("au").is_none()
            && s.get("du")
                .and_then(|d| d.as_str())
                .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                .is_some_and(|d| d <= du)
    })
}

/// Spans à conserver au moment de réécrire les évidences historiques : tout
/// ce qui ne vient pas du rejeu, moins le placeholder de réhydratation quand
/// l'identité est sortie du registre ET que le rejeu, lui, l'a vraiment
/// observée — le placeholder disait « actif jusqu'à une date inconnue »
/// faute de mieux, l'observation le remplace par des bornes.
fn kept_spans(
    activity: Option<&serde_json::Value>,
    active: bool,
    observed: bool,
) -> Vec<serde_json::Value> {
    let Some(spans) = activity.and_then(|a| a.as_array()) else {
        return Vec::new();
    };
    spans
        .iter()
        .filter(|s| s.get("src").and_then(|v| v.as_str()) != Some(SRC))
        .filter(|s| {
            let placeholder = s.get("du").is_none();
            active || !observed || !placeholder
        })
        .cloned()
        .collect()
}

/// Provenance des évidences produites par le rejeu.
const SRC: &str = "cnb-hist";

/// Écritures calculées par le rejeu (pur, testé).
#[derive(Default)]
struct HistPlan {
    /// Identités absentes de la base — sorties avant l'apparition du code
    /// CNBF, recréées inactives sous le schéma d'uid legacy.
    inserts: Vec<EntityWriteItem>,
    /// Identités existantes dont l'activité gagne des périodes observées.
    updates: Vec<CnbEntityUpdate>,
    edges: Vec<AffiliationEdge>,
    /// Périodes écartées car déjà couvertes par un span ouvert daté.
    dominated: u64,
}

fn hist_plan(
    identities: &HashMap<String, HistIdentity>,
    existing: &[CnbEntityState],
    dates: &[chrono::NaiveDate],
) -> HistPlan {
    let by_uid: HashMap<&str, &CnbEntityState> =
        existing.iter().map(|s| (s.uid.as_str(), s)).collect();
    let mut plan = HistPlan::default();

    for (uid, id) in identities {
        let state = by_uid.get(uid.as_str()).copied();
        let active = state.is_some_and(|s| s.active);

        // Spans de présence. `acDateSerment` recule le début de la première
        // période : la source déclare l'entrée dans la profession, valide
        // rétroactivement, là où l'observation ne commence qu'en 2022.
        let mut spans: Vec<serde_json::Value> = Vec::new();
        for (n, run) in runs(id.seen.clone()).iter().enumerate() {
            let mut du = dates[run.first];
            if n == 0 {
                if let Some(s) = id.serment.filter(|s| *s < du) {
                    du = s;
                }
            }
            if covered_by_open_span(state.and_then(|s| s.activity.as_ref()), du) {
                plan.dominated += 1;
                continue;
            }
            spans.push(serde_json::json!({
                "du": du.to_string(), "au": dates[run.last].to_string(), "src": SRC
            }));
        }

        // Arêtes d'affiliation : une par période continue passée dans la
        // même structure. Les changements de siren se lisent comme des
        // périodes distinctes, y compris un aller-retour.
        for (siren, seen) in &id.by_siren {
            for run in runs(seen.clone()) {
                plan.edges.push(AffiliationEdge {
                    entity_uid: uid.clone(),
                    structure_uid: format!("siren:{siren}"),
                    du: Some(dates[run.first]),
                    au: Some(dates[run.last]),
                });
            }
        }

        let observed = !spans.is_empty();
        match state {
            Some(s) => {
                let mut all = kept_spans(s.activity.as_ref(), active, observed);
                let before = s
                    .activity
                    .as_ref()
                    .and_then(|a| a.as_array())
                    .map_or(0, Vec::len);
                if spans.is_empty() && all.len() == before {
                    continue; // ni période à ajouter ni placeholder à retirer
                }
                all.extend(spans);
                plan.updates.push(CnbEntityUpdate {
                    uid: uid.clone(),
                    active: s.active,
                    activity: Some(serde_json::Value::Array(all)),
                    barreau: s.barreau.clone(),
                    denomination: s.denomination.clone(),
                    forme: s.forme.clone(),
                    surname_key: s.surname_key.clone(),
                });
            }
            None => plan.inserts.push(EntityWriteItem {
                uid: uid.clone(),
                nature: "physique",
                denomination: format!("{} {}", id.prenom, id.nom).trim().to_string(),
                sigle: None,
                forme: Some(format!("avocat ({})", id.barreau)),
                active: false,
                surname_key: surname_key(&id.nom),
                category: "avocats",
                ape: None,
                barreau: Some(id.barreau_slug.clone()),
                alt_denominations: Vec::new(),
                activity: Some(serde_json::Value::Array(spans)),
            }),
        }
    }
    plan
}

/// Ponts (barreau, nom-prénom) → code CNBF depuis le snapshot courant : une
/// identité encore inscrite garde son uid CNBF à travers tout l'historique.
/// Une clé ambiguë dans le snapshot courant ne ponte pas — elle retombe sur
/// l'uid legacy, qui distingue au moins par barreau.
struct Bridge {
    exact: HashMap<(String, String), Option<String>>,
    by_name: HashMap<String, Option<String>>,
}

impl Bridge {
    /// `multi_porteurs` : noms dont l'historique prouve qu'ils désignent
    /// plusieurs personnes (cf. `multi_bearer_names`). Leur pont par nom seul
    /// est retiré — il ne resterait qu'à deviner lequel des porteurs.
    fn build(
        rows: &[super::registries::AvocatRow],
        multi_porteurs: &std::collections::HashSet<String>,
    ) -> Self {
        let mut exact: HashMap<(String, String), Option<String>> = HashMap::new();
        let mut by_name: HashMap<String, Option<String>> = HashMap::new();
        for r in rows {
            let np = r.nomprenom_slug();
            exact
                .entry((r.barreau_slug.clone(), np.clone()))
                .and_modify(|v| *v = None)
                .or_insert(Some(r.uid.clone()));
            let porte = (!multi_porteurs.contains(&np)).then(|| r.uid.clone());
            by_name.entry(np).and_modify(|v| *v = None).or_insert(porte);
        }
        Self { exact, by_name }
    }

    /// Uid de l'identité pour une ligne historique — CNBF si le pont existe,
    /// sinon le schéma legacy `cnb:{barreau}:{nom}-{prenom}` (le même que
    /// celui des danglants réhydratés : les évidences se rejoignent).
    fn uid(&self, barreau_slug: &str, nomprenom: &str) -> String {
        if let Some(Some(uid)) = self
            .exact
            .get(&(barreau_slug.to_string(), nomprenom.to_string()))
        {
            return uid.clone();
        }
        if let Some(Some(uid)) = self.by_name.get(nomprenom) {
            return uid.clone();
        }
        format!("cnb:{barreau_slug}:{nomprenom}")
    }
}

/// Noms `nom-prenom` portés par PLUSIEURS personnes, prouvé par la source
/// elle-même : deux barreaux pour le même nom au même snapshot, alors qu'un
/// avocat n'est inscrit qu'à un barreau à la fois.
///
/// Une mutation de barreau produit parfois la même double inscription le temps
/// d'un snapshot, et rien dans la source ne la distingue d'un homonyme : on
/// s'abstient dans les deux cas, ce qui coûte une identité legacy de plus et
/// n'attribue jamais le passé d'un partant à un homonyme vivant.
fn collect_multi_bearers(rows: &[HistRow], multi: &mut std::collections::HashSet<String>) {
    let mut barreau_par_nom: HashMap<String, &str> = HashMap::new();
    for r in rows {
        let np = format!("{}-{}", slug(&r.nom), slug(&r.prenom));
        match barreau_par_nom.get(&np) {
            Some(b) if *b != r.barreau_slug => {
                multi.insert(np);
            }
            Some(_) => {}
            None => {
                barreau_par_nom.insert(np, &r.barreau_slug);
            }
        }
    }
}

/// Télécharge un snapshot (le cache le rend gratuit dès la 2ᵉ passe).
async fn fetch_snapshot(
    res: &RegistryResource,
    dir: &std::path::Path,
) -> Result<std::path::PathBuf> {
    let (res, dir) = (res.clone(), dir.to_path_buf());
    Ok(tokio::task::spawn_blocking(move || download_resource(&res, &dir)).await??)
}

/// Commande `cnb-history` : rejoue tous les snapshots publiés et remplace
/// les évidences `cnb-hist`. Dry-run par défaut.
pub async fn replay_cnb_history(apply: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.cache_dir().join("registries");
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let (CnbRows::Coded(current), _) = fetch_avocat_rows(&dir).await? else {
        anyhow::bail!("current cnb snapshot carries no avCnbfCode");
    };

    let resources: Vec<RegistryResource> = tokio::task::spawn_blocking(|| {
        datagouv_resources(
            "annuaire-des-avocats-de-france",
            |_| true,
            |f| f.eq_ignore_ascii_case("csv"),
        )
    })
    .await??;
    // Un snapshot est daté par son titre ; deux publications du même jour
    // sont le même instant observé, la seconde n'ajoute rien.
    let mut snapshots: Vec<(chrono::NaiveDate, RegistryResource)> = resources
        .into_iter()
        .filter_map(|r| snapshot_date_from_title(&r.title).map(|d| (d, r)))
        .collect();
    snapshots.sort_by_key(|(d, _)| *d);
    snapshots.dedup_by_key(|(d, _)| *d);
    let dates: Vec<chrono::NaiveDate> = snapshots.iter().map(|(d, _)| *d).collect();
    tracing::info!(
        snapshots = dates.len(),
        premier = %dates.first().map(|d| d.to_string()).unwrap_or_default(),
        dernier = %dates.last().map(|d| d.to_string()).unwrap_or_default(),
        "cnb replay: snapshots kept"
    );

    // Deux passes sur les mêmes fichiers (le premier téléchargement les met en
    // cache) : les noms multi-porteurs se lisent sur l'historique entier, et le
    // pont doit les connaître avant d'attribuer le moindre uid.
    let mut multi_porteurs = std::collections::HashSet::new();
    for (_, res) in &snapshots {
        let path = fetch_snapshot(res, &dir).await?;
        let rows = tokio::task::block_in_place(|| parse_snapshot(&path))?;
        collect_multi_bearers(&rows, &mut multi_porteurs);
    }
    tracing::info!(
        noms = multi_porteurs.len(),
        "names borne by several people, name-only bridge removed"
    );
    let bridge = Bridge::build(&current, &multi_porteurs);

    let mut identities: HashMap<String, HistIdentity> = HashMap::new();
    for (i, (date, res)) in snapshots.iter().enumerate() {
        let path = fetch_snapshot(res, &dir).await?;
        let rows = tokio::task::block_in_place(|| parse_snapshot(&path))?;
        for r in &rows {
            let nomprenom = format!("{}-{}", slug(&r.nom), slug(&r.prenom));
            let uid = bridge.uid(&r.barreau_slug, &nomprenom);
            let id = identities.entry(uid).or_default();
            id.seen.push(i);
            if let Some(s) = &r.siren {
                id.by_siren.entry(s.clone()).or_default().push(i);
            }
            id.serment = match (id.serment, r.serment) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            id.nom.clone_from(&r.nom);
            id.prenom.clone_from(&r.prenom);
            id.barreau.clone_from(&r.barreau);
            id.barreau_slug.clone_from(&r.barreau_slug);
        }
        tracing::info!(%date, rows = rows.len(), identites = identities.len(), "snapshot replayed");
    }

    let existing = repo.entity_cnb_states().await?;
    let plan = hist_plan(&identities, &existing, &dates);
    println!(
        "cnb-history : {} snapshots ({} → {}), {} identités observées\n  \
         {} entités à créer (sorties sans code CNBF)\n  \
         {} activités à enrichir ({} périodes déjà couvertes par un span ouvert)\n  \
         {} arêtes d'affiliation datées",
        dates.len(),
        dates.first().map(|d| d.to_string()).unwrap_or_default(),
        dates.last().map(|d| d.to_string()).unwrap_or_default(),
        identities.len(),
        plan.inserts.len(),
        plan.updates.len(),
        plan.dominated,
        plan.edges.len(),
    );
    if !apply {
        println!("(dry-run, re-run with --apply to write)");
        return Ok(());
    }

    conn.batch_execute("BEGIN; SET LOCAL statement_timeout = 0")
        .await?;
    for batch in plan.inserts.chunks(20_000) {
        repo.entity_copy(batch).await?;
    }
    let updated = repo.entity_cnb_update_bulk(&plan.updates).await?;
    let dropped = repo.affiliation_source_clear(SRC).await?;
    let mut edges = 0u64;
    for batch in plan.edges.chunks(20_000) {
        edges += repo.affiliation_insert_bulk(SRC, batch).await?;
    }

    conn.batch_execute("COMMIT").await?;

    // Stats fraîches avant les filets qui suivent : ils choisissent leur plan sur
    // la taille du superset d'ambiguïté, où le rejeu vient de verser des milliers
    // de candidats.
    conn.batch_execute("ANALYZE entity, entity_affiliation")
        .await?;

    tracing::info!(
        inserted = plan.inserts.len(),
        updated,
        edges,
        dropped,
        "cnb replay applied"
    );
    println!(
        "cnb-history appliqué : {} entités, {updated} activités, {edges} arêtes \
         ({dropped} remplacées).\nEnchaîner `reconcile` (dé-lie ce que ces identités \
         rendent ambigu à date) puis `parties-relink --full` (lie ce qu'elles attirent).",
        plan.inserts.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> chrono::NaiveDate {
        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn runs_coupe_sur_les_trous() {
        assert_eq!(runs(vec![]), vec![]);
        assert_eq!(runs(vec![0, 1, 2]), vec![Run { first: 0, last: 2 }]);
        assert_eq!(
            runs(vec![0, 1, 3, 5, 6]),
            vec![
                Run { first: 0, last: 1 },
                Run { first: 3, last: 3 },
                Run { first: 5, last: 6 },
            ]
        );
    }

    #[test]
    fn periode_couverte_par_un_span_ouvert_anterieur() {
        let act = serde_json::json!([{"du": "1998-01-05", "src": "cnb"}]);
        assert!(covered_by_open_span(Some(&act), d("2022-10-11")));
        let act = serde_json::json!([{"du": "2024-03-01", "src": "cnb"}]);
        assert!(!covered_by_open_span(Some(&act), d("2022-10-11")));
        let act = serde_json::json!([{"du": "1998-01-05", "au": "2024-01-01", "src": "cnb"}]);
        assert!(!covered_by_open_span(Some(&act), d("2022-10-11")));
        assert!(!covered_by_open_span(None, d("2022-10-11")));
    }

    #[test]
    fn placeholder_remplace_seulement_quand_le_rejeu_observe_un_sorti() {
        let act = serde_json::json!([{"au": "2026-07-17", "src": "cnb"}]);
        assert!(kept_spans(Some(&act), false, true).is_empty());
        assert_eq!(kept_spans(Some(&act), false, false).len(), 1);
        assert_eq!(kept_spans(Some(&act), true, true).len(), 1);
        let act = serde_json::json!([
            {"du": "1998-01-05", "src": "cnb"},
            {"du": "2022-10-11", "au": "2024-03-18", "src": SRC},
        ]);
        let kept = kept_spans(Some(&act), true, true);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["du"], "1998-01-05");
    }

    #[test]
    fn identite_sortie_recree_inactive_avec_ses_periodes() {
        let dates = vec![d("2022-10-11"), d("2022-11-25"), d("2022-12-26")];
        let mut identities = HashMap::new();
        identities.insert(
            "cnb:paris:dupont-jean".to_string(),
            HistIdentity {
                seen: vec![0, 1],
                serment: Some(d("2005-01-10")),
                nom: "DUPONT".into(),
                prenom: "Jean".into(),
                barreau: "PARIS".into(),
                barreau_slug: "paris".into(),
                ..Default::default()
            },
        );
        let plan = hist_plan(&identities, &[], &dates);
        assert_eq!(plan.inserts.len(), 1);
        let e = &plan.inserts[0];
        assert!(!e.active);
        assert_eq!(e.denomination, "Jean DUPONT");
        let spans = e.activity.as_ref().unwrap().as_array().unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0]["du"], "2005-01-10");
        assert_eq!(spans[0]["au"], "2022-11-25");
    }
}
