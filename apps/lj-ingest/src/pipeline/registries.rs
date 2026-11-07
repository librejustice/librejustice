//! Chargement des registres d'entités : SIRENE (unités légales Insee), RNA
//! (associations), annuaire des avocats (CNB, open data).
//!
//! Découverte + download via `lj_sources::registries` (API dataset data.gouv,
//! URLs de ressources datées) ; chargement par remplacement de namespace + COPY
//! binaire (`lj_store`, idempotent).
//!
//! Denominations fold with `lj_fold::fold`, the same fold as the NER extraction
//! and the SQL `lj_fold`: resolution compares folded to folded.

use super::cnb_history::{parse_snapshot, HistRow};
use super::cnb_reduced;
use crate::config::Settings;
use anyhow::{anyhow, Result};
use lj_sources::error::SourceError;
use lj_sources::registries::{
    datagouv_latest_resource, decode_field, download_resource, for_each_csv_record,
    RegistryResource,
};
use lj_store::repository::{
    AffiliationEdge, CnbEntityState, CnbEntityUpdate, DecisionRepository, EntityHistoryWriteItem,
    EntityWriteItem,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Sources de registres (clap `value_enum`).
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum RegistrySource {
    /// Unités légales Insee (namespace `siren:`) : personnes morales + EI
    /// diffusibles.
    Sirene,
    /// Répertoire national des associations (namespace `rna:`).
    Rna,
    /// Annuaire des avocats de France, CNB (namespace `cnb:`).
    Avocats,
    /// Annuaire des avocats aux Conseils (Ordre au CE + Cass., namespace
    /// `oacc:`) — snapshot curé sous `ingest/corpus/registries/oacc.json`, pas de
    /// bulk data.gouv.
    AvocatsConseils,
}

const BATCH: usize = 20_000;

/// Slug d'uid : pliage des uids, blancs et apostrophes en tirets.
pub(super) fn slug(s: &str) -> String {
    lj_fold::fold(s).replace([' ', '\''], "-")
}

/// Clé patronyme composé : nom-seul plié, tirets normalisés en espaces —
/// `None` si mono-token (le nom-seul simple reste irrésoluble).
pub(super) fn surname_key(nom: &str) -> Option<String> {
    let k = lj_fold::fold(nom)
        .replace('-', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    k.contains(' ').then_some(k)
}

/// Slug d'uid pour raison sociale : ne garde qu'alphanumérique + tirets
/// (les raisons sociales OACC portent virgules, `&`, points — hors uid).
fn firm_slug(s: &str) -> String {
    let mut out = String::new();
    for c in lj_fold::fold(s).chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

pub async fn load_registries(source: RegistrySource, with_history: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.cache_dir().join("registries");
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    // Toggle GIN encadrant : le régime établi tient `entity_folded_all_idx` à
    // `fastupdate = off` (les probes at-write de la résolution des parties ne
    // rescannent pas de pending list) ; le reload bulk repasse en `on` le temps du
    // COPY, puis restaure `off` ET vide la pending list — le retour à `off` ne la
    // vide pas. Restauration sur les deux chemins.
    conn.batch_execute("ALTER INDEX entity_folded_all_idx SET (fastupdate = on)")
        .await?;
    let result = match source {
        RegistrySource::Sirene => load_sirene(&repo, &conn, &dir, with_history)
            .await
            .map(|n| ("siren", n)),
        RegistrySource::Rna => load_rna(&repo, &conn, &dir).await.map(|n| ("rna", n)),
        RegistrySource::Avocats => load_avocats(&repo, &conn, &dir).await.map(|n| ("cnb", n)),
        RegistrySource::AvocatsConseils => {
            // Snapshot curé : `ingest/corpus/registries/oacc.json`, pas de download
            // data.gouv (annuaire sans bulk).
            let path = settings
                .legal_corpus_dir()
                .join("registries")
                .join("oacc.json");
            load_avocats_conseils(&repo, &conn, &path)
                .await
                .map(|n| ("oacc", n))
        }
    };
    // Un échec laisse la connexion dans sa transaction avortée : la purger, sinon
    // la restauration ci-dessous échoue et masque l'erreur du loader.
    if result.is_err() {
        conn.batch_execute("ROLLBACK").await.ok();
    }
    conn.batch_execute(
        "ALTER INDEX entity_folded_all_idx SET (fastupdate = off); \
         SELECT gin_clean_pending_list('entity_folded_all_idx')",
    )
    .await?;
    let (namespace, loaded) = result?;
    let count = repo.entity_count(namespace).await?;
    tracing::info!(namespace, loaded, count, "registry loaded");
    println!("registries {namespace}: {loaded} entities loaded ({count} in the database).");
    Ok(())
}

/// Résout puis télécharge une ressource (les deux appels sont bloquants →
/// `spawn_blocking`).
async fn fetch(
    dataset: &'static str,
    dir: &Path,
    title_ok: impl Fn(&str) -> bool + Send + 'static,
    format_ok: impl Fn(&str) -> bool + Send + 'static,
) -> Result<(RegistryResource, PathBuf)> {
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<(RegistryResource, PathBuf)> {
        let res = datagouv_latest_resource(dataset, title_ok, format_ok)?;
        let path = download_resource(&res, &dir)?;
        Ok((res, path))
    })
    .await?
}

/// Résolution paresseuse des index de colonnes par nom — recalculée quand le
/// slice d'en-têtes change (nouveau membre de zip).
struct Cols {
    key: usize,
    idx: Vec<usize>,
}

impl Cols {
    fn new() -> Self {
        Self {
            key: 0,
            idx: Vec::new(),
        }
    }

    fn resolve(&mut self, headers: &[String], names: &[&str]) -> Result<&[usize], SourceError> {
        let key = headers.as_ptr() as usize;
        if self.key != key {
            self.idx = names
                .iter()
                .map(|n| {
                    headers.iter().position(|h| h == n).ok_or_else(|| {
                        SourceError::Invalid(format!("colonne {n} absente de {headers:?}"))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            self.key = key;
        }
        Ok(&self.idx)
    }
}

pub(super) fn field(rec: &csv::ByteRecord, i: usize) -> String {
    decode_field(rec.get(i).unwrap_or(b"")).trim().to_string()
}

fn db_err(e: impl std::fmt::Display) -> SourceError {
    SourceError::Invalid(format!("db: {e}"))
}

/// Boucle générique : parse bloquant (`block_in_place`) qui pousse des lots
/// vers la connexion async (`Handle::block_on`) — RAM bornée aux lots.
struct Loader<'a, 'b> {
    repo: &'a DecisionRepository<'b>,
    handle: tokio::runtime::Handle,
    ents: Vec<EntityWriteItem>,
    hist: Vec<EntityHistoryWriteItem>,
    loaded: u64,
}

impl<'a, 'b> Loader<'a, 'b> {
    fn new(repo: &'a DecisionRepository<'b>) -> Self {
        Self {
            repo,
            handle: tokio::runtime::Handle::current(),
            ents: Vec::with_capacity(BATCH),
            hist: Vec::with_capacity(BATCH),
            loaded: 0,
        }
    }

    fn push(&mut self, ent: EntityWriteItem) -> Result<(), SourceError> {
        self.ents.push(ent);
        self.loaded += 1;
        if self.ents.len() >= BATCH {
            self.flush()?;
        }
        Ok(())
    }

    /// Dénomination historique → staging (`entity_history_stage_init` doit
    /// avoir été appelé dans la transaction).
    fn push_hist(&mut self, d: EntityHistoryWriteItem) -> Result<(), SourceError> {
        self.hist.push(d);
        self.loaded += 1;
        if self.hist.len() >= BATCH {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), SourceError> {
        if !self.ents.is_empty() {
            self.handle
                .block_on(self.repo.entity_copy(&self.ents))
                .map_err(db_err)?;
            self.ents.clear();
        }
        if !self.hist.is_empty() {
            self.handle
                .block_on(self.repo.entity_history_stage_copy(&self.hist))
                .map_err(db_err)?;
            self.hist.clear();
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Catégorie d'annuaire — dérivation UNIQUE, un seul écrivain
// ---------------------------------------------------------------------------

/// Tokens (pliés) qui excluent une unité APE 69.10Z de la catégorie
/// `cabinets` : les autres professions du droit partagent le code —
/// études notariales, huissiers/commissaires de justice, AJ-MJ, greffiers
/// des tribunaux de commerce.
const NON_CABINET_TOKENS: &[&str] = &[
    "notair",
    "notari",
    "huissier",
    "commissaire",
    "mandataire",
    "administrateur",
    "greffe",
    "greffier",
];

/// Catégorie d'annuaire d'une unité légale SIRENE : droit public →
/// `personnes_publiques` ; APE 69.10Z (activités juridiques) sans token
/// d'une autre profession du droit → `cabinets` (structures d'exercice
/// d'avocats) ; le reste → `entreprises`.
fn sirene_category(nature: &'static str, ape: Option<&str>, folded: &str) -> &'static str {
    if nature == "morale_publique" {
        return "personnes_publiques";
    }
    if ape == Some("69.10Z") && !NON_CABINET_TOKENS.iter().any(|t| folded.contains(t)) {
        return "cabinets";
    }
    "entreprises"
}

// ---------------------------------------------------------------------------
// SIRENE — unités légales (personnes morales seules)
// ---------------------------------------------------------------------------

/// Nature depuis la catégorie juridique Insee : `1xxx` = entrepreneur
/// individuel (personne physique, chargée si diffusible) ; niveaux 4/7 =
/// droit public.
fn sirene_nature(cat: &str) -> Option<&'static str> {
    match cat.as_bytes().first()? {
        b'1' => Some("physique"),
        b'4' | b'7' => Some("morale_publique"),
        _ => Some("morale_privee"),
    }
}

async fn load_sirene(
    repo: &DecisionRepository<'_>,
    conn: &lj_pg::db::Connection,
    dir: &Path,
    with_history: bool,
) -> Result<u64> {
    let (_, stock) = fetch(
        "base-sirene-des-entreprises-et-de-leurs-etablissements-siren-siret",
        dir,
        |t| t.contains("StockUniteLegale") && !t.contains("Historique"),
        |f| f == "zip",
    )
    .await?;
    let history = if with_history {
        let (_, h) = fetch(
            "base-sirene-des-entreprises-et-de-leurs-etablissements-siren-siret",
            dir,
            |t| t.contains("StockUniteLegaleHistorique"),
            |f| f == "zip",
        )
        .await?;
        Some(h)
    } else {
        None
    };

    conn.batch_execute("BEGIN").await?;
    let cleared = repo.entity_namespace_clear("siren").await?;
    tracing::info!(cleared, "siren namespace emptied");

    // Sirens chargés (u32 : 9 chiffres) — filtre d'intégrité de l'historique.
    let mut sirens: HashSet<u32> = HashSet::new();
    let loaded = tokio::task::block_in_place(|| -> Result<u64, SourceError> {
        let mut loader = Loader::new(repo);
        let mut cols = Cols::new();
        for_each_csv_record(&stock, b',', |headers, rec| {
            let &[siren, cat, denom, sigle, etat, ape, statut, nom, nom_usage, prenom1, prenom_usuel, denom_usuelle] =
                cols.resolve(
                    headers,
                    &[
                        "siren",
                        "categorieJuridiqueUniteLegale",
                        "denominationUniteLegale",
                        "sigleUniteLegale",
                        "etatAdministratifUniteLegale",
                        "activitePrincipaleUniteLegale",
                        "statutDiffusionUniteLegale",
                        "nomUniteLegale",
                        "nomUsageUniteLegale",
                        "prenom1UniteLegale",
                        "prenomUsuelUniteLegale",
                        "denominationUsuelle1UniteLegale",
                    ],
                )?
            else {
                unreachable!("12 colonnes demandées")
            };
            let cat = field(rec, cat);
            let Some(nature) = sirene_nature(&cat) else {
                return Ok(());
            };
            // EI : personnes physiques diffusibles seulement, nom = PRENOM NOM
            // (usuels d'abord) ; alternatives = ordre inverse + nom commercial.
            // Morales : dénomination telle quelle.
            let (denom, alt_denominations) = if nature == "physique" {
                if field(rec, statut) != "O" {
                    return Ok(());
                }
                let nom = Some(field(rec, nom_usage))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| field(rec, nom));
                let prenom = Some(field(rec, prenom_usuel))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| field(rec, prenom1));
                if nom.is_empty() || prenom.is_empty() {
                    return Ok(());
                }
                let mut alts = vec![format!("{nom} {prenom}")];
                let usuelle = field(rec, denom_usuelle);
                if !usuelle.is_empty() {
                    alts.push(usuelle);
                }
                (format!("{prenom} {nom}"), alts)
            } else {
                let denom = field(rec, denom);
                if denom.is_empty() {
                    return Ok(());
                }
                (denom, Vec::new())
            };
            let siren = field(rec, siren);
            let Ok(siren_num) = siren.parse::<u32>() else {
                return Ok(());
            };
            sirens.insert(siren_num);
            let sigle = Some(field(rec, sigle)).filter(|s| !s.is_empty());
            let ape = Some(field(rec, ape)).filter(|a| !a.is_empty());
            let uid = format!("siren:{siren}");
            let folded = lj_fold::fold(&denom);
            let category = sirene_category(nature, ape.as_deref(), &folded);
            loader.push(EntityWriteItem {
                uid,
                nature,
                denomination: denom,
                sigle,
                forme: Some(cat).filter(|c| !c.is_empty()),
                active: field(rec, etat) == "A",
                surname_key: None,
                category,
                ape,
                barreau: None,
                alt_denominations,
                activity: None,
            })
        })?;
        loader.flush()?;
        Ok(loader.loaded)
    })?;

    // Historique : périodes CLOSES de dénomination (la courante est déjà là) —
    // stagées puis fusionnées d'un bloc dans `entity.denominations`.
    let mut hist_rows = 0u64;
    if let Some(hist) = history {
        repo.entity_history_stage_init().await?;
        hist_rows = tokio::task::block_in_place(|| -> Result<u64, SourceError> {
            let mut loader = Loader::new(repo);
            let mut cols = Cols::new();
            for_each_csv_record(&hist, b',', |headers, rec| {
                let &[siren, debut, fin, denom] = cols.resolve(
                    headers,
                    &["siren", "dateDebut", "dateFin", "denominationUniteLegale"],
                )?
                else {
                    unreachable!("4 colonnes demandées")
                };
                let denom = field(rec, denom);
                let fin = field(rec, fin);
                if denom.is_empty() || fin.is_empty() {
                    return Ok(());
                }
                let siren = field(rec, siren);
                if !siren.parse::<u32>().is_ok_and(|n| sirens.contains(&n)) {
                    return Ok(());
                }
                let parse_date = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok();
                loader.push_hist(EntityHistoryWriteItem {
                    entity_uid: format!("siren:{siren}"),
                    denomination: denom.clone(),
                    date_debut: parse_date(&field(rec, debut)),
                    date_fin: parse_date(&fin),
                })
            })?;
            loader.flush()?;
            Ok(loader.loaded)
        })?;
        let merged = repo.entity_history_merge().await?;
        tracing::info!(merged, "history merged into entity.denominations");
    }
    conn.batch_execute("COMMIT").await?;
    tracing::info!(loaded, hist_rows, "sirene loaded");
    Ok(loaded)
}

// ---------------------------------------------------------------------------
// RNA — associations
// ---------------------------------------------------------------------------

async fn load_rna(
    repo: &DecisionRepository<'_>,
    conn: &lj_pg::db::Connection,
    dir: &Path,
) -> Result<u64> {
    let (_, path) = fetch(
        "repertoire-national-des-associations",
        dir,
        |t| t.starts_with("rna_waldec_"),
        |f| f == "zip",
    )
    .await?;
    conn.batch_execute("BEGIN").await?;
    let cleared = repo.entity_namespace_clear("rna").await?;
    tracing::info!(cleared, "rna namespace emptied");
    let loaded = tokio::task::block_in_place(|| -> Result<u64, SourceError> {
        let mut loader = Loader::new(repo);
        let mut cols = Cols::new();
        let mut seen: HashSet<String> = HashSet::new();
        for_each_csv_record(&path, b';', |headers, rec| {
            let &[id, titre, titre_court, position] =
                cols.resolve(headers, &["id", "titre", "titre_court", "position"])?
            else {
                unreachable!("4 colonnes demandées")
            };
            let titre = field(rec, titre);
            let id = field(rec, id);
            if titre.is_empty() || id.is_empty() || !seen.insert(id.clone()) {
                return Ok(());
            }
            let uid = format!("rna:{id}");
            loader.push(EntityWriteItem {
                uid,
                nature: "morale_privee",
                sigle: Some(field(rec, titre_court)).filter(|s| !s.is_empty() && *s != titre),
                forme: Some("association".to_string()),
                active: field(rec, position) == "A",
                surname_key: None,
                category: "associations",
                ape: None,
                barreau: None,
                alt_denominations: Vec::new(),
                activity: None,
                denomination: titre,
            })
        })?;
        loader.flush()?;
        Ok(loader.loaded)
    })?;
    conn.batch_execute("COMMIT").await?;
    Ok(loaded)
}

// ---------------------------------------------------------------------------
// Avocats — annuaire CNB (open data, Licence Ouverte 2.0) : identité par code
// CNBF, upsert vivant (on ne supprime plus, on observe), spans d'activité,
// affiliations à la structure d'exercice (siren déclaré).
// ---------------------------------------------------------------------------

/// A lawyer row of a snapshot, under the identity it stands for.
#[derive(Debug)]
pub(super) struct AvocatRow {
    /// `cnb:{avCnbfCode}`, or what a snapshot without the code bridges to.
    pub(super) uid: String,
    /// Nom de barreau brut (pour `forme`).
    pub(super) barreau: String,
    pub(super) barreau_slug: String,
    pub(super) nom: String,
    pub(super) prenom: String,
    /// `avBarEntree` — entrée au barreau, déclarée.
    pub(super) bar_entree: Option<chrono::NaiveDate>,
    /// `cbSiretSiren` — siren de la structure d'exercice (EI de l'avocat pour
    /// les CABI, société sinon).
    pub(super) siren: Option<String>,
    /// `acDateEntree` — entrée dans la structure courante.
    pub(super) date_entree: Option<chrono::NaiveDate>,
}

impl AvocatRow {
    /// Slug `nom-prenom` — le 3ᵉ segment des uids legacy
    /// `cnb:{barreau}:{nom}-{prenom}`.
    pub(super) fn nomprenom_slug(&self) -> String {
        format!("{}-{}", slug(&self.nom), slug(&self.prenom))
    }
}

/// Date `YYYYMMDD` stricte du CSV CNB.
pub(super) fn parse_yyyymmdd(s: &str) -> Option<chrono::NaiveDate> {
    if s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit()) {
        chrono::NaiveDate::parse_from_str(s, "%Y%m%d").ok()
    } else {
        None
    }
}

/// Un code CNBF valide est numérique — les lignes de pied de fichier du CSV
/// portent le marqueur `V1.8` dans cette colonne.
fn is_cnbf(code: &str) -> bool {
    !code.is_empty() && code.bytes().all(|b| b.is_ascii_digit())
}

/// Date du snapshot depuis le titre de la ressource
/// (`annuaire-avocats-20260717.CSV`) : dernière fenêtre de 8 chiffres.
pub(super) fn snapshot_date_from_title(title: &str) -> Option<chrono::NaiveDate> {
    let digits: Vec<(usize, char)> = title.char_indices().collect();
    let bytes = title.as_bytes();
    let mut best = None;
    let mut run = 0usize;
    for (i, _) in digits {
        if bytes[i].is_ascii_digit() {
            run += 1;
            if run >= 8 {
                best = parse_yyyymmdd(&title[i + 1 - 8..=i]).or(best);
            }
        } else {
            run = 0;
        }
    }
    best
}

/// Résultat du diff d'upsert CNB (pure, testé) — ce qu'il faut écrire, et
/// les identités nouvelles/réactivées pour la sonde du delta.
struct CnbDiff {
    inserts: Vec<EntityWriteItem>,
    updates: Vec<CnbEntityUpdate>,
    deactivated: u64,
    reactivated: u64,
    /// Uids apparus ou réactivés à ce snapshot — surfaces à sonder contre
    /// les clés pendantes.
    added_uids: Vec<String>,
}

/// Ouvre un span d'activité `cnb` (`du` optionnel).
fn cnb_span(du: Option<chrono::NaiveDate>) -> serde_json::Value {
    match du {
        Some(d) => serde_json::json!({"du": d.to_string(), "src": "cnb"}),
        None => serde_json::json!({"src": "cnb"}),
    }
}

/// Ferme les spans `cnb` ouverts (`au` absent) d'un tableau d'activité.
fn close_open_spans(activity: &mut serde_json::Value, au: chrono::NaiveDate) {
    if let Some(spans) = activity.as_array_mut() {
        for s in spans {
            if s.get("src").and_then(|v| v.as_str()) == Some("cnb") && s.get("au").is_none() {
                s["au"] = serde_json::Value::String(au.to_string());
            }
        }
    }
}

/// Diff d'upsert : CSV du snapshot × état `cnb:` en base → écritures.
/// Présent = actif (réapparition rouvre un span à la date du snapshot) ;
/// absent = inactif, spans clos, la ligne reste ; un changement de barreau est
/// une mutation d'attribut, pas d'identité. `activity` absent en base (stock du
/// re-key) est semé depuis `avBarEntree`.
fn cnb_diff(
    rows: &[AvocatRow],
    existing: &[CnbEntityState],
    kept: &HashSet<String>,
    snapshot: chrono::NaiveDate,
    first_seen: Option<chrono::NaiveDate>,
) -> CnbDiff {
    let by_uid: std::collections::HashMap<&str, &CnbEntityState> =
        existing.iter().map(|s| (s.uid.as_str(), s)).collect();
    let mut diff = CnbDiff {
        inserts: Vec::new(),
        updates: Vec::new(),
        deactivated: 0,
        reactivated: 0,
        added_uids: Vec::new(),
    };
    let mut seen: HashSet<&str> = HashSet::with_capacity(rows.len());
    for r in rows {
        if !seen.insert(&r.uid) {
            continue;
        }
        let uid = r.uid.clone();
        let denomination = format!("{} {}", r.prenom, r.nom).trim().to_string();
        let surname = surname_key(&r.nom);
        let forme = Some(format!("avocat ({})", r.barreau));
        match by_uid.get(uid.as_str()) {
            None => {
                diff.added_uids.push(uid.clone());
                diff.inserts.push(EntityWriteItem {
                    uid,
                    nature: "physique",
                    denomination,
                    sigle: None,
                    forme,
                    active: true,
                    surname_key: surname,
                    category: "avocats",
                    ape: None,
                    barreau: Some(r.barreau_slug.clone()),
                    alt_denominations: Vec::new(),
                    activity: Some(serde_json::json!([cnb_span(r.bar_entree.or(first_seen))])),
                });
            }
            Some(state) => {
                let mut activity = state.activity.clone();
                match activity.as_mut() {
                    // Stock du re-key sans évidence : semé depuis la date
                    // déclarée d'entrée au barreau.
                    None => activity = Some(serde_json::json!([cnb_span(r.bar_entree)])),
                    Some(act) => {
                        if !state.active {
                            // Réapparition (come and go) : nouveau span
                            // observé au snapshot.
                            if let Some(spans) = act.as_array_mut() {
                                spans.push(cnb_span(Some(snapshot)));
                            }
                        } else if let Some(spans) = act.as_array_mut() {
                            // Baseline mono-span sans du : la source déclare
                            // l'entrée au barreau, on l'adopte.
                            if let (1, Some(d)) = (spans.len(), r.bar_entree) {
                                let s = &mut spans[0];
                                if s.get("du").is_none() && s.get("au").is_none() {
                                    s["du"] = serde_json::Value::String(d.to_string());
                                }
                            }
                        }
                    }
                }
                if !state.active {
                    diff.reactivated += 1;
                    diff.added_uids.push(uid.clone());
                }
                let changed = !state.active
                    || state.barreau.as_deref() != Some(r.barreau_slug.as_str())
                    || state.denomination != denomination
                    || state.surname_key != surname
                    || state.activity != activity;
                if changed {
                    diff.updates.push(CnbEntityUpdate {
                        uid,
                        active: true,
                        activity,
                        barreau: Some(r.barreau_slug.clone()),
                        denomination,
                        forme,
                        surname_key: surname,
                    });
                }
            }
        }
    }
    let present: HashSet<&str> = rows.iter().map(|r| r.uid.as_str()).collect();
    for state in existing {
        if state.active && !present.contains(state.uid.as_str()) && !kept.contains(&state.uid) {
            diff.deactivated += 1;
            let mut activity = state
                .activity
                .clone()
                .unwrap_or_else(|| serde_json::json!([cnb_span(None)]));
            close_open_spans(&mut activity, snapshot);
            diff.updates.push(CnbEntityUpdate {
                uid: state.uid.clone(),
                active: false,
                activity: Some(activity),
                barreau: state.barreau.clone(),
                denomination: state.denomination.clone(),
                forme: state.forme.clone(),
                surname_key: state.surname_key.clone(),
            });
        }
    }
    diff
}

/// A CNB snapshot, by whether it carries `avCnbfCode`.
pub(super) enum CnbRows {
    Coded(Vec<AvocatRow>),
    /// The format of every snapshot since 2022 but the one of 17/07/2026.
    Reduced(Vec<HistRow>),
}

/// Télécharge le snapshot CNB courant et le parse (filtres pieds de fichier
/// + identité). Partagé entre le reload upsert et le re-key one-shot.
pub(super) async fn fetch_avocat_rows(dir: &Path) -> Result<(CnbRows, chrono::NaiveDate)> {
    let (res, path) = fetch(
        "annuaire-des-avocats-de-france",
        dir,
        |_| true,
        |f| f.eq_ignore_ascii_case("csv"),
    )
    .await?;
    let snapshot =
        snapshot_date_from_title(&res.title).unwrap_or_else(|| chrono::Utc::now().date_naive());
    let mut header = Vec::new();
    std::io::BufRead::read_until(
        &mut std::io::BufReader::new(std::fs::File::open(&path)?),
        b'\n',
        &mut header,
    )?;
    if !header.windows(10).any(|w| w == b"avCnbfCode") {
        let rows = tokio::task::block_in_place(|| parse_snapshot(&path))?;
        return Ok((CnbRows::Reduced(rows), snapshot));
    }

    // Parse complet en mémoire : le diff a besoin du snapshot entier face à
    // l'état en base.
    let rows = tokio::task::block_in_place(|| -> Result<Vec<AvocatRow>, SourceError> {
        let mut rows = Vec::new();
        let mut cols = Cols::new();
        for_each_csv_record(&path, b';', |headers, rec| {
            let &[cnbf, barreau, nom, prenom, bar_entree, siren, date_entree] = cols.resolve(
                headers,
                &[
                    "avCnbfCode",
                    "Barreau",
                    "avNom",
                    "avPrenom",
                    "avBarEntree",
                    "cbSiretSiren",
                    "acDateEntree",
                ],
            )?
            else {
                unreachable!("7 colonnes demandées")
            };
            let cnbf = field(rec, cnbf);
            let nom = field(rec, nom);
            // Pieds de fichier (V1.8) + lignes sans identité.
            if !is_cnbf(&cnbf) || nom.is_empty() {
                return Ok(());
            }
            let barreau = field(rec, barreau);
            let siren = Some(field(rec, siren))
                .filter(|s| s.len() == 9 && s.bytes().all(|b| b.is_ascii_digit()));
            rows.push(AvocatRow {
                uid: format!("cnb:{cnbf}"),
                barreau_slug: slug(&barreau),
                barreau,
                prenom: field(rec, prenom),
                bar_entree: parse_yyyymmdd(&field(rec, bar_entree)),
                siren,
                date_entree: parse_yyyymmdd(&field(rec, date_entree)),
                nom,
            });
            Ok(())
        })?;
        Ok(rows)
    })?;
    Ok((CnbRows::Coded(rows), snapshot))
}

async fn load_avocats(
    repo: &DecisionRepository<'_>,
    conn: &lj_pg::db::Connection,
    dir: &Path,
) -> Result<u64> {
    let (snapshot_rows, snapshot) = fetch_avocat_rows(dir).await?;
    // L'UPDATE en masse du registre (index GIN d'entity) dépasse le
    // `statement_timeout` du pool.
    conn.batch_execute("BEGIN; SET LOCAL statement_timeout = 0")
        .await?;
    let existing = repo.entity_cnb_states().await?;
    let open = repo.affiliation_open_edges("cnb").await?;
    let (rows, kept, first_seen) = match snapshot_rows {
        CnbRows::Coded(rows) => {
            // An active legacy uid is someone a coded snapshot lists under a code:
            // left as is, each reload would hold them twice.
            let legacy_active = existing
                .iter()
                .filter(|s| s.active && !s.uid[4..].bytes().all(|b| b.is_ascii_digit()))
                .count();
            if legacy_active > 0 {
                anyhow::bail!(
                    "{legacy_active} active legacy cnb uids: run `lj-ingest rekey-cnb` \
                     before reloading a snapshot with avCnbfCode"
                );
            }
            (rows, HashSet::new(), None)
        }
        CnbRows::Reduced(rows) => {
            let bridged = cnb_reduced::bridge(rows, &existing, &open);
            let c = &bridged.counts;
            tracing::info!(
                exact = c.exact,
                siren = c.siren,
                name = c.name,
                new = c.new,
                abstained = c.abstained,
                kept = bridged.kept.len(),
                %snapshot,
                "cnb snapshot without avCnbfCode bridged"
            );
            if c.mapped_share() < cnb_reduced::MIN_MAPPED {
                anyhow::bail!(
                    "cnb snapshot {snapshot} maps {:.1} % of its rows onto known identities, \
                     under {:.0} %: refusing to write",
                    100.0 * c.mapped_share(),
                    100.0 * cnb_reduced::MIN_MAPPED
                );
            }
            (bridged.rows, bridged.kept, Some(snapshot))
        }
    };

    let diff = cnb_diff(&rows, &existing, &kept, snapshot, first_seen);
    for batch in diff.inserts.chunks(BATCH) {
        repo.entity_copy(batch).await?;
    }
    let updated = repo.entity_cnb_update_bulk(&diff.updates).await?;

    // Affiliations déclarées (source `cnb`) : diff des arêtes ouvertes contre le
    // siren de structure du snapshot.
    let open_by_uid: std::collections::HashMap<&str, (&str, Option<chrono::NaiveDate>)> = open
        .iter()
        .map(|(e, s, du)| (e.as_str(), (s.as_str(), *du)))
        .collect();
    let mut desired: std::collections::HashMap<String, (String, Option<chrono::NaiveDate>)> =
        std::collections::HashMap::new();
    for r in &rows {
        if let Some(siren) = &r.siren {
            desired.insert(r.uid.clone(), (format!("siren:{siren}"), r.date_entree));
        }
    }
    let mut to_close: Vec<String> = Vec::new();
    let mut to_insert: Vec<AffiliationEdge> = Vec::new();
    let mut to_backfill: Vec<(String, chrono::NaiveDate)> = Vec::new();
    for (uid, (structure, du)) in &desired {
        match open_by_uid.get(uid.as_str()) {
            None => to_insert.push(AffiliationEdge {
                entity_uid: uid.clone(),
                structure_uid: structure.clone(),
                du: du.or(first_seen),
                au: None,
            }),
            Some((open_structure, _)) if *open_structure != structure.as_str() => {
                // Mobilité observée : l'ancienne arête se clôt au snapshot,
                // la nouvelle s'ouvre à la date déclarée (sinon au snapshot).
                to_close.push(uid.clone());
                to_insert.push(AffiliationEdge {
                    entity_uid: uid.clone(),
                    structure_uid: structure.clone(),
                    du: du.or(Some(snapshot)),
                    au: None,
                });
            }
            Some((_, open_du)) => {
                if let (None, Some(d)) = (open_du, du) {
                    to_backfill.push((uid.clone(), *d));
                }
            }
        }
    }
    for (uid, _, _) in &open {
        if !desired.contains_key(uid) && !kept.contains(uid) {
            to_close.push(uid.clone());
        }
    }
    let closed = repo
        .affiliation_close_bulk("cnb", &to_close, snapshot)
        .await?;
    let opened = repo.affiliation_insert_bulk("cnb", &to_insert).await?;
    let backfilled = repo.affiliation_backfill_du("cnb", &to_backfill).await?;

    // Delta at-write : les identités nouvelles/réactivées dé-lient d'abord ce que
    // leur arrivée rend ambigu à date (patronyme, nom complet), puis attirent les
    // clés pendantes qui les visaient.
    let delta_unlinked = repo.cnb_delta_unlink(&diff.added_uids).await?;
    let delta_linked = repo
        .resolve_pending_counsel_for_added(&diff.added_uids)
        .await?;
    conn.batch_execute("COMMIT").await?;

    tracing::info!(
        rows = rows.len(),
        inserted = diff.inserts.len(),
        updated,
        deactivated = diff.deactivated,
        reactivated = diff.reactivated,
        edges_closed = closed,
        edges_opened = opened,
        edges_backfilled = backfilled,
        delta_unlinked,
        delta_linked,
        %snapshot,
        "cnb upsert"
    );
    Ok(rows.len() as u64)
}

// ---------------------------------------------------------------------------
// Avocats aux Conseils — annuaire OACC (snapshot curé)
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct OaccData {
    /// Date de capture du snapshot curé — `au` des arêtes que ce reload ferme.
    captured_at: chrono::NaiveDate,
    avocats: Vec<OaccAvocat>,
    societes: Vec<OaccSociete>,
}

#[derive(serde::Deserialize)]
struct OaccAvocat {
    nom: String,
    prenom: String,
    /// Nom d'affichage « Prénom NOM » — dénomination de résolution.
    full_name: String,
    /// Courriel de contact — celui du CABINET, d'où l'affiliation.
    email: Option<String>,
}

#[derive(serde::Deserialize)]
struct OaccSociete {
    /// Raison sociale officielle (SCP/SARL/Cabinet…, tous les associés).
    nom: String,
    email: Option<String>,
}

/// Courriel comparable : casse et blancs de bord neutralisés.
fn mail_key(email: Option<&String>) -> Option<String> {
    let e = email?.trim().to_ascii_lowercase();
    (!e.is_empty()).then_some(e)
}

/// Charge le registre des avocats aux Conseils depuis le snapshot curé JSON
/// (même racine `ingest/corpus` que `load-legal-corpus`). Dataset minuscule :
/// deux COPY directs dans la transaction, pas de streaming.
///
/// Uid : `oacc:<nom>-<prenom>` (avocats, comme cnb sans segment barreau) et
/// `oacc:firm-<raison-pliée>` (sociétés). Dénomination « Prénom NOM » /
/// raison sociale, pliée `lj_fold::fold`.
async fn load_avocats_conseils(
    repo: &DecisionRepository<'_>,
    conn: &lj_pg::db::Connection,
    path: &Path,
) -> Result<u64> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        anyhow!(
            "snapshot oacc absent ({}) — lancer scripts/curate_oacc.py : {e}",
            path.display()
        )
    })?;
    let data: OaccData = serde_json::from_str(&raw)?;

    let mut ents: Vec<EntityWriteItem> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    // Courriel de cabinet → uid de société, quand il en désigne UNE seule
    // (un courriel partagé par deux raisons sociales ne tranche rien).
    let mut firm_by_mail: HashMap<String, Option<String>> = HashMap::new();
    let mut avocat_mail: Vec<(String, String)> = Vec::new();

    for av in &data.avocats {
        if av.nom.is_empty() {
            continue;
        }
        let uid = format!("oacc:{}-{}", slug(&av.nom), slug(&av.prenom));
        if !seen.insert(uid.clone()) {
            continue;
        }
        if let Some(m) = mail_key(av.email.as_ref()) {
            avocat_mail.push((uid.clone(), m));
        }
        ents.push(EntityWriteItem {
            uid,
            nature: "physique",
            denomination: av.full_name.trim().to_string(),
            sigle: None,
            forme: Some("avocat aux Conseils".to_string()),
            active: true,
            surname_key: None,
            category: "avocats",
            ape: None,
            barreau: None,
            alt_denominations: Vec::new(),
            activity: None,
        });
    }

    for sc in &data.societes {
        let nom = sc.nom.trim().to_string();
        if nom.is_empty() {
            continue;
        }
        let uid = format!("oacc:firm-{}", firm_slug(&nom));
        if !seen.insert(uid.clone()) {
            continue;
        }
        if let Some(m) = mail_key(sc.email.as_ref()) {
            firm_by_mail
                .entry(m)
                .and_modify(|v| *v = None)
                .or_insert(Some(uid.clone()));
        }
        ents.push(EntityWriteItem {
            uid,
            nature: "morale_privee",
            denomination: nom,
            sigle: None,
            forme: Some("société d'avocats aux Conseils".to_string()),
            active: true,
            surname_key: None,
            category: "cabinets",
            ape: None,
            barreau: None,
            alt_denominations: Vec::new(),
            activity: None,
        });
    }

    // Affiliations (source `oacc`) : l'avocat aux Conseils porte le courriel de
    // son cabinet, qui le désigne. Snapshot sans dates — les arêtes s'ouvrent sans
    // `du` (début inconnu), le diff clôt les départs.
    let desired: Vec<(String, String)> = avocat_mail
        .iter()
        .filter_map(|(uid, mail)| match firm_by_mail.get(mail) {
            Some(Some(firm)) => Some((uid.clone(), firm.clone())),
            _ => None,
        })
        .collect();

    conn.batch_execute("BEGIN").await?;
    let cleared = repo.entity_namespace_clear("oacc").await?;
    tracing::info!(cleared, "oacc namespace emptied");
    repo.entity_copy(&ents).await?;

    let open = repo.affiliation_open_edges("oacc").await?;
    let open_by_uid: HashMap<&str, &str> = open
        .iter()
        .map(|(e, s, _)| (e.as_str(), s.as_str()))
        .collect();
    let desired_uids: HashSet<&str> = desired.iter().map(|(u, _)| u.as_str()).collect();
    let mut to_close: Vec<String> = Vec::new();
    let mut to_insert: Vec<AffiliationEdge> = Vec::new();
    for (uid, firm) in &desired {
        let edge = |du| AffiliationEdge {
            entity_uid: uid.clone(),
            structure_uid: firm.clone(),
            du,
            au: None,
        };
        match open_by_uid.get(uid.as_str()) {
            Some(open_firm) if *open_firm == firm.as_str() => {}
            Some(_) => {
                to_close.push(uid.clone());
                to_insert.push(edge(Some(data.captured_at)));
            }
            None => to_insert.push(edge(None)),
        }
    }
    for (uid, _, _) in &open {
        if !desired_uids.contains(uid.as_str()) {
            to_close.push(uid.clone());
        }
    }
    let edges_closed = repo
        .affiliation_close_bulk("oacc", &to_close, data.captured_at)
        .await?;
    let edges_opened = repo.affiliation_insert_bulk("oacc", &to_insert).await?;
    conn.batch_execute("COMMIT").await?;

    tracing::info!(
        avocats = data.avocats.len(),
        societes = data.societes.len(),
        edges = desired.len(),
        edges_closed,
        edges_opened,
        "oacc loaded"
    );
    Ok(ents.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sirene_category_cabinets_apres_exclusions() {
        assert_eq!(
            sirene_category("morale_privee", Some("69.10Z"), "scp martin bernard"),
            "cabinets"
        );
        assert_eq!(
            sirene_category("morale_privee", Some("69.10Z"), "selarl dupont avocats"),
            "cabinets"
        );
        for (ape, folded) in [
            (Some("69.10Z"), "office notarial de la plaine"),
            (Some("69.10Z"), "selarl martin huissiers de justice"),
            (Some("69.10Z"), "etude durand commissaire de justice"),
            (Some("69.10Z"), "aj partenaires administrateurs judiciaires"),
            (Some("62.01Z"), "cabinet conseil informatique"),
            (None, "scp fictive sans ape"),
        ] {
            assert_eq!(
                sirene_category("morale_privee", ape, folded),
                "entreprises",
                "{ape:?} {folded:?}"
            );
        }
        assert_eq!(
            sirene_category("morale_publique", Some("69.10Z"), "chambre des notaires"),
            "personnes_publiques"
        );
    }

    // ── Upsert CNB ──────────────────────────────────────────────────────────

    fn d(s: &str) -> chrono::NaiveDate {
        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn row(cnbf: &str, barreau: &str, nom: &str, prenom: &str) -> AvocatRow {
        AvocatRow {
            uid: format!("cnb:{cnbf}"),
            barreau: barreau.to_string(),
            barreau_slug: slug(barreau),
            nom: nom.to_string(),
            prenom: prenom.to_string(),
            bar_entree: Some(d("2019-01-01")),
            siren: None,
            date_entree: None,
        }
    }

    fn state(uid: &str, active: bool, activity: Option<serde_json::Value>) -> CnbEntityState {
        CnbEntityState {
            uid: uid.to_string(),
            active,
            activity,
            barreau: Some("agen".to_string()),
            denomination: "Louise Bertin".to_string(),
            surname_key: None,
            forme: Some("avocat (AGEN)".to_string()),
        }
    }

    #[test]
    fn cnb_diff_upsert_spec() {
        let snapshot = d("2026-07-17");
        let none = HashSet::new();

        let diff = cnb_diff(
            &[row("900001", "AGEN", "BERTIN", "Louise")],
            &[],
            &none,
            snapshot,
            None,
        );
        assert_eq!(diff.inserts.len(), 1);
        assert_eq!(diff.inserts[0].uid, "cnb:900001");
        assert_eq!(
            diff.inserts[0].activity,
            Some(serde_json::json!([{"du": "2019-01-01", "src": "cnb"}]))
        );
        assert_eq!(diff.added_uids, vec!["cnb:900001"]);

        let cur = state(
            "cnb:900001",
            true,
            Some(serde_json::json!([{"du": "2019-01-01", "src": "cnb"}])),
        );
        let mut cur = cur;
        cur.denomination = "Louise BERTIN".to_string();
        let rows = [row("900001", "AGEN", "BERTIN", "Louise")];
        let diff = cnb_diff(&rows, std::slice::from_ref(&cur), &none, snapshot, None);
        assert!(diff.inserts.is_empty() && diff.updates.is_empty());

        let diff = cnb_diff(&[], std::slice::from_ref(&cur), &none, snapshot, None);
        assert_eq!(diff.deactivated, 1);
        assert_eq!(diff.updates.len(), 1);
        assert!(!diff.updates[0].active);
        assert_eq!(
            diff.updates[0].activity,
            Some(serde_json::json!([{"du": "2019-01-01", "au": "2026-07-17", "src": "cnb"}]))
        );

        let gone = state(
            "cnb:900001",
            false,
            Some(serde_json::json!([{"du": "2019-01-01", "au": "2024-01-01", "src": "cnb"}])),
        );
        let diff = cnb_diff(&rows, std::slice::from_ref(&gone), &none, snapshot, None);
        assert_eq!(diff.reactivated, 1);
        assert_eq!(diff.added_uids, vec!["cnb:900001"]);
        assert_eq!(
            diff.updates[0].activity,
            Some(serde_json::json!([
                {"du": "2019-01-01", "au": "2024-01-01", "src": "cnb"},
                {"du": "2026-07-17", "src": "cnb"}
            ]))
        );

        let diff = cnb_diff(
            &[row("900001", "LYON", "BERTIN", "Louise")],
            std::slice::from_ref(&cur),
            &none,
            snapshot,
            None,
        );
        assert!(diff.inserts.is_empty());
        assert_eq!(diff.updates.len(), 1);
        assert_eq!(diff.updates[0].barreau.as_deref(), Some("lyon"));
        assert!(diff.updates[0].active);

        let bare = state("cnb:900001", true, None);
        let diff = cnb_diff(&rows, std::slice::from_ref(&bare), &none, snapshot, None);
        assert_eq!(
            diff.updates[0].activity,
            Some(serde_json::json!([{"du": "2019-01-01", "src": "cnb"}]))
        );

        let diff = cnb_diff(
            &[
                row("900001", "AGEN", "BERTIN", "Louise"),
                row("900001", "LYON", "BERTIN", "Louise"),
            ],
            &[],
            &none,
            snapshot,
            None,
        );
        assert_eq!(diff.inserts.len(), 1);
        assert_eq!(diff.inserts[0].barreau.as_deref(), Some("agen"));

        let kept = HashSet::from(["cnb:900001".to_string()]);
        let diff = cnb_diff(&[], std::slice::from_ref(&cur), &kept, snapshot, None);
        assert_eq!(diff.deactivated, 0);
        assert!(diff.updates.is_empty());

        let mut unsworn = row("900002", "AGEN", "BERTIN", "Louise");
        unsworn.bar_entree = None;
        let diff = cnb_diff(&[unsworn], &[], &none, snapshot, Some(snapshot));
        assert_eq!(
            diff.inserts[0].activity,
            Some(serde_json::json!([{"du": "2026-07-17", "src": "cnb"}]))
        );
    }

    #[test]
    fn cnb_csv_guards() {
        assert!(!is_cnbf("V1.8"));
        assert!(!is_cnbf(""));
        assert!(is_cnbf("900001"));
        assert_eq!(parse_yyyymmdd("20190101"), Some(d("2019-01-01")));
        assert_eq!(parse_yyyymmdd("1"), None);
        assert_eq!(parse_yyyymmdd("20191301"), None);
        assert_eq!(
            snapshot_date_from_title("annuaire-avocats-20260717.CSV"),
            Some(d("2026-07-17"))
        );
        assert_eq!(snapshot_date_from_title("annuaire-avocats.csv"), None);
    }
}
