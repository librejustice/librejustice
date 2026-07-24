//! Chargement des registres d'entités (ADR 0179) : SIRENE (unités légales
//! Insee), RNA (associations), annuaire des avocats (CNB, open data).
//!
//! Découverte + download via `lj_sources::registries` (API dataset
//! data.gouv, URLs de ressources datées) ; chargement par remplacement de
//! namespace + COPY binaire (`lj_store` — idempotent, règle #7). Le pliage
//! des dénominations est `fold_stable` (le MÊME que l'extraction NER — la
//! résolution comparera plié à plié) avec blancs réduits à un.

use crate::config::Settings;
use anyhow::{anyhow, Result};
use lj_core::text::fold_stable;
use lj_sources::error::SourceError;
use lj_sources::registries::{
    datagouv_latest_resource, decode_field, download_resource, for_each_csv_record,
    RegistryResource,
};
use lj_store::repository::{DecisionRepository, EntityHistoryWriteItem, EntityWriteItem};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Sources de registres (clap `value_enum`).
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum RegistrySource {
    /// Unités légales Insee (namespace `siren:`) : personnes morales + EI
    /// diffusibles (ADR 0249).
    Sirene,
    /// Répertoire national des associations (namespace `rna:`).
    Rna,
    /// Annuaire des avocats de France, CNB (namespace `cnb:`).
    Avocats,
    /// Annuaire des avocats aux Conseils (Ordre au CE + Cass., namespace
    /// `oacc:`) — snapshot curé sous `ingest/corpus/registries/oacc.json`
    /// (ADR 0190), pas de bulk data.gouv.
    AvocatsConseils,
}

const BATCH: usize = 20_000;

/// Pliage canonique de résolution : `fold_stable` + blancs réduits à un.
fn canon(s: &str) -> String {
    fold_stable(s)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Slug d'uid : pliage canonique, blancs et apostrophes en tirets.
fn slug(s: &str) -> String {
    canon(s).replace([' ', '\''], "-")
}

/// Clé patronyme composé (ADR 0195) : nom-seul plié, tirets normalisés en
/// espaces — `None` si mono-token (le nom-seul simple reste irrésoluble).
fn surname_key(nom: &str) -> Option<String> {
    let k = canon(nom)
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
    for c in canon(s).chars() {
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
        lj_store::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let (namespace, loaded) = match source {
        RegistrySource::Sirene => (
            "siren",
            load_sirene(&repo, &conn, &dir, with_history).await?,
        ),
        RegistrySource::Rna => ("rna", load_rna(&repo, &conn, &dir).await?),
        RegistrySource::Avocats => ("cnb", load_avocats(&repo, &conn, &dir).await?),
        RegistrySource::AvocatsConseils => {
            // Snapshot curé (règle #17) : `ingest/corpus/registries/oacc.json`,
            // pas de download data.gouv (annuaire sans bulk, ADR 0190).
            let path = settings
                .legal_corpus_dir()
                .join("registries")
                .join("oacc.json");
            ("oacc", load_avocats_conseils(&repo, &conn, &path).await?)
        }
    };
    let count = repo.entity_count(namespace).await?;
    tracing::info!(namespace, loaded, count, "registre chargé");
    println!("registries {namespace} : {loaded} entités chargées ({count} en base).");
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

fn field(rec: &csv::ByteRecord, i: usize) -> String {
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
// Catégorie d'annuaire (ADR 0239) — dérivation UNIQUE, un seul écrivain
// ---------------------------------------------------------------------------

/// Tokens (pliés) qui excluent une unité APE 69.10Z de la catégorie
/// `cabinets` : les autres professions du droit partagent le code —
/// études notariales, huissiers/commissaires de justice, AJ-MJ, greffiers
/// des tribunaux de commerce. Règle calibrée sur le stock (working-note
/// du plan ADR 0239).
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
/// individuel (personne physique, chargée si diffusible — ADR 0249) ;
/// niveaux 4/7 = droit public.
fn sirene_nature(cat: &str) -> Option<&'static str> {
    match cat.as_bytes().first()? {
        b'1' => Some("physique"),
        b'4' | b'7' => Some("morale_publique"),
        _ => Some("morale_privee"),
    }
}

async fn load_sirene(
    repo: &DecisionRepository<'_>,
    conn: &lj_store::db::Connection,
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
    tracing::info!(cleared, "namespace siren vidé");

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
            // EI (ADR 0249) : personnes physiques diffusibles seulement,
            // nom = PRENOM NOM (usuels d'abord) ; alternatives = ordre
            // inverse + nom commercial. Morales : dénomination telle quelle.
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
            let folded = canon(&denom);
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
            })
        })?;
        loader.flush()?;
        Ok(loader.loaded)
    })?;

    // Historique : périodes CLOSES de dénomination (la courante est déjà là) —
    // stagées puis fusionnées d'un bloc dans `entity.denominations` (ADR 0245).
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
        tracing::info!(merged, "historique fusionné dans entity.denominations");
    }
    conn.batch_execute("COMMIT").await?;
    tracing::info!(loaded, hist_rows, "sirene chargé");
    Ok(loaded)
}

// ---------------------------------------------------------------------------
// RNA — associations
// ---------------------------------------------------------------------------

async fn load_rna(
    repo: &DecisionRepository<'_>,
    conn: &lj_store::db::Connection,
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
    tracing::info!(cleared, "namespace rna vidé");
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
// Avocats — annuaire CNB (open data, Licence Ouverte 2.0)
// ---------------------------------------------------------------------------

async fn load_avocats(
    repo: &DecisionRepository<'_>,
    conn: &lj_store::db::Connection,
    dir: &Path,
) -> Result<u64> {
    let (_, path) = fetch(
        "annuaire-des-avocats-de-france",
        dir,
        |_| true,
        |f| f.eq_ignore_ascii_case("csv"),
    )
    .await?;
    conn.batch_execute("BEGIN").await?;
    let cleared = repo.entity_namespace_clear("cnb").await?;
    tracing::info!(cleared, "namespace cnb vidé");
    let loaded = tokio::task::block_in_place(|| -> Result<u64, SourceError> {
        let mut loader = Loader::new(repo);
        let mut cols = Cols::new();
        let mut seen: HashSet<String> = HashSet::new();
        for_each_csv_record(&path, b';', |headers, rec| {
            let &[barreau, nom, prenom] =
                cols.resolve(headers, &["Barreau", "avNom", "avPrenom"])?
            else {
                unreachable!("3 colonnes demandées")
            };
            let nom = field(rec, nom);
            let prenom = field(rec, prenom);
            let barreau = field(rec, barreau);
            if nom.is_empty() {
                return Ok(());
            }
            // Pas d'id source : la clé naturelle est (barreau, nom, prénom).
            let barreau_slug = slug(&barreau);
            let uid = format!("cnb:{barreau_slug}:{}-{}", slug(&nom), slug(&prenom));
            if !seen.insert(uid.clone()) {
                return Ok(());
            }
            let denomination = format!("{prenom} {nom}").trim().to_string();
            loader.push(EntityWriteItem {
                uid,
                nature: "physique",
                denomination,
                sigle: None,
                forme: Some(format!("avocat ({barreau})")),
                active: true,
                surname_key: surname_key(&nom),
                category: "avocats",
                ape: None,
                barreau: Some(barreau_slug),
                alt_denominations: Vec::new(),
            })
        })?;
        loader.flush()?;
        Ok(loader.loaded)
    })?;
    conn.batch_execute("COMMIT").await?;
    Ok(loaded)
}

// ---------------------------------------------------------------------------
// Avocats aux Conseils — annuaire OACC (snapshot curé, ADR 0190)
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct OaccData {
    avocats: Vec<OaccAvocat>,
    societes: Vec<OaccSociete>,
}

#[derive(serde::Deserialize)]
struct OaccAvocat {
    nom: String,
    prenom: String,
    /// Nom d'affichage « Prénom NOM » — dénomination de résolution.
    full_name: String,
}

#[derive(serde::Deserialize)]
struct OaccSociete {
    /// Raison sociale officielle (SCP/SARL/Cabinet…, tous les associés).
    nom: String,
}

/// Charge le registre des avocats aux Conseils depuis le snapshot curé JSON
/// (même racine `ingest/corpus` que `load-legal-corpus`). Dataset minuscule
/// (~140 lignes) : deux COPY directs dans la transaction, pas de streaming.
///
/// Uid : `oacc:<nom>-<prenom>` (avocats, comme cnb sans segment barreau) et
/// `oacc:firm-<raison-pliée>` (sociétés). Dénomination « Prénom NOM » /
/// raison sociale, pliée `fold_stable` (la résolution comparera plié à plié).
async fn load_avocats_conseils(
    repo: &DecisionRepository<'_>,
    conn: &lj_store::db::Connection,
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

    for av in &data.avocats {
        if av.nom.is_empty() {
            continue;
        }
        let uid = format!("oacc:{}-{}", slug(&av.nom), slug(&av.prenom));
        if !seen.insert(uid.clone()) {
            continue;
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
        });
    }

    conn.batch_execute("BEGIN").await?;
    let cleared = repo.entity_namespace_clear("oacc").await?;
    tracing::info!(cleared, "namespace oacc vidé");
    repo.entity_copy(&ents).await?;
    conn.batch_execute("COMMIT").await?;
    tracing::info!(
        avocats = data.avocats.len(),
        societes = data.societes.len(),
        "oacc chargé"
    );
    Ok(ents.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec ADR 0239 : cabinets = APE 69.10Z sans token d'une autre profession
    // du droit ; le droit public prime ; hors 69.10Z → entreprises.
    #[test]
    fn sirene_category_cabinets_apres_exclusions() {
        assert_eq!(
            sirene_category("morale_privee", Some("69.10Z"), "scp bernal chevallier"),
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
}
