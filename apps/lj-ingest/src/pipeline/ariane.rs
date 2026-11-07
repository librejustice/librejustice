//! ArianeWeb : les trois fonds en un job, piloté par le recensement. Une requête
//! par fond rend la facette `anneeDateDeLecture` comptée serveur, donc le fond
//! entier année par année. Seul ce que le recensement dément est visité : les
//! années où `AW_DCE` compte plus que le fonds (décisions absentes, créées sous
//! `ariane-dce`), celles dont le compte `AW_AJCE`+`AW_CRP` a bougé depuis le
//! dernier passage propre (commentaires). Les décisions passent avant les
//! commentaires : une décision créée dans le run reçoit ses analyses dans le même
//! run, au lieu de faire un orphelin de plus.
//!
//! Analyses AJCE + existence des conclusions CRP → bundles `commentaires[]` en
//! `decision_sources` (`source = 'ariane-web'`, une ligne par décision CE, pivot
//! `source_uid = ariane-web/<dossier>@<date>`).
//!
//! Déroulé par **année** — celle de la date de lecture de la parente, pour que
//! analyses et conclusions d'une même décision tombent dans la même tranche.
//! Rattachement par (n° dossier, date de lecture), jamais l'ECLI brut. La fusion
//! des bundles reste séquentielle : checksums stables.
//!
//! Idempotence : `content_checksum` = xxh3-64 du bundle sérialisé. Une année
//! n'est marquée au manifeste que si elle passe **sans erreur transitoire** ;
//! orphelins, fiches vides et hits sans numéro ne bloquent pas le marquage.
//!
//! Un orphelin dont le hit porte un n° `AW_DCE` devient la décision elle-même,
//! sous la source `ariane-dce` : c'est une charge de décision, le bundle de
//! commentaires n'en est pas une, et le barème d'autorité les sépare.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use lj_core::decision::Decision;
use lj_parse::{
    analyse_body, build_ariane_source_fields, parse_ajce_html, parse_dce_html, AjceEntry,
};
use lj_sources::ariane::{self, ArianeFond, ArianeHit};
use lj_store::repository::{DecisionRepository, ExtractedFields};

use crate::config::Settings;

use super::batch::drain_batch;
use super::{content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode};

/// Threads de crawl concurrents. Avec [`ariane::THROTTLE`] respecté par
/// worker, le débit reste plafonné vers un site public.
const CRAWL_WORKERS: usize = 4;

/// Les trois fonds recensés année par année, une requête chacun.
struct Census {
    ajce: BTreeMap<u16, u32>,
    crp: BTreeMap<u16, u32>,
    dce: BTreeMap<u16, u32>,
}

/// Comptes qu'ArianeWeb déclarait pour une année au dernier passage propre, un
/// champ par fond : le recensement du run suivant les dément ou les confirme, et
/// rien d'autre ne décide de rejouer l'année. `dce` reste absent tant qu'aucune
/// énumération de décisions n'a conclu.
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct YearMark {
    ajce: u32,
    crp: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dce: Option<u32>,
}

type Manifest = HashMap<u16, YearMark>;

impl Census {
    fn read(client: &reqwest::blocking::Client) -> Result<Self> {
        let of = |fond| {
            ariane::census_by_year(client, fond)
                .map_err(|e| anyhow!("recensement {}: {e}", ArianeFond::as_str(fond)))
        };
        Ok(Self {
            ajce: of(ArianeFond::Ajce)?,
            crp: of(ArianeFond::Crp)?,
            dce: of(ArianeFond::Dce)?,
        })
    }

    fn count(fond: &BTreeMap<u16, u32>, year: u16) -> u32 {
        fond.get(&year).copied().unwrap_or(0)
    }

    /// Années portant des commentaires dont le compte a bougé depuis le dernier
    /// passage propre. La borne basse vient du fond, jamais d'une constante :
    /// `AW_AJCE` descend à 1875.
    fn commented_years_moved(&self, manifest: &Manifest) -> Vec<u16> {
        let mut years: Vec<u16> = self.ajce.keys().chain(self.crp.keys()).copied().collect();
        years.sort_unstable();
        years.dedup();
        years.retain(|&y| {
            manifest.get(&y).map(|m| (m.ajce, m.crp))
                != Some((Self::count(&self.ajce, y), Self::count(&self.crp, y)))
        });
        years
    }

    fn mark_comments(&self, manifest: &mut Manifest, year: u16) {
        let m = manifest.entry(year).or_default();
        m.ajce = Self::count(&self.ajce, year);
        m.crp = Self::count(&self.crp, year);
    }

    fn mark_decisions(&self, manifest: &mut Manifest, year: u16) {
        manifest.entry(year).or_default().dce = Some(Self::count(&self.dce, year));
    }
}

/// Bundle en construction pour une décision (clé : dossier principal + date).
#[derive(Default)]
struct Bundle {
    /// N°s de dossier (multi-valués sur affaires jointes) — le premier compose
    /// le lien public, tous servent de clés de rattachement candidates.
    dossiers: Vec<String>,
    date: Option<String>,
    ecli: Option<String>,
    /// (num AJCE, entrée) — trié par num avant sérialisation pour un
    /// checksum stable quel que soit l'ordre d'énumération.
    analyses: Vec<(u32, AjceEntry)>,
    has_crp: bool,
    /// N° `AW_DCE` de la décision parente — premier
    /// hit qui en porte un fait foi.
    dce: Option<u32>,
}

#[derive(Default)]
struct Stats {
    analyses: usize,
    conclusions: usize,
    upserted: usize,
    unchanged: usize,
    orphans: usize,
    sans_cle: usize,
    /// Hits durablement inexploitables côté source (fiche vide, sans numéro) —
    /// ne bloquent pas le marquage de l'année au manifeste.
    vides: usize,
    /// Erreurs transitoires (téléchargement échoué) — bloquent le marquage.
    erreurs: usize,
}

/// Sync ArianeWeb : recensement des trois fonds, décisions absentes, puis
/// commentaires des années qui ont bougé. `years` force les années des deux
/// passes ; `census_only` s'arrête au recensement ; `limit` plafonne les
/// documents AJCE téléchargés (sonde), et le run s'arrête alors proprement sans
/// marquer l'année au manifeste.
pub async fn sync_ariane(years: Vec<u16>, census_only: bool, limit: Option<usize>) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.cache_dir().join("ariane");
    fs::create_dir_all(&dir)?;

    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;

    let census = tokio::task::spawn_blocking(|| {
        let client = ariane::http_client().map_err(|e| anyhow!("client ariane: {e}"))?;
        Census::read(&client)
    })
    .await
    .map_err(|e| anyhow!("tâche recensement: {e}"))??;

    let mut manifest = read_manifest(&dir)?;
    let deficits =
        deficits_de_decisions(&DecisionRepository::new(&conn), &census, &manifest).await?;
    let bougees = census.commented_years_moved(&manifest);
    tracing::info!(
        ajce = census.ajce.values().map(|&n| u64::from(n)).sum::<u64>(),
        crp = census.crp.values().map(|&n| u64::from(n)).sum::<u64>(),
        dce = census.dce.values().map(|&n| u64::from(n)).sum::<u64>(),
        annees_en_deficit = deficits.len(),
        annees_commentaires_a_rejouer = bougees.len(),
        "recensement ArianeWeb"
    );
    if census_only {
        return Ok(());
    }

    let cibles = |designees: Vec<u16>| {
        if years.is_empty() {
            designees
        } else {
            years.clone()
        }
    };
    fill_decisions(&dir, &conn, cibles(deficits), &census, &mut manifest).await?;
    sync_comments(&dir, &conn, cibles(bougees), limit, &census, manifest).await
}

/// Récolte les commentaires des années désignées et rattache leurs bundles. Une
/// année qui passe sans erreur transitoire est marquée au manifeste sur les
/// comptes du recensement, et sautée tant qu'ils ne bougent pas.
async fn sync_comments(
    dir: &Path,
    conn: &lj_pg::db::Connection,
    years: Vec<u16>,
    limit: Option<usize>,
    census: &Census,
    mut manifest: Manifest,
) -> Result<()> {
    let repo = DecisionRepository::new(conn);
    let index: HashMap<(String, String), i64> = repo
        .docket_date_index("CE")
        .await?
        .into_iter()
        .map(|(dossier, date, id)| ((norm_dossier(&dossier), date), id))
        .collect();
    let existing = repo.ariane_checksums().await?;
    tracing::info!(
        ce_keys = index.len(),
        bundles_en_base = existing.len(),
        annees = years.len(),
        "sync_ariane commentaires"
    );

    let mut totals = Stats::default();
    let mut budget = limit;
    // Orphelins dont le hit porte un n° AW_DCE : récupérés en fin de run
    // (décision créée depuis le document intégral).
    let mut recoverable: Vec<((String, String), Bundle)> = Vec::new();

    for year in years {
        if budget == Some(0) {
            break;
        }
        let (bundles, stats, exhausted) = {
            let dir = dir.to_path_buf();
            let year_budget = budget;
            // Le client `reqwest::blocking` vit entièrement côté bloquant :
            // sa création comme son drop paniquent en contexte async.
            tokio::task::spawn_blocking(move || {
                let client = ariane::http_client().map_err(|e| anyhow!("client ariane: {e}"))?;
                gather_year(&client, &dir, year, year_budget)
            })
            .await
            .map_err(|e| anyhow!("tâche gather ariane {year}: {e}"))??
        };
        if let Some(b) = budget.as_mut() {
            *b = b.saturating_sub(stats.analyses);
        }
        totals.analyses += stats.analyses;
        totals.conclusions += stats.conclusions;
        totals.vides += stats.vides;
        totals.erreurs += stats.erreurs;
        totals.sans_cle += stats.sans_cle;

        let mut orphans = 0usize;
        for ((dossier, date), mut bundle) in bundles {
            let Some(&decision_id) = bundle
                .dossiers
                .iter()
                .find_map(|d| index.get(&(norm_dossier(d), date.clone())))
            else {
                if bundle.dce.is_some() {
                    recoverable.push(((dossier, date), bundle));
                } else {
                    tracing::warn!(
                        dossier,
                        date,
                        year,
                        "orphan ariane bundle: parent decision missing from the stock, no DCE"
                    );
                    orphans += 1;
                }
                continue;
            };
            bundle.analyses.sort_by_key(|(num, _)| *num);
            let entries: Vec<AjceEntry> = bundle.analyses.into_iter().map(|(_, e)| e).collect();
            let sf = build_ariane_source_fields(
                &dossier,
                &date,
                bundle.ecli.as_deref(),
                &entries,
                bundle.has_crp,
            );
            let payload = serde_json::to_vec(&sf)?;
            let checksum = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(&payload));
            let source_uid = format!("ariane-web/{dossier}@{date}");
            if existing.get(&source_uid) == Some(&checksum) {
                totals.unchanged += 1;
                continue;
            }
            repo.upsert_decision_source(decision_id, &source_uid, &checksum, "json", &sf)
                .await?;
            totals.upserted += 1;
        }
        totals.orphans += orphans;

        // Année marquée quand elle est entièrement traitée sans erreur
        // transitoire. Les états durables (`orphans`, `vides`) ne bloquent
        // pas : identiques au re-run.
        if !exhausted && stats.erreurs == 0 {
            census.mark_comments(&mut manifest, year);
            write_manifest(dir, &manifest)?;
        }
        tracing::info!(
            year,
            analyses = stats.analyses,
            conclusions = stats.conclusions,
            orphans,
            vides = stats.vides,
            erreurs = stats.erreurs,
            "sync_ariane year"
        );
    }

    let (recuperees, rattachees) = if recoverable.is_empty() {
        (0, 0)
    } else {
        let (created, attached, still_orphans) =
            recover_dce_orphans(dir, conn, index, recoverable).await?;
        totals.orphans += still_orphans;
        (created, attached)
    };

    tracing::info!(
        analyses = totals.analyses,
        conclusions = totals.conclusions,
        upserted = totals.upserted,
        unchanged = totals.unchanged,
        recuperees,
        rattachees,
        orphans = totals.orphans,
        sans_cle = totals.sans_cle,
        vides = totals.vides,
        erreurs = totals.erreurs,
        "sync_ariane done"
    );
    Ok(())
}

/// Récolte d'une année : bundles indexés par (dossier principal, date),
/// statistiques, et drapeau « budget épuisé ».
type YearHarvest = (HashMap<(String, String), Bundle>, Stats, bool);

/// Collecte bloquante d'une année : hits CRP + AJCE, HTML AJCE (cache), parse.
/// Renvoie `(bundles par (dossier principal, date), stats, budget épuisé ?)`.
fn gather_year(
    client: &reqwest::blocking::Client,
    dir: &Path,
    year: u16,
    budget: Option<usize>,
) -> Result<YearHarvest> {
    let mut bundles: HashMap<(String, String), Bundle> = HashMap::new();
    let mut stats = Stats::default();

    for hit in enumerate(client, ArianeFond::Crp, year)? {
        let Some(key) = bundle_key(&hit) else {
            tracing::warn!(id = %hit.id, "CRP hit without a key (dossier, date)");
            stats.sans_cle += 1;
            continue;
        };
        let b = bundles.entry(key).or_default();
        b.has_crp = true;
        fill_keys(b, &hit);
        stats.conclusions += 1;
    }

    let ajce_hits = enumerate(client, ArianeFond::Ajce, year)?;
    let mut exhausted = false;
    if budget.is_some() {
        // Sonde : séquentiel, le plafond s'évalue hit par hit.
        for hit in ajce_hits {
            if budget.is_some_and(|b| stats.analyses >= b) {
                exhausted = true;
                break;
            }
            let fetch = fetch_hit(client, dir, &hit);
            merge_hit(&mut bundles, &mut stats, &hit, fetch);
        }
    } else {
        for (hit, fetch) in parallel_map(ajce_hits, CRAWL_WORKERS, |hit| {
            let fetch = fetch_hit(client, dir, &hit);
            (hit, fetch)
        }) {
            merge_hit(&mut bundles, &mut stats, &hit, fetch);
        }
    }
    Ok((bundles, stats, exhausted))
}

/// Résultat de l'étape réseau d'un hit AJCE — téléchargement caché + parse
/// pur, sans toucher aux bundles, pour être parallélisable.
enum HitFetch {
    /// Hit sans numéro ou fiche sans contenu — durable côté source (`vides`).
    Vide,
    /// Téléchargement échoué — transitoire (`erreurs`).
    Erreur,
    /// Fiche exploitable mais sans clé (dossier, date) — durable (`sans_cle`).
    SansCle,
    Analyse {
        num: u32,
        key: (String, String),
        /// N° de dossier de l'en-tête HTML — repli si le hit n'en porte pas.
        dossier_htm: Option<String>,
        entry: Box<AjceEntry>,
    },
}

fn fetch_hit(client: &reqwest::blocking::Client, dir: &Path, hit: &ArianeHit) -> HitFetch {
    let Ok(num) = hit.num() else {
        tracing::warn!(id = %hit.id, "AJCE hit without a number");
        return HitFetch::Vide;
    };
    let html = match ariane::ajce_html_cached(client, dir, num) {
        Ok((html, fetched)) => {
            if fetched {
                std::thread::sleep(ariane::THROTTLE);
            }
            html
        }
        Err(e) => {
            tracing::warn!(num, error = %e, "AJCE download failed");
            return HitFetch::Erreur;
        }
    };
    let parsed = parse_ajce_html(&html);
    let body = analyse_body(&parsed);
    // Fiche « classement seul » (rubriques PCJA sans sommaire rédigé) : le
    // classement vaut d'être servi, l'entrée part avec un corps vide. Seule une
    // fiche sans rien est un vrai vide.
    if body.is_empty() && parsed.rubriques.is_empty() {
        tracing::warn!(num, "AJCE analysis without content, skipped");
        return HitFetch::Vide;
    }
    // Clé du hit, repli sur le n° de dossier de l'en-tête HTML (quelques
    // hits anciens n'ont ni `SourceCsv1` ni ECLI).
    let Some(key) =
        bundle_key(hit).or_else(|| Some((parsed.dossier.clone()?, hit.date_iso()?.to_string())))
    else {
        tracing::warn!(id = %hit.id, "AJCE hit without a key (dossier, date)");
        return HitFetch::SansCle;
    };
    let entry = AjceEntry {
        body,
        codes_pcja: hit.pcja(),
        niveau: hit.niveau.clone().filter(|s| !s.is_empty()),
        rubriques: parsed
            .rubriques
            .iter()
            .map(|r| format!("{} : {}", r.code, r.label))
            .collect(),
        renvois: parsed.renvois,
        date: hit.date_iso().map(str::to_string),
    };
    HitFetch::Analyse {
        num,
        key,
        dossier_htm: parsed.dossier,
        entry: Box::new(entry),
    }
}

/// Fusion séquentielle d'un hit traité dans les bundles + compteurs.
fn merge_hit(
    bundles: &mut HashMap<(String, String), Bundle>,
    stats: &mut Stats,
    hit: &ArianeHit,
    fetch: HitFetch,
) {
    match fetch {
        HitFetch::Vide => stats.vides += 1,
        HitFetch::Erreur => stats.erreurs += 1,
        HitFetch::SansCle => stats.sans_cle += 1,
        HitFetch::Analyse {
            num,
            key,
            dossier_htm,
            entry,
        } => {
            let b = bundles.entry(key).or_default();
            if hit.crp_num().is_some() {
                b.has_crp = true;
            }
            fill_keys(b, hit);
            if b.dossiers.is_empty() {
                b.dossiers.extend(dossier_htm);
            }
            b.analyses.push((num, *entry));
            stats.analyses += 1;
        }
    }
}

/// Applique `work` à chaque élément via un pool borné de threads (débit réseau
/// plafonné par [`ariane::THROTTLE`] côté worker). Les résultats sortent dans
/// l'ordre des entrées.
fn parallel_map<T, R, F>(items: Vec<T>, workers: usize, work: F) -> Vec<R>
where
    T: Send,
    R: Send,
    F: Fn(T) -> R + Sync,
{
    let n = items.len();
    let queue: Mutex<VecDeque<(usize, T)>> = Mutex::new(items.into_iter().enumerate().collect());
    let slots: Mutex<Vec<Option<R>>> = Mutex::new((0..n).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..workers.min(n) {
            s.spawn(|| loop {
                let popped = queue.lock().expect("queue empoisonnée").pop_front();
                let Some((i, item)) = popped else { break };
                let r = work(item);
                slots.lock().expect("slots empoisonnés")[i] = Some(r);
            });
        }
    });
    slots
        .into_inner()
        .expect("slots empoisonnés")
        .into_iter()
        .map(|r| r.expect("chaque slot est rempli par un worker"))
        .collect()
}

/// Clé de groupage et de pivot d'un hit : (n° de dossier principal, date de
/// lecture ISO) — la clé naturelle de la décision côté source. L'id interne
/// `AW_DCE` ne pivote pas : absent des documents pré-1968.
fn bundle_key(hit: &ArianeHit) -> Option<(String, String)> {
    let dossier = hit.dossiers().first().cloned()?;
    let date = hit.date_iso()?.to_string();
    Some((dossier, date))
}

/// Numéro de dossier normalisé pour les clés de rattachement : les zéros de
/// tête varient selon la source (`99137` côté ArianeWeb, `099137` côté JADE).
fn norm_dossier(d: &str) -> String {
    let t = d.trim_start_matches('0');
    if t.is_empty() { "0" } else { t }.to_string()
}

/// (numéro, date ISO) portés par un ECLI CE
/// (`ECLI:FR:CESSR:1976:99137.19760303`). La date de l'ECLI est celle gravée
/// dans le document officiel — plus fiable que la date de la fiche AJCE.
fn ecli_num_date(ecli: &str) -> Option<(String, String)> {
    let tail = ecli.rsplit(':').next()?;
    let (num, d) = tail.split_once('.')?;
    if num.is_empty() || d.len() != 8 || d.bytes().any(|b| !b.is_ascii_digit()) {
        return None;
    }
    Some((
        num.to_string(),
        format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..8]),
    ))
}

/// Clés de rattachement du bundle depuis un hit (premier arrivé fait foi).
fn fill_keys(b: &mut Bundle, hit: &ArianeHit) {
    if b.dossiers.is_empty() {
        b.dossiers = hit.dossiers();
    }
    if b.date.is_none() {
        b.date = hit.date_iso().map(str::to_string);
    }
    if b.ecli.is_none() {
        b.ecli = hit.ecli_parent.clone().filter(|s| !s.is_empty());
    }
    if b.dce.is_none() {
        b.dce = hit.parent_dce_num();
    }
}

/// Énumère toutes les pages d'un fond pour une année (20 hits/page) : la
/// page 1 révèle `PageCount`, les suivantes partent sur le pool de workers.
fn enumerate(
    client: &reqwest::blocking::Client,
    fond: ArianeFond,
    year: u16,
) -> Result<Vec<ArianeHit>> {
    let first = ariane::search_page(client, fond, year, 1)
        .map_err(|e| anyhow!("xsearch {} {year} p1: {e}", fond.as_str()))?;
    let (page_count, total_count) = (first.page_count, first.total_count);
    let mut out = first.documents;
    if page_count > 1 {
        let pages: Vec<u32> = (2..=page_count).collect();
        for fetched in parallel_map(pages, CRAWL_WORKERS, |page| {
            let p = ariane::search_page(client, fond, year, page)
                .map_err(|e| anyhow!("xsearch {} {year} p{page}: {e}", fond.as_str()));
            std::thread::sleep(ariane::THROTTLE);
            p
        }) {
            out.extend(fetched?.documents);
        }
    }
    if out.len() != total_count as usize {
        // Pagination serveur instable (tri sans requête) : signaler,
        // ne pas masquer.
        tracing::warn!(
            fond = fond.as_str(),
            year,
            total = total_count,
            vus = out.len(),
            "enumeration incomplete or duplicated"
        );
    }
    Ok(out)
}

/// Années où `AW_DCE` compte plus de décisions que le fonds, du déficit le plus
/// large au plus étroit. Le compte est un **majorant** du manque : ArianeWeb
/// porte un document par dossier, le fonds une décision par affaire, et cinq
/// affaires jointes creusent un écart que rien ne manque. L'énumération tranche.
async fn deficits_de_decisions(
    repo: &DecisionRepository<'_>,
    census: &Census,
    manifest: &Manifest,
) -> Result<Vec<u16>> {
    let ours: HashMap<u16, i64> = repo
        .ce_counts_by_year()
        .await?
        .into_iter()
        .map(|(y, n)| (y as u16, n))
        .collect();
    let mut deficits: Vec<(u16, u32, i64)> = census
        .dce
        .iter()
        .map(|(&y, &aw)| (y, aw, ours.get(&y).copied().unwrap_or(0)))
        .filter(|(_, aw, notre)| i64::from(*aw) > *notre)
        .collect();
    deficits.sort_by_key(|(_, aw, notre)| *notre - i64::from(*aw));
    for (year, aw, notre) in &deficits {
        let tranche = manifest.get(year).and_then(|m| m.dce) == Some(*aw);
        tracing::info!(
            year,
            ariane = aw,
            fonds = notre,
            manque = i64::from(*aw) - notre,
            tranche,
            "shortfall"
        );
    }
    Ok(deficits
        .into_iter()
        .filter(|(y, aw, _)| manifest.get(y).and_then(|m| m.dce) != Some(*aw))
        .map(|(y, _, _)| y)
        .collect())
}

/// Énumère les années désignées, garde les hits que le fonds n'a pas, et crée
/// leurs décisions depuis le document intégral.
async fn fill_decisions(
    dir: &Path,
    conn: &lj_pg::db::Connection,
    years: Vec<u16>,
    census: &Census,
    manifest: &mut Manifest,
) -> Result<()> {
    if years.is_empty() {
        return Ok(());
    }
    let index: HashMap<(String, String), i64> = DecisionRepository::new(conn)
        .docket_date_index("CE")
        .await?
        .into_iter()
        .map(|(dossier, date, id)| ((norm_dossier(&dossier), date), id))
        .collect();

    let mut counts = IngestCounts::default();
    for year in years {
        let absents = {
            let index = index.clone();
            tokio::task::spawn_blocking(move || -> Result<Vec<ArianeHit>> {
                let client = ariane::http_client().map_err(|e| anyhow!("client ariane: {e}"))?;
                let hits = enumerate(&client, ArianeFond::Dce, year)?;
                Ok(hits
                    .into_iter()
                    .filter(|h| !hit_en_base(h, &index))
                    .collect())
            })
            .await
            .map_err(|e| anyhow!("tâche énumération DCE {year}: {e}"))??
        };
        tracing::info!(year, absents = absents.len(), "DCE enumeration");
        if absents.is_empty() {
            census.mark_decisions(manifest, year);
            write_manifest(dir, manifest)?;
            continue;
        }
        let attendus = absents.len();
        let candidates = {
            let dir = dir.to_path_buf();
            tokio::task::spawn_blocking(move || -> Result<Vec<Candidate>> {
                let client = ariane::http_client().map_err(|e| anyhow!("client ariane: {e}"))?;
                Ok(parallel_map(absents, CRAWL_WORKERS, |hit| {
                    dce_from_hit(&client, &dir, &hit)
                })
                .into_iter()
                .flatten()
                .collect())
            })
            .await
            .map_err(|e| anyhow!("tâche téléchargement DCE {year}: {e}"))??
        };
        tracing::info!(year, candidats = candidates.len(), "DCE documents read");
        let tous_lus = candidates.len() == attendus;
        drain_batch(conn, candidates, IngestMode::MissingHash, &mut counts).await?;
        if tous_lus {
            census.mark_decisions(manifest, year);
            write_manifest(dir, manifest)?;
        }
    }
    tracing::info!(created = counts.created, "ArianeWeb decisions created");
    Ok(())
}

/// Un hit DCE est en base si l'une de ses clés (dossier normalisé, date de
/// lecture) y est. Un hit sans date ni dossier n'est pas rattachable : le
/// traiter comme absent créerait un doublon à chaque run.
fn hit_en_base(hit: &ArianeHit, index: &HashMap<(String, String), i64>) -> bool {
    let Some(date) = hit.date_iso() else {
        return true;
    };
    let dossiers = hit.dossiers();
    if dossiers.is_empty() {
        return true;
    }
    dossiers
        .iter()
        .any(|d| index.contains_key(&(norm_dossier(d), date.to_string())))
}

/// Télécharge et lit le document DCE d'un hit absent du fonds.
fn dce_from_hit(
    client: &reqwest::blocking::Client,
    dir: &Path,
    hit: &ArianeHit,
) -> Option<Candidate> {
    let num = hit.num().ok()?;
    let date = hit.date_iso()?.to_string();
    let html = match ariane::dce_html_cached(client, dir, num) {
        Ok((html, fetched)) => {
            if fetched {
                std::thread::sleep(ariane::THROTTLE);
            }
            html
        }
        Err(e) => {
            tracing::warn!(num, error = %e, "DCE download failed, skip");
            return None;
        }
    };
    let parsed = parse_dce_html(&html);
    if parsed.body.len() < 200 {
        tracing::warn!(num, "DCE document nearly empty, skip");
        return None;
    }
    let dossiers = if parsed.dossiers.is_empty() {
        hit.dossiers()
    } else {
        parsed.dossiers.clone()
    };
    Some(dce_candidate(
        num,
        &html,
        parsed.body,
        dossiers,
        parsed.ecli.or_else(|| hit.ecli_parent.clone()),
        &date,
        parsed.niveau.or_else(|| hit.niveau.clone()),
    ))
}

/// Candidat de décision CE bâti sur un document `AW_DCE` : c'est la forme sous
/// laquelle une décision entre par ArianeWeb, que le bundle l'ait révélée ou que
/// le recensement l'ait trouvée manquante.
fn dce_candidate(
    num: u32,
    html: &str,
    body: String,
    dossiers: Vec<String>,
    ecli: Option<String>,
    date: &str,
    niveau: Option<String>,
) -> Candidate {
    let extracted = ExtractedFields {
        date_lecture: chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok(),
        docket_numbers: dossiers.clone(),
        ..Default::default()
    };
    let extracted = lj_extract_rows::with_facet_uids(extracted, Some("CE"));
    let decision = Decision {
        source_uid: format!("ariane-dce/{num}"),
        member_name: num.to_string(),
        ecli: ecli.clone(),
        jurisdiction_source_code: None,
        chamber: None,
        nac: None,
        jurisdiction_name: Some("Conseil d'État".to_string()),
        jurisdiction_type: Some("CE".to_string()),
        jurisdiction_location: None,
        numero_dossier: dossiers.first().cloned(),
        numero_dossiers: Some(dossiers.clone()),
        numero_role: None,
        date_lecture: Some(date.to_string()),
        date_audience: None,
        date_mise_jour: None,
        formation: None,
        type_decision: None,
        type_recours: None,
        solution: None,
        publication_codes: Vec::new(),
        avocat_requerant: None,
        listed_counsel: Vec::new(),
        themes: Vec::new(),
        attacked: None,
        texte_integral_raw: body.clone(),
        texte_integral_clean: body,
        sections: Vec::new(),
        metadata_header: String::new(),
        parse_warnings: Vec::new(),
    };
    Candidate {
        decision_id: None,
        public_id: generate_public_id(),
        decision,
        content_checksum: content_checksum(html.as_bytes()),
        raw_payload: Vec::new(),
        payload_format: "html".to_string(),
        write_mode: super::WriteMode::Full,
        passages: true,
        distinct: false,
        // Forme payload-Judilibre minimale : le chemin linéaire
        // (`Decision::from_source_fields_json`) reconstruit la décision
        // canonique depuis ces clés.
        prebuilt_source_fields: Some(serde_json::json!({
            "jurisdiction": "ce",
            "decision_date": date,
            "numbers": dossiers,
            "ecli": ecli,
            "ariane_dce": num,
            "niveau": niveau,
        })),
        prebuilt_extracted: Some(extracted),
    }
}

/// Récupération DCE : les bundles orphelins dont le hit xsearch porte un n°
/// `AW_DCE` deviennent des décisions (source `ariane-web`, rang 0 — le texte
/// cède à toute source officielle future), puis leurs bundles sont rattachés
/// dans le même run.
///
/// Avant de créer, l'ECLI du document DCE arbitre : si (numéro, date ECLI)
/// pointe sur une décision du stock, l'orphelin est un **lien raté** (date de
/// fiche erronée, zéros de tête) — on rattache, on ne crée pas.
///
/// Idempotent : re-jouable, skip par checksum. Retourne (créées, rattachées,
/// restées orphelines).
async fn recover_dce_orphans(
    dir: &Path,
    conn: &lj_pg::db::Connection,
    index: HashMap<(String, String), i64>,
    recoverable: Vec<((String, String), Bundle)>,
) -> Result<(usize, usize, usize)> {
    let repo = DecisionRepository::new(conn);
    let total = recoverable.len();
    tracing::info!(orphelins_avec_dce = total, "DCE fetch: start");

    // 1. Téléchargement + parse DCE → candidats à créer + clés de rattachement
    //    (bloquant, throttlé).
    let (candidates, attaches, liens_rates) = {
        let dir = dir.to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<_> {
            let client = ariane::http_client().map_err(|e| anyhow!("client ariane: {e}"))?;
            let mut candidates: Vec<Candidate> = Vec::new();
            // (clé fiche pour le source_uid, bundle, clés de résolution
            //  normalisées (dossier, date) à essayer post-drain).
            type Attach = ((String, String), Bundle, Vec<(String, String)>);
            let mut attaches: Vec<Attach> = Vec::new();
            let mut liens_rates = 0usize;
            for (key, bundle) in recoverable {
                let num = bundle.dce.expect("récolte filtrée sur dce.is_some()");
                let html = match ariane::dce_html_cached(&client, &dir, num) {
                    Ok((html, fetched)) => {
                        if fetched {
                            std::thread::sleep(ariane::THROTTLE);
                        }
                        html
                    }
                    Err(e) => {
                        tracing::warn!(num, error = %e, "DCE download failed, skip");
                        continue;
                    }
                };
                let parsed = parse_dce_html(&html);
                if parsed.body.len() < 200 {
                    tracing::warn!(num, "DCE document nearly empty, skip");
                    continue;
                }
                let dossiers = if parsed.dossiers.is_empty() {
                    bundle.dossiers.clone()
                } else {
                    parsed.dossiers.clone()
                };
                // Clés de résolution : chaque dossier connu (parse DCE, fiche,
                // numéro ECLI) × chaque date connue (fiche, ECLI).
                let ecli_nd = parsed
                    .ecli
                    .as_deref()
                    .or(bundle.ecli.as_deref())
                    .and_then(ecli_num_date);
                let true_date = ecli_nd
                    .as_ref()
                    .map(|(_, d)| d.clone())
                    .unwrap_or_else(|| key.1.clone());
                let mut lookup_keys: Vec<(String, String)> = Vec::new();
                for d in dossiers
                    .iter()
                    .chain(bundle.dossiers.iter())
                    .map(|d| norm_dossier(d))
                    .chain(ecli_nd.iter().map(|(n, _)| norm_dossier(n)))
                {
                    for date in [&key.1, &true_date] {
                        let k = (d.clone(), date.clone());
                        if !lookup_keys.contains(&k) {
                            lookup_keys.push(k);
                        }
                    }
                }
                if let Some(k) = lookup_keys.iter().find(|k| index.contains_key(*k)) {
                    tracing::info!(
                        num,
                        dossier = %k.0,
                        date = %k.1,
                        "orphan already in the database (link missed), attaching without creating"
                    );
                    liens_rates += 1;
                    attaches.push((key, bundle, lookup_keys));
                    continue;
                }
                candidates.push(dce_candidate(
                    num,
                    &html,
                    parsed.body.clone(),
                    dossiers,
                    parsed.ecli.clone().or_else(|| bundle.ecli.clone()),
                    &true_date,
                    parsed.niveau.clone(),
                ));
                attaches.push((key, bundle, lookup_keys));
            }
            Ok((candidates, attaches, liens_rates))
        })
        .await
        .map_err(|e| anyhow!("tâche download DCE: {e}"))??
    };
    tracing::info!(
        candidats = candidates.len(),
        liens_rates,
        "DCE fetch: parse ok"
    );

    // 2. Création (chunk + embed) via le drain standard.
    let mut counts = IngestCounts::default();
    drain_batch(conn, candidates, IngestMode::MissingHash, &mut counts).await?;

    // 3. Rattachement des bundles — décisions créées et liens ratés confondus,
    //    via les clés de résolution calculées au parse.
    let index: HashMap<(String, String), i64> = repo
        .docket_date_index("CE")
        .await?
        .into_iter()
        .map(|(dossier, date, id)| ((norm_dossier(&dossier), date), id))
        .collect();
    let existing = repo.ariane_checksums().await?;
    // Les skips du téléchargement/parse (DCE vide côté CE) restent orphelins.
    let (mut attached, mut still_orphans) = (0usize, total - attaches.len());
    for ((dossier, date), mut bundle, lookup_keys) in attaches {
        let Some(&decision_id) = lookup_keys.iter().find_map(|k| index.get(k)) else {
            still_orphans += 1;
            continue;
        };
        bundle.analyses.sort_by_key(|(num, _)| *num);
        let entries: Vec<AjceEntry> = bundle.analyses.into_iter().map(|(_, e)| e).collect();
        let sf = build_ariane_source_fields(
            &dossier,
            &date,
            bundle.ecli.as_deref(),
            &entries,
            bundle.has_crp,
        );
        let payload = serde_json::to_vec(&sf)?;
        let checksum = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(&payload));
        let source_uid = format!("ariane-web/{dossier}@{date}");
        if existing.get(&source_uid) == Some(&checksum) {
            continue;
        }
        repo.upsert_decision_source(decision_id, &source_uid, &checksum, "json", &sf)
            .await?;
        attached += 1;
    }
    tracing::info!(
        created = counts.created,
        attached,
        still_orphans,
        "DCE fetch done"
    );
    Ok((counts.created, attached, still_orphans))
}

/// Manifeste des années passées proprement (`année → comptes du recensement`),
/// sous le cache ariane. Une entrée illisible est tenue pour absente, donc son
/// année repasse une fois : c'est la porte de sortie du format antérieur, qui
/// stockait les documents traités, incomparables au recensement.
fn read_manifest(dir: &Path) -> Result<HashMap<u16, YearMark>> {
    let path = dir.join("manifest.json");
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let raw: HashMap<String, serde_json::Value> =
        serde_json::from_str(&fs::read_to_string(&path)?)?;
    Ok(raw
        .into_iter()
        .filter_map(|(y, v)| Some((y.parse().ok()?, serde_json::from_value::<YearMark>(v).ok()?)))
        .collect())
}

fn write_manifest(dir: &Path, manifest: &Manifest) -> Result<()> {
    let keyed: BTreeMap<String, &YearMark> =
        manifest.iter().map(|(y, m)| (y.to_string(), m)).collect();
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(&keyed)?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ecli_num_date, hit_en_base, norm_dossier, Census, Manifest};
    use lj_sources::ariane::ArianeHit;
    use std::collections::{BTreeMap, HashMap};

    fn census(ajce: &[(u16, u32)], crp: &[(u16, u32)], dce: &[(u16, u32)]) -> Census {
        let map = |v: &[(u16, u32)]| -> BTreeMap<u16, u32> { v.iter().copied().collect() };
        Census {
            ajce: map(ajce),
            crp: map(crp),
            dce: map(dce),
        }
    }

    /// Le compte du recensement est le seul juge : une année confirmée est
    /// sautée, une année démentie repart, une année inconnue passe une fois.
    #[test]
    fn seul_un_compte_qui_bouge_rejoue_une_annee() {
        let c = census(&[(1918, 100), (2026, 500)], &[(2026, 20)], &[]);
        let mut m = Manifest::new();
        c.mark_comments(&mut m, 1918);
        c.mark_comments(&mut m, 2026);
        assert!(c.commented_years_moved(&m).is_empty());

        let bouge = census(&[(1918, 100), (2026, 510)], &[(2026, 20)], &[]);
        assert_eq!(bouge.commented_years_moved(&m), vec![2026]);

        let neuve = census(&[(1918, 100), (1919, 3), (2026, 500)], &[(2026, 20)], &[]);
        assert_eq!(neuve.commented_years_moved(&m), vec![1919]);
    }

    /// The two passes mark the same year without erasing each other: an
    /// enumeration settles the year, and does not run again while the fonds stays
    /// put.
    #[test]
    fn une_enumeration_qui_ne_trouve_rien_clot_lannee() {
        let c = census(&[(1982, 1000)], &[], &[(1982, 2000)]);
        let mut m = Manifest::new();
        c.mark_decisions(&mut m, 1982);
        assert_eq!(m[&1982].dce, Some(2000));
        assert_eq!(m[&1982].ajce, 0);

        c.mark_comments(&mut m, 1982);
        assert_eq!(m[&1982].dce, Some(2000));
        assert_eq!(m[&1982].ajce, 1000);

        let ajoutee = census(&[(1982, 1000)], &[], &[(1982, 2001)]);
        assert_ne!(m[&1982].dce, Some(super::Census::count(&ajoutee.dce, 1982)));
    }

    // Join keys: ArianeWeb drops the leading zero JADE keeps.
    #[test]
    fn norm_dossier_zeros_de_tete() {
        assert_eq!(norm_dossier("099137"), "99137");
        assert_eq!(norm_dossier("99137"), "99137");
        assert_eq!(norm_dossier("04834"), "4834");
        assert_eq!(norm_dossier("0000"), "0");
    }

    #[test]
    fn ecli_num_date_ce() {
        assert_eq!(
            ecli_num_date("ECLI:FR:CESSR:1976:99137.19760303"),
            Some(("99137".to_string(), "1976-03-03".to_string()))
        );
        assert_eq!(
            ecli_num_date("ECLI:FR:CESJS:1978:04834.19780623"),
            Some(("04834".to_string(), "1978-06-23".to_string()))
        );
        assert_eq!(ecli_num_date("ECLI:FR:CE:2020:sansdate"), None);
    }

    fn hit(dossier: Option<&str>, date: Option<&str>) -> ArianeHit {
        ArianeHit {
            id: "/Ariane_Web/AW_DCE/|12345".to_string(),
            dossier: dossier.map(str::to_string),
            date_lecture: date.map(str::to_string),
            parent: None,
            siblings: None,
            crp: None,
            codes_pcja: None,
            niveau: None,
            ecli_parent: None,
        }
    }

    /// Un hit sans clé de rattachement passe pour présent : le créer poserait un
    /// doublon de plus à chaque recensement, faute de pouvoir le retrouver.
    #[test]
    fn un_hit_sans_cle_ne_se_cree_pas() {
        let index = HashMap::from([(("18297".to_string(), "1982-07-26".to_string()), 1i64)]);
        assert!(hit_en_base(
            &hit(Some("018297"), Some("1982-07-26 02:00:00")),
            &index
        ));
        assert!(!hit_en_base(
            &hit(Some("99999"), Some("1982-07-26 02:00:00")),
            &index
        ));
        assert!(hit_en_base(&hit(Some("99999"), None), &index));
        assert!(hit_en_base(&hit(None, Some("1982-07-26 02:00:00")), &index));
    }
}
