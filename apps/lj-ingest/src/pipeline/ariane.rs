//! ArianeWeb (ADR 0204) : analyses AJCE + existence des conclusions CRP →
//! bundles `commentaires[]` en `decision_sources` (`source = 'ariane-web'`,
//! une ligne par décision CE, pivot `source_uid =
//! ariane-web/<dossier>@<date>` — l'id interne `AW_DCE` de l'ADR ne tient
//! pas : absent des documents pré-1968, cf. addendum).
//!
//! Déroulé par **année** (l'année xsearch est celle de la date de lecture de
//! la décision parente — analyses et conclusions d'une même décision tombent
//! dans la même tranche) : énumération CRP puis AJCE, téléchargement HTML AJCE
//! (cache disque shardé), parse pur, groupage et rattachement en mémoire par
//! (n° dossier, date de lecture) — jamais l'ECLI brut (ADR 0095).
//!
//! Le réseau (pages xsearch, HTML des fiches) est crawlé par un pool borné de
//! [`CRAWL_WORKERS`] threads, [`ariane::THROTTLE`] par worker ; la fusion des
//! bundles reste séquentielle dans l'ordre d'énumération (checksums stables).
//!
//! Idempotence (#7) : `content_checksum` = xxh3-64 hex du bundle sérialisé,
//! skip si identique. Une année est marquée au manifeste quand elle est
//! entièrement traitée **sans erreur transitoire** (téléchargement échoué).
//! Les états durables côté source ne bloquent pas le marquage : orphelins
//! (parente hors stock — avant ~1990 le CE en base est clairsemé), fiches
//! vides et hits sans numéro (`vides`) — ils seraient identiques au re-run.
//!
//! Un orphelin dont le hit porte un n° `AW_DCE` n'est pas abandonné : le
//! document intégral ArianeWeb devient la décision elle-même (ADR 0219),
//! rang 0 — auto-cicatrisation du trou de versement CE → DILA.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use chrono::Datelike;

use lj_core::decision::Decision;
use lj_core::parsing::{
    analyse_body, build_ariane_source_fields, parse_ajce_html, parse_dce_html, AjceEntry,
};
use lj_sources::ariane::{self, ArianeFond, ArianeHit};
use lj_store::repository::{DecisionRepository, ExtractedFields};

use crate::config::Settings;

use super::batch::drain_batch;
use super::embed::build_embedder_opt;
use super::{content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode};

/// Première année sondée par défaut : le fond AJCE descend à 1976 (audit
/// ADR 0095) — une marge basse, chaque année vide coûte une requête.
const FIRST_YEAR: u16 = 1960;

/// Threads de crawl concurrents. Avec [`ariane::THROTTLE`] respecté par
/// worker, le débit reste plafonné à ~8 req/s vers un site public — courtois,
/// mais ~4× le crawl séquentiel (une année AJCE fraîche : ~10 min au lieu de ~35).
const CRAWL_WORKERS: usize = 4;

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
    /// N° `AW_DCE` de la décision parente (récupération ADR 0219) — premier
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

/// Sync ArianeWeb. `years` vide = toutes les années depuis [`FIRST_YEAR`] ;
/// `limit` = plafond de documents AJCE téléchargés (sonde) — quand il est
/// atteint le run s'arrête proprement sans marquer l'année au manifeste.
pub async fn sync_ariane(years: Vec<u16>, limit: Option<usize>) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.cache_dir().join("ariane");
    fs::create_dir_all(&dir)?;

    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let index: HashMap<(String, String), i64> = repo
        .ce_docket_date_index()
        .await?
        .into_iter()
        .map(|(dossier, date, id)| ((norm_dossier(&dossier), date), id))
        .collect();
    let existing = repo.ariane_checksums().await?;
    tracing::info!(
        ce_keys = index.len(),
        bundles_en_base = existing.len(),
        "sync_ariane démarré"
    );

    let current_year = chrono::Utc::now().year() as u16;
    let years = if years.is_empty() {
        (FIRST_YEAR..=current_year).collect()
    } else {
        years
    };

    let mut manifest = read_manifest(&dir)?;
    let mut totals = Stats::default();
    let mut budget = limit;
    // Orphelins dont le hit porte un n° AW_DCE : récupérés en fin de run
    // (décision créée depuis le document intégral, ADR 0219).
    let mut recoverable: Vec<((String, String), Bundle)> = Vec::new();

    for year in years {
        // Une année marquée est entièrement traitée ; les deux dernières
        // années restent vivantes (le fond s'y remplit encore).
        if manifest.contains_key(&year.to_string()) && year + 1 < current_year {
            continue;
        }
        if budget == Some(0) {
            break;
        }
        let (bundles, stats, exhausted) = {
            let dir = dir.clone();
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
                        "bundle ariane orphelin : décision parente absente du stock, pas de DCE"
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
        // pas : identiques au re-run, ils condamneraient l'année à être
        // re-balayée à chaque fill. Le raccrochage des orphelins passera par
        // une purge du manifeste quand un fond CE profond arrivera (le cache
        // HTML rend ce rejeu quasi gratuit).
        if !exhausted && stats.erreurs == 0 {
            manifest.insert(year.to_string(), stats.analyses + stats.conclusions);
            write_manifest(&dir, &manifest)?;
        }
        tracing::info!(
            year,
            analyses = stats.analyses,
            conclusions = stats.conclusions,
            orphans,
            vides = stats.vides,
            erreurs = stats.erreurs,
            "sync_ariane année"
        );
    }

    let (recuperees, rattachees) = if recoverable.is_empty() {
        (0, 0)
    } else {
        let (created, attached, still_orphans) =
            recover_dce_orphans(&settings, &dir, &conn, index, recoverable).await?;
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
        "sync_ariane terminé"
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
            tracing::warn!(id = %hit.id, "hit CRP sans clé (dossier, date)");
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
        tracing::warn!(id = %hit.id, "hit AJCE sans numéro");
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
            tracing::warn!(num, error = %e, "téléchargement AJCE échoué");
            return HitFetch::Erreur;
        }
    };
    let parsed = parse_ajce_html(&html);
    let body = analyse_body(&parsed);
    // Fiche « classement seul » (rubriques PCJA sans sommaire rédigé, fréquent
    // 1986-1995) : le classement vaut d'être servi, l'entrée part avec un corps
    // vide. Seule une fiche sans rien est un vrai vide.
    if body.is_empty() && parsed.rubriques.is_empty() {
        tracing::warn!(num, "analyse AJCE sans contenu, skippée");
        return HitFetch::Vide;
    }
    // Clé du hit, repli sur le n° de dossier de l'en-tête HTML (quelques
    // hits anciens n'ont ni `SourceCsv1` ni ECLI).
    let Some(key) =
        bundle_key(hit).or_else(|| Some((parsed.dossier.clone()?, hit.date_iso()?.to_string())))
    else {
        tracing::warn!(id = %hit.id, "hit AJCE sans clé (dossier, date)");
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
/// `AW_DCE` ne pivote pas : absent des documents pré-1968 et qualifié
/// d'instable/opaque par l'audit (ADR 0204, addendum).
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
        // ne pas masquer (pas de plafond silencieux).
        tracing::warn!(
            fond = fond.as_str(),
            year,
            total = total_count,
            vus = out.len(),
            "énumération incomplète ou dupliquée"
        );
    }
    Ok(out)
}

/// Récupération DCE (ADR 0219) : les bundles orphelins dont le hit xsearch
/// porte un n° `AW_DCE` deviennent des décisions (source `ariane-web`, rang 0 —
/// le texte cède à toute source officielle future, promotion ADR 0105), puis
/// leurs bundles sont rattachés dans le même run.
///
/// Avant de créer, l'ECLI du document DCE arbitre : si (numéro, date ECLI)
/// pointe sur une décision du stock, l'orphelin est un **lien raté** (date de
/// fiche erronée, zéros de tête) — on rattache, on ne crée pas. Mesure du
/// 12/07/2026 : 20 liens ratés sur 96 orphelins à DCE.
///
/// Idempotent : re-jouable, skip par checksum. Retourne (créées, rattachées,
/// restées orphelines).
async fn recover_dce_orphans(
    settings: &Settings,
    dir: &Path,
    conn: &lj_store::db::Connection,
    index: HashMap<(String, String), i64>,
    recoverable: Vec<((String, String), Bundle)>,
) -> Result<(usize, usize, usize)> {
    let repo = DecisionRepository::new(conn);
    let total = recoverable.len();
    tracing::info!(orphelins_avec_dce = total, "récupération DCE : démarrage");

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
                        tracing::warn!(num, error = %e, "téléchargement DCE échoué, skip");
                        continue;
                    }
                };
                let parsed = parse_dce_html(&html);
                if parsed.body.len() < 200 {
                    tracing::warn!(num, "document DCE quasi vide, skip");
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
                        "orphelin déjà en base (lien raté), rattachement sans création"
                    );
                    liens_rates += 1;
                    attaches.push((key, bundle, lookup_keys));
                    continue;
                }
                let dossiers2 = dossiers.clone();
                let ecli2 = parsed.ecli.clone().or_else(|| bundle.ecli.clone());
                let extracted = ExtractedFields {
                    date_lecture: chrono::NaiveDate::parse_from_str(&true_date, "%Y-%m-%d").ok(),
                    docket_numbers: dossiers.clone(),
                    ..Default::default()
                };
                let extracted = lj_ingest::extract::with_facet_uids(extracted, Some("CE"));
                let decision = Decision {
                    source_uid: format!("ariane-web/dce/{num}"),
                    member_name: num.to_string(),
                    ecli: parsed.ecli.clone().or_else(|| bundle.ecli.clone()),
                    jurisdiction_source_code: None,
                    chamber: None,
                    nac: None,
                    jurisdiction_name: Some("Conseil d'État".to_string()),
                    jurisdiction_type: Some("CE".to_string()),
                    jurisdiction_location: None,
                    numero_dossier: dossiers.first().cloned(),
                    numero_dossiers: Some(dossiers),
                    numero_role: None,
                    date_lecture: Some(true_date.clone()),
                    date_audience: None,
                    date_mise_jour: None,
                    formation: None,
                    type_decision: None,
                    type_recours: None,
                    solution: None,
                    publication_codes: Vec::new(),
                    avocat_requerant: None,
                    themes: Vec::new(),
                    attacked: None,
                    texte_integral_raw: parsed.body.clone(),
                    texte_integral_clean: parsed.body.clone(),
                    sections: Vec::new(),
                    metadata_header: String::new(),
                    visa_trim: String::new(),
                    parse_warnings: Vec::new(),
                };
                candidates.push(Candidate {
                    decision_id: None,
                    public_id: generate_public_id(),
                    decision,
                    content_checksum: content_checksum(html.as_bytes()),
                    raw_payload: Vec::new(),
                    payload_format: "html".to_string(),
                    write_mode: super::WriteMode::Full,
                    dila_fond: None,
                    // Forme payload-Judilibre minimale : le chemin linéaire
                    // (`Decision::from_source_fields_json`) reconstruit la
                    // décision canonique depuis ces clés — la struct `decision`
                    // ci-dessus ne sert qu'aux garde-fous amont.
                    prebuilt_source_fields: Some(serde_json::json!({
                        "jurisdiction": "ce",
                        "decision_date": true_date,
                        "numbers": dossiers2,
                        "ecli": ecli2,
                        "ariane_dce": num,
                        "niveau": parsed.niveau,
                    })),
                    prebuilt_extracted: Some(extracted),
                });
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
        "récupération DCE : parse ok"
    );

    // 2. Création (chunk + embed) via le drain standard.
    let (embedder, require_embeddings) = build_embedder_opt(settings).await?;
    let mut counts = IngestCounts::default();
    drain_batch(
        conn,
        embedder.as_ref(),
        candidates,
        require_embeddings,
        IngestMode::MissingHash,
        &mut counts,
    )
    .await?;

    // 3. Rattachement des bundles — décisions créées et liens ratés confondus,
    //    via les clés de résolution calculées au parse.
    let index: HashMap<(String, String), i64> = repo
        .ce_docket_date_index()
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
        "récupération DCE terminée"
    );
    Ok((counts.created, attached, still_orphans))
}

/// Manifeste des années complètes (`année → docs vus`), sous le cache ariane.
fn read_manifest(dir: &Path) -> Result<HashMap<String, usize>> {
    let path = dir.join("manifest.json");
    if !path.exists() {
        return Ok(HashMap::new());
    }
    Ok(serde_json::from_str(&fs::read_to_string(&path)?)?)
}

fn write_manifest(dir: &Path, manifest: &HashMap<String, usize>) -> Result<()> {
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(manifest)?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ecli_num_date, norm_dossier};

    // Clés de rattachement : cas réels des liens ratés du 12/07/2026
    // (99137 ArianeWeb vs 099137 JADE ; date de fiche 09/06 vs ECLI 23/06).
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
}
