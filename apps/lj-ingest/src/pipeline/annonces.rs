//! Fond marchés du pot Annonces : moisson quotidienne des canaux,
//! réconciliation multi-canal, upsert.
//!
//! On enregistre le flux, on ne stampe aucun état : la fenêtre « ouverte » se
//! calcule à la lecture (`echeance > NOW()`). Un connecteur cassé ne coûte donc
//! que de la fraîcheur — ses annonces sortent de la fenêtre par leur échéance —
//! mais l'écart doit se voir : l'alarme `annonces_moteurs_degrades` sonne quand
//! un moteur rend zéro ligne ou moins de 90 % de ce qu'il déclare.
//!
//! Ré-énumération complète quotidienne, pas de delta incrémental à maintenir.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{anyhow, Result};
use futures::StreamExt;
use jiff::tz::TimeZone;
use jiff::Timestamp;

use lj_annonces::{
    canonicaliser, cpv_division_uid, marche_uid_de_resultat, AnnonceBrute, AnnonceCanonique,
    ContenuAnnonce, LotAttribue, ResultatMarche,
};
use lj_sources_annonces::{self as annonces, HoteVerdict, MoteurRun};
use lj_store::repository::{
    AnnonceAEnrichir, AnnonceChamps, AnnonceSourceWriteItem, AnnonceWriteItem, AnnonceWriteStats,
    ContenuPose, DecisionRepository, DetailPose, FileEnrichissement, ResultatPose, SondeLiens,
};
use lj_vocab::JOURNAL_FOND_MARCHE;

use crate::config::Settings;

/// Ratio collecté/annoncé sous lequel un moteur est déclaré dégradé : 90 %
/// laisse la marge du churn normal sans laisser passer une casse de parseur.
const RATIO_MIN: f64 = 0.90;

/// Tentatives consécutives sans réponse au-delà desquelles un hôte recensé
/// cesse de compter comme un échec : sept nuits, une panne d'éditeur qui couche
/// 35 portails le temps d'un run n'en déclassant alors aucun. L'hôte muet reste
/// tenté chaque nuit, et revient au compte le jour où il répond.
const NUITS_MUETTES: i32 = 7;

/// Les hôtes en échec d'un run, répartis entre panne du jour et entrée morte du
/// recensement, celle-ci lue dans `muets`.
fn echecs_et_muets(hotes: &[HoteVerdict], muets: &HashSet<String>) -> (u32, u32) {
    hotes
        .iter()
        .filter(|h| !h.joignable)
        .fold((0, 0), |(echecs, morts), h| match muets.contains(&h.host) {
            true => (echecs, morts + 1),
            false => (echecs + 1, morts),
        })
}

/// Share of the live fleet down past which a multi-host engine is degraded. The
/// collected/announced ratio is blind to it: the announced total only sums over
/// hosts that ANSWERED. 40 % sits above the census's dead hosts not yet aged into
/// muet, and under a fleet-wide outage.
const ECHECS_MAX: f64 = 0.40;

/// No engine may eat the harvest's whole budget: past this, the other sixteen
/// still have time to land and write.
const BUDGET_MOTEUR: std::time::Duration = std::time::Duration::from_secs(1_200);

/// A weekly catch-up that finds nothing is what says the daily path holds, so
/// only one that repaired something rises above `info`.
macro_rules! rapport_rattrapage {
    ($repare:expr, $($champ:tt)*) => {
        match $repare {
            0 => tracing::info!($($champ)*),
            _ => tracing::warn!($($champ)*),
        }
    };
}

/// Notices the harvest pulls for the rows it has just written. The ceiling is a
/// runaway guard well above a night's discoveries: a backlog past it is a catch-up, run by hand.
const FICHES_PAR_NUIT: i64 = 6_000;

/// Minutes each engine gets for its notices. One host that rations them sets the
/// pace of its own branch alone, and past this it hands back, its rows waiting
/// for the next night.
const BUDGET_FICHES_MIN: u64 = 30;

/// Markets the harvest searches by reference, the residue no listing shows.
/// Unchanged from the chain step it replaces.
const RESOLUTIONS_PAR_NUIT: i64 = 500;

/// Uncategorised markets the weekly net reads back, so an archive drains over a few weeks.
const CPV_PAR_SEMAINE: i64 = 60_000;
const METIER_PAR_SEMAINE: i64 = 200_000;

/// Bounds an enumeration: a hung engine becomes a total failure, which the
/// verdict below counts, instead of holding the other sixteen to the watchdog.
async fn borne(
    f: impl std::future::Future<Output = lj_sources::error::Result<MoteurRun>>,
) -> Result<MoteurRun> {
    borne_a(BUDGET_MOTEUR, f).await
}

async fn borne_a(
    budget: std::time::Duration,
    f: impl std::future::Future<Output = lj_sources::error::Result<MoteurRun>>,
) -> Result<MoteurRun> {
    match tokio::time::timeout(budget, f).await {
        Ok(r) => r.map_err(|e| anyhow!("{e}")),
        Err(_) => Err(anyhow!("budget de {} s dépassé", budget.as_secs())),
    }
}

/// The Centrale walks a date window, so its budget follows it instead of the
/// flat one: a manual catch-up asks for months.
fn budget_centrale(jours: i32) -> std::time::Duration {
    BUDGET_MOTEUR.max(std::time::Duration::from_secs(70 * jours.max(0) as u64))
}

fn degrade(run: &MoteurRun, echecs_hotes: u32, hotes_vivants: usize) -> bool {
    run.rows.is_empty()
        || run
            .annonce
            .is_some_and(|a| a > 0 && (run.rows.len() as f64) < a as f64 * RATIO_MIN)
        || (hotes_vivants > 0 && f64::from(echecs_hotes) > hotes_vivants as f64 * ECHECS_MAX)
}

/// Moisson quotidienne : tous les canaux, puis réconciliation et écriture.
pub async fn annonces(centrale_jours: i32, relancer_muets: bool) -> Result<()> {
    let debut = std::time::Instant::now();
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Upserts de masse par unnest : hors statement_timeout.
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    // Les moteurs multi-tenants énumèrent les hôtes recensés par
    // `annonces-discover` ; sans recensement, ils ne rendent rien (et le
    // sanity check les déclare dégradés, sans fermeture).
    let plafond_muet = (!relancer_muets).then_some(NUITS_MUETTES);
    let hotes_atexo = repo
        .hotes_profil_acheteur("atexo", plafond_muet)
        .await
        .map_err(|e| anyhow!("hotes atexo: {e}"))?;
    let hotes_safetender = repo
        .hotes_profil_acheteur("safetender", plafond_muet)
        .await
        .map_err(|e| anyhow!("hotes safetender: {e}"))?;
    let hotes_synapse = repo
        .hotes_profil_acheteur("synapse", plafond_muet)
        .await
        .map_err(|e| anyhow!("hotes synapse: {e}"))?;
    // Les plateformes avérées (tous moteurs), validateurs du dépôt de
    // dernier ressort du connecteur BOAMP.
    let plateformes: HashSet<String> = repo
        .hotes_plateformes_classees()
        .await
        .map_err(|e| anyhow!("hotes plateformes: {e}"))?
        .into_iter()
        .collect();
    tracing::info!(
        atexo = hotes_atexo.len(),
        safetender = hotes_safetender.len(),
        synapse = hotes_synapse.len(),
        plateformes = plateformes.len(),
        "annonces: profiles surveyed"
    );

    // Les canaux en parallèle — le mur horaire est Klekoon, en série à 1 req/s.
    // Un canal en erreur franche vaut un run vide de ce canal, pas l'échec de la
    // moisson.
    let (
        boamp,
        ted,
        atexo,
        safetender,
        aws,
        dematis,
        achatpublic,
        atline,
        synapse,
        xmarches,
        aji,
        klekoon,
        modula,
        centraledesmarches,
        aws_tenants,
        provincenord,
        nicematin,
    ) = tokio::join!(
        borne(annonces::boamp::enumerate(&plateformes)),
        borne(annonces::ted::enumerate()),
        borne(annonces::atexo::enumerate(&hotes_atexo)),
        borne(annonces::safetender::enumerate(&hotes_safetender)),
        borne(annonces::aws::enumerate()),
        borne(annonces::dematis::enumerate()),
        borne(annonces::achatpublic::enumerate()),
        borne(annonces::atline::enumerate()),
        borne(annonces::synapse::enumerate(&hotes_synapse)),
        borne(annonces::xmarches::enumerate()),
        borne(annonces::aji::enumerate()),
        borne(annonces::klekoon::enumerate()),
        borne(annonces::modula::enumerate()),
        borne_a(
            budget_centrale(centrale_jours),
            annonces::centraledesmarches::enumerate(centrale_jours),
        ),
        borne(annonces::aws_tenants::enumerate()),
        borne(annonces::provincenord::enumerate()),
        borne(annonces::nicematin::enumerate()),
    );

    // An engine whose enumeration returns `Err` yields no `MoteurRun`, so the
    // degradation test below never sees it.
    let mut echecs_totaux: Vec<&'static str> = Vec::new();
    let mut runs: Vec<(&'static str, MoteurRun)> = [
        ("boamp", boamp),
        ("ted", ted),
        ("atexo", atexo),
        ("safetender", safetender),
        ("aws", aws),
        ("dematis", dematis),
        ("achatpublic", achatpublic),
        ("atline", atline),
        ("synapse", synapse),
        ("xmarches", xmarches),
        ("aji", aji),
        ("klekoon", klekoon),
        ("modula", modula),
        ("centraledesmarches", centraledesmarches),
        ("aws-tenants", aws_tenants),
        ("provincenord", provincenord),
        ("nicematin", nicematin),
    ]
    .into_iter()
    .filter_map(|(moteur, res)| match res {
        Ok(run) => Some((moteur, run)),
        Err(e) => {
            tracing::error!(moteur, error = %e, "annonces: moteur fully failed");
            echecs_totaux.push(moteur);
            None
        }
    })
    .collect();

    // La presse n'a pas de porte HTTP : sa session navigateur laisse un dump,
    // que la moisson consomme s'il est là. Pas de dump = pas de canal, sans
    // erreur — le portail n'est pas une dépendance du run.
    match dernier_dump(&settings.paths().presse()) {
        Some(f) => match annonces::presse::enumerate(&f).await {
            Ok(run) => {
                tracing::info!(dump = %f.display(), "annonces: press dump");
                runs.push(("presse", run));
            }
            Err(e) => tracing::error!(error = %e, "annonces: press dump unreadable"),
        },
        None => tracing::info!("annonces: no press dump"),
    }

    let muets: HashSet<String> = repo
        .hotes_muets(NUITS_MUETTES)
        .await
        .map_err(|e| anyhow!("hotes muets: {e}"))?
        .into_iter()
        .collect();

    let mut degrades: Vec<&'static str> = echecs_totaux;
    let mut brutes: Vec<AnnonceBrute> = Vec::new();
    let mut verdicts: Vec<(String, bool)> = Vec::new();
    for (moteur, run) in runs {
        let (echecs_hotes, hotes_muets) = echecs_et_muets(&run.hotes, &muets);
        verdicts.extend(run.hotes.iter().map(|h| (h.host.clone(), h.joignable)));
        let sain = !degrade(&run, echecs_hotes, run.hotes.len() - hotes_muets as usize);
        tracing::info!(
            moteur,
            collecte = run.rows.len(),
            annonce = run.annonce,
            echecs = run.echecs + echecs_hotes,
            hotes_muets,
            sain,
            "annonces: moteur"
        );
        if !sain {
            degrades.push(moteur);
        }
        brutes.extend(run.rows);
    }
    repo.enregistrer_verdicts_hotes(&verdicts)
        .await
        .map_err(|e| anyhow!("verdicts hôtes: {e}"))?;

    let aujourdhui = Timestamp::now()
        .to_zoned(TimeZone::get("Europe/Paris").expect("tzdb embarquée"))
        .date();
    let (canoniques, stats, absorbees) = reconcilier_et_ecrire(&repo, brutes, aujourdhui).await?;
    let relies = repo
        .relier_annonces_par_denomination(None)
        .await
        .map_err(|e| anyhow!("rattachement par dénomination: {e}"))?;

    // The lifecycle of what was just written, then the notices. Order is what
    // makes this cheap: twinning merges a platform row into the BOAMP one that
    // already carries the four fields, and about half the open markets with a
    // platform channel have such a sibling. Pulling first pays for those twice.
    for (phase, res) in [
        ("régularisation", regulariser(&repo, false).await),
        ("jumelage", jumeler(&repo, true).await),
        (
            "résolution",
            resoudre(&repo, RESOLUTIONS_PAR_NUIT, true).await,
        ),
    ] {
        if let Err(e) = res {
            tracing::error!(phase, error = %e, "annonces: phase failed");
        }
    }

    // Here and not in a queue read tomorrow: a queue ordered by closing date
    // serves what shuts soonest, so a market with a distant one waits behind
    // every new arrival.
    if let Err(e) = tirer_les_fiches(
        &repo,
        FICHES_PAR_NUIT,
        BUDGET_FICHES_MIN,
        FileEnrichissement::NonVisitees,
    )
    .await
    {
        tracing::error!(error = %e, "annonces: record pull failed");
    }

    // Fond `attribution` : la fenêtre active des avis de résultat. Un échec ne
    // coûte que de la fraîcheur d'enrichissement, pas la moisson.
    match annonces::boamp::resultats().await {
        Ok(resultats) => match ecrire_resultats(&repo, resultats).await {
            Ok((n, s, rattaches)) => tracing::info!(
                resultats = n,
                annonces = s.annonces,
                canaux = s.canaux,
                aretes = s.aretes,
                rattaches,
                "annonces: award results"
            ),
            Err(e) => tracing::error!(error = %e, "annonces: writing results failed"),
        },
        Err(e) => tracing::error!(error = %e, "annonces: award results failed"),
    }

    match annonces::ted::resultats(RESULTATS_TED_JOURS).await {
        Ok(resultats) => match ecrire_resultats_ted(&repo, resultats).await {
            Ok((vus, poses)) => {
                tracing::info!(vus, poses, "annonces: TED awards")
            }
            Err(e) => tracing::error!(error = %e, "annonces: writing TED awards"),
        },
        Err(e) => tracing::error!(error = %e, "annonces: TED awards failed"),
    }

    match annonces::aws::attributions().await {
        Ok(attributions) => match ecrire_resultats_aws(&repo, attributions).await {
            Ok((vus, rattachees, poses)) => {
                tracing::info!(vus, rattachees, poses, "annonces: AWS awards")
            }
            Err(e) => tracing::error!(error = %e, "annonces: writing AWS awards"),
        },
        Err(e) => tracing::error!(error = %e, "annonces: AWS awards failed"),
    }

    // The fourth award channel, last because it attaches to markets the phases
    // above have just closed and merged. It opens its own pool, after a download
    // no connection should sit through.
    if let Err(e) = annonces_decp(None, true).await {
        tracing::error!(error = %e, "annonces: DECP failed");
    }

    // Tripwire de volumétrie.
    for t in repo
        .tailles_tables_annonces()
        .await
        .map_err(|e| anyhow!("tailles: {e}"))?
    {
        tracing::info!(table = %t.table, bytes = t.bytes, "annonces: table size");
    }

    tracing::info!(
        canoniques,
        annonces = stats.annonces,
        canaux = stats.canaux,
        aretes = stats.aretes,
        absorbees,
        relies,
        elapsed_s = debut.elapsed().as_secs(),
        "annonces"
    );
    if !degrades.is_empty() {
        // Clé d'alerte Grafana : `annonces_moteurs_degrades`. Non fatal — le
        // reste de la moisson est écrit, mais l'écart doit se voir.
        tracing::error!(
            annonces_moteurs_degrades = degrades.len(),
            moteurs = degrades.join(","),
            "annonces: moteurs degraded, partial coverage"
        );
    }
    if canoniques == 0 {
        anyhow::bail!(
            "annonces: no canonical row, {} moteurs degraded",
            degrades.len()
        );
    }
    Ok(())
}

/// Le dump de presse le plus récent, quand une session en a laissé un. Le nom
/// porte la date de la session (`2026-08-15.ndjson`) : l'ordre lexicographique
/// suffit à désigner le dernier.
fn dernier_dump(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut fichiers: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "ndjson"))
        .collect();
    fichiers.sort();
    fichiers.pop()
}

/// Le dataset BOAMP ne porte pas d'avis paru avant.
const PLANCHER_BOAMP: jiff::civil::Date = jiff::civil::date(2010, 1, 1);

/// Backfill de l'historique BOAMP. Mois par mois, du plus ancien au plus
/// récent : les avis de marché de la fenêtre, puis ses résultats. L'ordre est la
/// décision — un résultat cherche sa consultation en stock, et les mois
/// antérieurs sont déjà écrits quand il arrive. Reprenable : tout est
/// idempotent, relancer sur une fenêtre déjà passée ne réécrit rien.
pub async fn annonces_backfill(
    depuis: Option<jiff::civil::Date>,
    jusqu: jiff::civil::Date,
) -> Result<()> {
    let debut = std::time::Instant::now();
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let plateformes: HashSet<String> = repo
        .hotes_plateformes_classees()
        .await
        .map_err(|e| anyhow!("hotes plateformes: {e}"))?
        .into_iter()
        .collect();

    let repris = depuis.unwrap_or(PLANCHER_BOAMP);
    tracing::info!(depuis = %repris, jusqu = %jusqu, "backfill: start");

    let un_mois = jiff::Span::new().months(1);
    let mut lo = repris.first_of_month();
    let (mut avis_total, mut resultats_total, mut orphelins_total, mut rattaches_total) =
        (0u64, 0u64, 0u64, 0u64);
    while lo < jusqu {
        let hi = lo
            .checked_add(un_mois)
            .map_err(|e| anyhow!("mois suivant de {lo}: {e}"))?;
        let mois = std::time::Instant::now();

        let brutes = annonces::boamp::avis_historiques(lo, hi, &plateformes)
            .await
            .map_err(|e| anyhow!("avis historiques {lo}: {e}"))?;
        let vus = brutes.len();
        let (_, mois_stats, _) = reconcilier_et_ecrire(&repo, brutes, hi)
            .await
            .map_err(|e| anyhow!("avis {lo}: {e}"))?;
        let ecrits = mois_stats.annonces;

        let resultats = annonces::boamp::resultats_historiques(lo, hi)
            .await
            .map_err(|e| anyhow!("résultats historiques {lo}: {e}"))?;
        let (n, stats, rattaches) = ecrire_resultats(&repo, resultats).await?;

        avis_total += vus as u64;
        resultats_total += n as u64;
        orphelins_total += stats.annonces;
        rattaches_total += rattaches;
        tracing::info!(
            mois = %lo.strftime("%Y-%m").to_string(),
            avis = vus,
            avis_ecrits = ecrits,
            resultats = n,
            orphelins = stats.annonces,
            rattaches,
            elapsed_s = mois.elapsed().as_secs(),
            "backfill: month"
        );
        lo = hi;
    }

    tracing::info!(
        avis = avis_total,
        resultats = resultats_total,
        orphelins = orphelins_total,
        rattaches = rattaches_total,
        elapsed_s = debut.elapsed().as_secs(),
        "backfill: end"
    );
    Ok(())
}

/// Refills the four fields on the stock already written, by re-reading the BOAMP
/// export window by window. It carries no identity: a row is reached by its
/// channel, so a notice whose market is not in stock writes nothing.
pub async fn annonces_contenu(
    depuis: Option<jiff::civil::Date>,
    jusqu: jiff::civil::Date,
) -> Result<()> {
    let debut = std::time::Instant::now();
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let plateformes = HashSet::new();

    let un_mois = jiff::Span::new().months(1);
    let mut lo = depuis.unwrap_or(PLANCHER_BOAMP).first_of_month();
    let (mut avis_total, mut poses_total, mut ecrits_total) = (0u64, 0u64, 0u64);
    while lo < jusqu {
        let hi = lo
            .checked_add(un_mois)
            .map_err(|e| anyhow!("mois suivant de {lo}: {e}"))?;
        let mois = std::time::Instant::now();
        let brutes = annonces::boamp::avis_historiques(lo, hi, &plateformes)
            .await
            .map_err(|e| anyhow!("avis historiques {lo}: {e}"))?;
        let poses: Vec<ContenuPose> = brutes
            .iter()
            .filter(|b| !b.contenu.est_vide() || !b.descripteurs.is_empty())
            .map(|b| {
                let mut contenu = serde_json::json!({});
                poser_contenu(&mut contenu, &b.contenu, &b.descripteurs);
                ContenuPose {
                    source_uid: b.source_uid.clone(),
                    description: b.contenu.description.clone(),
                    contenu,
                }
            })
            .collect();
        let mut ecrits = 0u64;
        for lot in poses.chunks(2_000) {
            ecrits += repo
                .poser_contenu_annonces(lot)
                .await
                .map_err(|e| anyhow!("poser le contenu {lo}: {e}"))?;
        }
        avis_total += brutes.len() as u64;
        poses_total += poses.len() as u64;
        ecrits_total += ecrits;
        tracing::info!(
            mois = %lo.strftime("%Y-%m").to_string(),
            avis = brutes.len(),
            poses = poses.len(),
            ecrits,
            elapsed_s = mois.elapsed().as_secs(),
            "annonces_contenu: month"
        );
        lo = hi;
    }

    tracing::info!(
        avis = avis_total,
        poses = poses_total,
        ecrits = ecrits_total,
        elapsed_s = debut.elapsed().as_secs(),
        "annonces_contenu"
    );
    Ok(())
}

fn date_chrono(d: jiff::civil::Date) -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(d.year() as i32, d.month() as u32, d.day() as u32)
        .expect("date civile valide par construction")
}

/// Réconcilie un batch contre le stock et l'écrit : charge les affectations
/// existantes des canaux du batch et de leurs clés de jointure, canonicalise,
/// upserte par lots (10 arrays de paramètres par statement), puis applique les
/// fusions découvertes — l'élue vient d'être écrite, l'absorbée se vide de ses
/// canaux de batch, il reste le hors-batch, la redirection et la suppression.
/// Rend (canoniques, stats, absorbées).
async fn reconcilier_et_ecrire(
    repo: &DecisionRepository<'_>,
    brutes: Vec<AnnonceBrute>,
    aujourdhui: jiff::civil::Date,
) -> Result<(usize, AnnonceWriteStats, u64)> {
    let cles: Vec<String> = brutes
        .iter()
        .flat_map(|b| std::iter::once(b.source_uid.clone()).chain(b.cles_jointure.iter().cloned()))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let existants: HashMap<String, String> = repo
        .annonces_par_source(&cles)
        .await
        .map_err(|e| anyhow!("affectations existantes: {e}"))?
        .into_iter()
        .collect();
    let reconciliation = canonicaliser(brutes, &existants, aujourdhui);
    let items: Vec<AnnonceWriteItem> = reconciliation.annonces.iter().map(ecrire).collect();
    let mut stats = AnnonceWriteStats::default();
    for lot in items.chunks(2_000) {
        let s = repo
            .upsert_annonces(lot)
            .await
            .map_err(|e| anyhow!("upsert_annonces: {e}"))?;
        stats.annonces += s.annonces;
        stats.canaux += s.canaux;
        stats.aretes += s.aretes;
    }
    let fusions: Vec<(String, String)> = reconciliation
        .fusions
        .into_iter()
        .map(|f| (f.ancien, f.nouveau))
        .collect();
    let absorbees = repo
        .fusionner_annonces(&fusions)
        .await
        .map_err(|e| anyhow!("fusions: {e}"))?;
    let ecrits: Vec<String> = items.iter().map(|i| i.uid.clone()).collect();
    replier_les_metiers(repo, Some(&ecrits)).await?;
    Ok((items.len(), stats, absorbees))
}

/// Régularisation du stock : chaque ligne orpheline (uid abandonné par un
/// re-groupage, plus aucun canal) est résolue par ses propres `details.liens` —
/// clés de jointure extraites des URLs et URLs d'avis elles-mêmes — vers la
/// ligne qui porte aujourd'hui ses canaux, puis absorbée (redirection +
/// suppression). Une orpheline sans trace ou à cibles multiples n'est pas
/// devinée : elle reste, comptée. Avec `formes`, réapplique aussi les règles de
/// forme à tout le stock — le filet hebdomadaire.
pub async fn annonces_regulariser(formes: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    regulariser(&repo, formes).await
}

async fn regulariser(repo: &DecisionRepository<'_>, formes: bool) -> Result<()> {
    let debut = std::time::Instant::now();
    let orphelines = repo
        .annonces_orphelines(JOURNAL_FOND_MARCHE)
        .await
        .map_err(|e| anyhow!("orphelines: {e}"))?;
    let mut cles: Vec<String> = Vec::new();
    let mut urls: Vec<String> = Vec::new();
    for o in &orphelines {
        cles.extend(lj_annonces::cles_jointure_depuis_urls(
            o.urls.iter().map(String::as_str),
        ));
        urls.extend(o.urls.iter().cloned());
    }
    cles.sort();
    cles.dedup();
    urls.sort();
    urls.dedup();
    let par_source: HashMap<String, String> = repo
        .annonces_par_source(&cles)
        .await
        .map_err(|e| anyhow!("par source: {e}"))?
        .into_iter()
        .collect();
    let par_url: HashMap<String, String> = repo
        .annonces_par_url_avis(&urls)
        .await
        .map_err(|e| anyhow!("par url: {e}"))?
        .into_iter()
        .collect();
    let candidats: Vec<String> = {
        let mut v: Vec<String> = par_source
            .values()
            .chain(par_url.values())
            .cloned()
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let champs: HashMap<String, AnnonceChamps> = repo
        .annonces_champs(&candidats)
        .await
        .map_err(|e| anyhow!("champs des cibles: {e}"))?
        .into_iter()
        .map(|c| (c.uid.clone(), c))
        .collect();

    let total = orphelines.len();
    let mut fusions: Vec<(String, String)> = Vec::new();
    let mut compte = CompteRegularisation::default();
    for o in orphelines {
        let mut cibles: Vec<&String> =
            lj_annonces::cles_jointure_depuis_urls(o.urls.iter().map(String::as_str))
                .iter()
                .filter_map(|k| par_source.get(k))
                .chain(o.urls.iter().filter_map(|u| par_url.get(u)))
                .collect();
        cibles.sort();
        cibles.dedup();
        let cibles: Vec<&AnnonceChamps> =
            cibles.iter().filter_map(|uid| champs.get(*uid)).collect();
        match cible_de_regularisation(&o, &cibles) {
            Resolution::Unique(uid) => {
                compte.uniques += 1;
                fusions.push((o.uid, uid.to_string()));
            }
            Resolution::Jumelle(uid) => {
                compte.jumelles += 1;
                fusions.push((o.uid, uid.to_string()));
            }
            Resolution::Indiscernables(uid) => {
                compte.indiscernables += 1;
                fusions.push((o.uid, uid.to_string()));
            }
            Resolution::Aucune if cibles.is_empty() => compte.sans_trace += 1,
            Resolution::Aucune => compte.ambigues += 1,
        }
    }
    let absorbees = repo
        .fusionner_annonces(&fusions)
        .await
        .map_err(|e| anyhow!("fusions: {e}"))?;

    // Filet hebdomadaire de la forme : l'ingest écrit propre à la
    // construction, mais une règle nouvelle (`embellir_titre`,
    // `nettoyer_acheteur`) doit aussi atteindre le stock clos, que la moisson
    // ne revisite jamais.
    let (mut formes_corrigees, mut resultats_corriges) = (0u64, 0u64);
    let (mut zones_posees, mut attributions_refondues) = (0u64, 0u64);
    let (mut descriptions_effacees, mut cpv_deduits) = (0u64, 0u64);
    let mut descriptions_reparees = 0u64;
    let mut familles_deduites = 0u64;
    let mut metiers_poses = 0u64;
    let mut metiers_deduits = 0u64;
    let mut demonstrations_purgees = 0u64;
    let mut echeances_effacees = 0u64;
    if formes {
        let formes = repo
            .formes_annonces()
            .await
            .map_err(|e| anyhow!("formes: {e}"))?;
        let demonstrations: Vec<&str> = formes
            .iter()
            .filter(|(_, titre, acteur)| {
                lj_annonces::consultation_de_test(acteur, &[titre.as_str()])
            })
            .map(|(uid, _, _)| uid.as_str())
            .collect();
        demonstrations_purgees = repo
            .purger_annonces(&demonstrations)
            .await
            .map_err(|e| anyhow!("purger démonstrations: {e}"))?;
        let corrections: Vec<(String, String, String)> = formes
            .into_iter()
            .filter(|(_, titre, acteur)| {
                !lj_annonces::consultation_de_test(acteur, &[titre.as_str()])
            })
            .filter_map(|(uid, titre, acteur)| {
                let titre_propre = lj_annonces::embellir_titre(&annonces::desechapper(&titre));
                let acteur_propre = lj_annonces::nettoyer_acheteur(&annonces::desechapper(&acteur));
                (titre_propre != titre || acteur_propre != acteur).then_some((
                    uid,
                    titre_propre,
                    acteur_propre,
                ))
            })
            .collect();
        for lot in corrections.chunks(2_000) {
            formes_corrigees += repo
                .corriger_formes(lot)
                .await
                .map_err(|e| anyhow!("corriger formes: {e}"))?;
        }

        // The stock predates the set-valued column: a row no harvest has
        // zoned yet is seeded from the single code it kept and from the région
        // its buyer names, rather than waiting a full re-enumeration to exist
        // on the axis.
        let zones: Vec<(String, Vec<String>)> = repo
            .annonces_a_zoner()
            .await
            .map_err(|e| anyhow!("zones: {e}"))?
            .into_iter()
            .map(|(uid, code, acteur)| (uid, zones_depuis_stock(code.as_deref(), &acteur)))
            .filter(|(_, z)| !z.is_empty())
            .collect();
        for lot in zones.chunks(2_000) {
            zones_posees += repo
                .poser_zones(lot)
                .await
                .map_err(|e| anyhow!("poser zones: {e}"))?;
        }

        descriptions_effacees = repo
            .effacer_descriptions_copiees()
            .await
            .map_err(|e| anyhow!("descriptions copiées: {e}"))?;
        echeances_effacees = repo
            .effacer_echeances_implausibles(lj_annonces::ECHEANCE_ANS_MAX)
            .await
            .map_err(|e| anyhow!("implausible deadlines: {e}"))?;

        let elisions: Vec<(String, String)> = repo
            .descriptions_a_reparer()
            .await
            .map_err(|e| anyhow!("descriptions à réparer: {e}"))?
            .into_iter()
            .filter_map(|(uid, texte)| {
                let propre = lj_annonces::reparer_elisions(&annonces::desechapper(&texte));
                (propre != texte).then_some((uid, propre))
            })
            .collect();
        for lot in elisions.chunks(2_000) {
            descriptions_reparees += repo
                .corriger_descriptions(lot)
                .await
                .map_err(|e| anyhow!("corriger descriptions: {e}"))?;
        }

        metiers_poses = replier_les_metiers(repo, None).await?;

        let deduits: Vec<(String, Vec<String>)> = repo
            .annonces_sans_metier(METIER_PAR_SEMAINE)
            .await
            .map_err(|e| anyhow!("annonces sans métier: {e}"))?
            .into_iter()
            .filter_map(|(uid, titre)| {
                let m = lj_annonces::metiers::metiers_du_titre(&titre);
                (!m.is_empty()).then_some((uid, m))
            })
            .collect();
        for lot in deduits.chunks(2_000) {
            metiers_deduits += repo
                .poser_metiers(lot)
                .await
                .map_err(|e| anyhow!("poser métiers déduits: {e}"))?;
        }

        let sans_cpv = repo
            .annonces_sans_cpv(CPV_PAR_SEMAINE)
            .await
            .map_err(|e| anyhow!("annonces sans cpv: {e}"))?;
        let deductions: Vec<(String, String)> = sans_cpv
            .into_iter()
            .filter_map(|(uid, titre)| {
                lj_annonces::cpv::division_probable(&titre).map(|d| (uid, d))
            })
            .collect();
        for lot in deductions.chunks(2_000) {
            cpv_deduits += repo
                .poser_cpv_deduit(lot)
                .await
                .map_err(|e| anyhow!("poser cpv déduit: {e}"))?;
        }

        let table: Vec<(&str, &str)> = lj_annonces::cpv::natures_par_division().collect();
        familles_deduites = repo
            .deduire_famille_depuis_cpv(&table)
            .await
            .map_err(|e| anyhow!("déduire famille: {e}"))?;

        attributions_refondues = repo
            .refondre_attributions()
            .await
            .map_err(|e| anyhow!("refondre attributions: {e}"))?;

        // Même filet pour les lots d'attribution des blocs `details.resultat`.
        let resultats: Vec<(String, serde_json::Value)> = repo
            .resultats_annonces()
            .await
            .map_err(|e| anyhow!("résultats: {e}"))?
            .into_iter()
            .filter_map(|(uid, mut resultat)| {
                embellir_resultat(&mut resultat).then_some((uid, resultat))
            })
            .collect();
        for lot in resultats.chunks(2_000) {
            resultats_corriges += repo
                .corriger_resultats(lot)
                .await
                .map_err(|e| anyhow!("corriger résultats: {e}"))?;
        }
    }

    rapport_rattrapage!(
        absorbees
            + formes_corrigees
            + resultats_corriges
            + zones_posees
            + attributions_refondues
            + descriptions_effacees
            + descriptions_reparees
            + cpv_deduits
            + familles_deduites
            + metiers_poses
            + metiers_deduits
            + demonstrations_purgees
            + echeances_effacees,
        orphelines = total,
        absorbees,
        uniques = compte.uniques,
        jumelles = compte.jumelles,
        indiscernables = compte.indiscernables,
        sans_trace = compte.sans_trace,
        ambigues = compte.ambigues,
        formes_corrigees,
        resultats_corriges,
        zones_posees,
        attributions_refondues,
        descriptions_effacees,
        descriptions_reparees,
        cpv_deduits,
        familles_deduites,
        metiers_poses,
        metiers_deduits,
        demonstrations_purgees,
        echeances_effacees,
        elapsed_s = debut.elapsed().as_secs(),
        "annonces_regulariser"
    );
    Ok(())
}

#[derive(Default)]
struct CompteRegularisation {
    uniques: usize,
    jumelles: usize,
    indiscernables: usize,
    sans_trace: usize,
    ambigues: usize,
}

/// L'issue de la régularisation d'une orpheline.
enum Resolution<'a> {
    /// Une seule ligne en stock est désignée par les liens de l'orpheline.
    Unique(&'a str),
    /// Parmi plusieurs cibles, une seule porte le même titre et le même
    /// acheteur (normalisés) — l'échéance départage s'il en reste plusieurs.
    Jumelle(&'a str),
    /// Plusieurs jumelles aux champs strictement identiques (lots jumeaux,
    /// republication multi-plateformes) : la ligne orpheline est une chimère
    /// d'époque dont le contenu vaut pour chacune — la plus petite par uid,
    /// déterministe, ne perd rien.
    Indiscernables(&'a str),
    /// On ne devine pas.
    Aucune,
}

/// Les lignes de `cibles` qui portent le même titre et le même acheteur que
/// `titre`/`acteur`, au pliage canonique du fond près
/// (`lj_fold::fold` : casse, accents, apostrophe typographique,
/// blancs). Un acheteur vide d'un côté n'est pas une contradiction : une
/// observation manquante ne réfute rien. Deux graphies divergentes valent la
/// même quand les deux lignes portent la même entité acheteur, résolue sans
/// ambiguïté : « Ville de Maubeuge » et « COMMUNE DE MAUBEUGE » sont
/// `siren:215903923`.
fn jumelles<'a>(
    titre: &str,
    acteur: &str,
    acheteur_uid: Option<&str>,
    cibles: &[&'a AnnonceChamps],
) -> Vec<&'a AnnonceChamps> {
    let (titre, acteur) = (lj_fold::fold(titre), lj_fold::fold(acteur));
    cibles
        .iter()
        .copied()
        .filter(|c| {
            lj_fold::fold(&c.titre) == titre && acheteur_compatible(&acteur, acheteur_uid, c)
        })
        .collect()
}

/// Le garde acheteur du jumelage, isolé pour valoir aussi sur le repli
/// approchant : deux graphies égales, une observation manquante d'un côté, ou
/// la même entité résolue des deux côtés. `acteur` arrive plié.
fn acheteur_compatible(acteur: &str, acheteur_uid: Option<&str>, c: &AnnonceChamps) -> bool {
    let ca = lj_fold::fold(&c.acteur_libelle);
    ca == acteur
        || ca.is_empty()
        || acteur.is_empty()
        || (acheteur_uid.is_some() && c.acheteur_uid.as_deref() == acheteur_uid)
}

/// Choisit la cible d'une orpheline parmi les lignes que ses propres liens
/// désignent (URL d'avis ou clé de jointure).
fn cible_de_regularisation<'a>(
    orpheline: &lj_store::repository::AnnonceOrpheline,
    cibles: &[&'a AnnonceChamps],
) -> Resolution<'a> {
    if let [seule] = cibles {
        return Resolution::Unique(&seule.uid);
    }
    let jumelles = jumelles(&orpheline.titre, &orpheline.acteur_libelle, None, cibles);
    match jumelles.as_slice() {
        [] => Resolution::Aucune,
        [seule] => Resolution::Jumelle(&seule.uid),
        _ => {
            let strictes: Vec<&&AnnonceChamps> = jumelles
                .iter()
                .filter(|c| c.echeance == orpheline.echeance)
                .collect();
            match strictes.as_slice() {
                [] => {
                    // Aucune à l'échéance de l'orpheline (qui date d'une autre
                    // génération) : si les jumelles sont indiscernables entre
                    // elles, le choix est libre — sinon on ne devine pas.
                    let cle = |c: &AnnonceChamps| (lj_fold::fold(&c.acteur_libelle), c.echeance);
                    let premiere = cle(jumelles[0]);
                    if jumelles.iter().all(|c| cle(c) == premiere) {
                        Resolution::Indiscernables(
                            jumelles
                                .iter()
                                .map(|c| c.uid.as_str())
                                .min()
                                .expect("au moins deux jumelles"),
                        )
                    } else {
                        Resolution::Aucune
                    }
                }
                [seule] => Resolution::Jumelle(&seule.uid),
                plusieurs => Resolution::Indiscernables(
                    plusieurs
                        .iter()
                        .map(|c| c.uid.as_str())
                        .min()
                        .expect("au moins deux jumelles strictes"),
                ),
            }
        }
    }
}

/// Les moteurs multi-tenants : leur `source_uid` porte l'instance
/// (`atexo:<hôte>:<id>`), et deux instances n'ont aucun marché en commun. Le
/// canal cible d'un dépôt y est donc l'instance, pas le moteur.
const MULTI_TENANTS: [&str; 3] = ["atexo", "safetender", "modula"];

/// Le canal que désigne un dépôt : le moteur pour les plateformes nationales,
/// l'instance pour les multi-tenants — soit exactement le préfixe des
/// `source_uid` que son connecteur produit.
/// L'hôte de dépôt et le moteur qui le sert. Quand le recensement ne connaît
/// pas l'hôte exact, on retombe sur son domaine parent : une marque blanche
/// (`alm.marches-securises.fr`, `agysoft.marches-publics.info`) est servie par
/// le moteur de l'instance mère, et ses consultations vivent dans le même
/// vivier. Un parent inconnu du recensement ne rend rien — on n'invente pas de
/// moteur.
fn hote_et_moteur(depot: &str, moteurs: &HashMap<String, String>) -> Option<(String, String)> {
    let hote = hote_de(depot)?;
    if let Some(m) = moteurs.get(&hote) {
        return Some((hote_de_service(&hote, m, moteurs), m.clone()));
    }
    // The census merges through `canon_host` before probing, so the table holds
    // the canonical host and a declared typo finds nothing.
    let canon = lj_sources_annonces::discover::canon_host(&hote);
    if let Some(m) = moteurs.get(&canon) {
        return Some((hote_de_service(&canon, m, moteurs), m.clone()));
    }
    let parent = hote.split_once('.')?.1.to_string();
    let m = moteurs.get(&parent)?.clone();
    Some((hote_de_service(&parent, &m, moteurs), m))
}

/// The host a connector can question. The notice declares the front door
/// (`megalis.bretagne.bzh`), the instance serves under `marches.…`, and
/// `/api/v2/consultations` on the front door answers HTML. One censused
/// sub-domain of the same engine settles it, several leave it be.
fn hote_de_service(hote: &str, moteur: &str, moteurs: &HashMap<String, String>) -> String {
    let suffixe = format!(".{hote}");
    let mut sous = moteurs
        .iter()
        .filter(|(h, m)| m.as_str() == moteur && h.ends_with(&suffixe));
    match (sous.next(), sous.next()) {
        (Some((seul, _)), None) => seul.clone(),
        _ => hote.to_string(),
    }
}

fn canal_de_depot(moteur: &str, hote: &str) -> String {
    if MULTI_TENANTS.contains(&moteur) {
        format!("{moteur}:{hote}")
    } else {
        moteur.to_string()
    }
}

/// Le canal du stock que désigne un canal déclaré : lui-même quand il a des
/// lignes, sinon l'unique canal du même moteur dont l'hôte est un sous-domaine
/// du sien. L'avis déclare l'hôte de façade (`alsacemarchespublics.eu`,
/// `megalis.bretagne.bzh`), le connecteur range le stock sous l'hôte de service
/// (`plateforme.…`, `marches.…`).
fn canal_du_stock<T>(canal: String, vivier: &HashMap<String, T>) -> String {
    if vivier.contains_key(&canal) {
        return canal;
    }
    let Some((moteur, hote)) = canal.split_once(':') else {
        return canal;
    };
    let suffixe = format!(".{hote}");
    let mut sous = vivier.keys().filter(|k| {
        k.strip_prefix(moteur)
            .and_then(|r| r.strip_prefix(':'))
            .is_some_and(|h| h.ends_with(&suffixe))
    });
    match (sous.next(), sous.next()) {
        (Some(seul), None) => seul.clone(),
        _ => canal,
    }
}

/// Le canal d'un `source_uid` de plateforme, dans le même vocabulaire.
fn canal_de_source(source_uid: &str) -> String {
    let mut parts = source_uid.split(':');
    let moteur = parts.next().unwrap_or_default();
    if MULTI_TENANTS.contains(&moteur) {
        format!("{moteur}:{}", parts.next().unwrap_or_default())
    } else {
        moteur.to_string()
    }
}

/// L'hôte canonique d'une URL (`https://www.X.fr/entreprise` → `x.fr`).
fn hote_de(url: &str) -> Option<String> {
    let hote =
        annonces::discover::canon_host(url.split_once("://")?.1.split(['/', '?', '#']).next()?);
    (!hote.is_empty()).then_some(hote)
}

/// Pourquoi une candidate n'a pas donné de paire.
#[derive(Default)]
struct CompteJumelage {
    avec_cle: usize,
    hote_inconnu: usize,
    sans_jumelle: usize,
    ambigues: usize,
    disputees: usize,
    jumelles_disputees: usize,
    proches: usize,
    acheteur: usize,
    sans_corroboration: usize,
}

/// Title floor of a corroboration, and buyer floor below it: two labels of the
/// same buyer diverge more than two wordings of the same object.
const TITRE_CORROBORE: f64 = 0.55;
const ACHETEUR_CORROBORE: f64 = 0.7;

/// The floor every pair crosses, whatever key proposed it: a close title, the
/// same deadline to the day, or a buyer that concurs. A bare reference can be a
/// serial number two unrelated buyers share.
fn corrobore(ligne: &lj_store::repository::AnnonceAJumeler, jumelle: &AnnonceChamps) -> bool {
    use lj_annonces::similarite_titres;
    let meme_echeance = ligne.echeance.is_some()
        && ligne.echeance.and_then(date_paris) == jumelle.echeance.and_then(date_paris);
    let meme_acheteur = ligne.acheteur_uid.is_some() && ligne.acheteur_uid == jumelle.acheteur_uid;
    similarite_titres(&ligne.titre, &jumelle.titre) >= TITRE_CORROBORE
        || meme_echeance
        || meme_acheteur
        || similarite_titres(&ligne.acteur_libelle, &jumelle.acteur_libelle) >= ACHETEUR_CORROBORE
}

/// The same floor on the consultation a search brings back, which is not in
/// stock yet and has no buyer entity.
fn corrobore_brute(ligne: &lj_store::repository::AnnonceAJumeler, brute: &AnnonceBrute) -> bool {
    use lj_annonces::similarite_titres;
    let meme_echeance = ligne
        .echeance
        .and_then(date_paris)
        .zip(brute.echeance.map(jour_paris))
        .is_some_and(|(a, b)| a == b);
    similarite_titres(&ligne.titre, &brute.objet) >= TITRE_CORROBORE
        || meme_echeance
        || brute
            .acheteur
            .as_deref()
            .is_some_and(|a| similarite_titres(&ligne.acteur_libelle, a) >= ACHETEUR_CORROBORE)
}

/// L'issue du jumelage d'une candidate.
enum Jumelage<'a> {
    /// Une seule prétendante — d'emblée, ou après départage par l'échéance.
    Une(&'a str),
    /// Aucune ligne du canal cible ne porte ces titre et acheteur.
    Aucune,
    /// Plusieurs prétendantes que l'échéance ne sépare pas.
    Ambigue,
}

/// La jumelle plateforme d'une ligne que seul le BOAMP publie : même titre et
/// même acheteur pliés qu'elle — ou la même entité acheteur quand les deux
/// canaux l'écrivent autrement —, l'échéance en départage quand plusieurs
/// répondent. Le prédicat de la régularisation, sans son repli
/// « indiscernables » : un canal BOAMP appartient à UN marché, et entre deux
/// lignes que rien ne sépare, il n'y a pas de choix libre.
fn jumelle_de_depot<'a>(
    ligne: &lj_store::repository::AnnonceAJumeler,
    vivier: &[&'a AnnonceChamps],
) -> Jumelage<'a> {
    let jumelles = jumelles(
        &ligne.titre,
        &ligne.acteur_libelle,
        ligne.acheteur_uid.as_deref(),
        vivier,
    );
    match jumelles.as_slice() {
        [seule] => Jumelage::Une(&seule.uid),
        [] => Jumelage::Aucune,
        plusieurs => match plusieurs
            .iter()
            .filter(|c| c.echeance == ligne.echeance)
            .collect::<Vec<_>>()
            .as_slice()
        {
            [seule] => Jumelage::Une(&seule.uid),
            _ => Jumelage::Ambigue,
        },
    }
}

/// La jumelle par la seule référence de marché, quand elle désigne **une seule**
/// ligne plateforme de tout le stock ouvert et qu'aucune autre candidate ne la
/// porte. C'est la clé quand l'acheteur ne peut pas servir : sur une instance
/// multi-locataire la plateforme nomme le groupe (« Ministère de la Transition
/// Écologique ») là où l'avis nomme l'acheteur (« Office français de la
/// biodiversité »), et le prédicat titre + acheteur ne peut jamais jouer.
///
/// L'index est global, pas par canal : la référence désigne le marché, pas
/// l'hôte, et l'avis se trompe parfois de plateforme.
fn jumelle_par_reference<'a>(
    ligne: &lj_store::repository::AnnonceAJumeler,
    index: &HashMap<String, Vec<&'a AnnonceChamps>>,
    portees: &HashMap<String, usize>,
    canal_de_cible: &HashMap<&str, String>,
    canal_declare: Option<&str>,
) -> Jumelage<'a> {
    let Some(reference) = ligne.reference.as_deref().map(lj_fold::fold) else {
        return Jumelage::Aucune;
    };
    // Une référence que deux candidates portent ne départage rien, et une
    // « référence » égale au nom de l'acheteur est un champ mal extrait.
    if reference.is_empty()
        || reference == lj_fold::fold(&ligne.acteur_libelle)
        || portees.get(&reference).copied().unwrap_or(0) > 1
    {
        return Jumelage::Aucune;
    }
    let Some(cibles) = index.get(&reference) else {
        return Jumelage::Aucune;
    };
    // Plusieurs lignes portent la référence : c'est le même marché publié sur
    // deux plateformes, et l'avis en nomme une. Le canal déclaré départage —
    // exactement, sans rien approcher.
    let retenues: Vec<&&AnnonceChamps> = match cibles.as_slice() {
        [_] => cibles.iter().collect(),
        _ => {
            let Some(canal) = canal_declare else {
                return Jumelage::Ambigue;
            };
            cibles
                .iter()
                .filter(|c| {
                    canal_de_cible
                        .get(c.uid.as_str())
                        .is_some_and(|k| k == canal)
                })
                .collect()
        }
    };
    let [seule] = retenues.as_slice() else {
        return Jumelage::Ambigue;
    };
    match (ligne.acheteur_uid.as_deref(), seule.acheteur_uid.as_deref()) {
        // Deux entités acheteur résolues et différentes refusent la paire.
        (Some(a), Some(b)) if a != b => Jumelage::Aucune,
        (Some(_), Some(_)) => Jumelage::Une(&seule.uid),
        // A reference of digits alone is a serial number carrying nothing of its
        // buyer: several buyers share one far more often than one carrying a letter.
        _ if reference.chars().all(|c| c.is_ascii_digit())
            && ligne
                .echeance
                .and_then(date_paris)
                .zip(seule.echeance.and_then(date_paris))
                .is_none_or(|(a, b)| a != b) =>
        {
            Jumelage::Aucune
        }
        _ => Jumelage::Une(&seule.uid),
    }
}

/// A missing observation on one side refutes nothing; two known and diverging
/// values reject the pair.
fn compatible(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}

/// Two channels of one market name overlapping sets, never identical ones: one
/// publishes the buyer's département, the other every place of performance. A
/// silent side contradicts nothing.
fn zones_compatibles(a: &[String], b: &[String]) -> bool {
    a.is_empty() || b.is_empty() || a.iter().any(|z| b.contains(z))
}

/// What a row written before the set-valued column can still be zoned from
/// without a network call: the single code the old schema kept, and the région
/// its buyer names.
fn zones_depuis_stock(code: Option<&str>, acteur: &str) -> Vec<String> {
    let mut zones = lj_annonces::zones::zones_departements(code);
    if zones.is_empty() {
        if let Some(region) = lj_annonces::zones::region_citee(acteur) {
            zones = lj_annonces::zones::zones_departements(
                lj_annonces::zones::departements_de_region(region),
            );
        }
    }
    lj_annonces::zones::codes(&zones)
}

/// La date d'une échéance en heure de Paris : deux canaux du même marché
/// publient la même date limite avec des heures divergentes (12 h ici, 16 h là).
fn date_paris(e: chrono::DateTime<chrono::Utc>) -> Option<jiff::civil::Date> {
    Some(jour_paris(Timestamp::from_second(e.timestamp()).ok()?))
}

fn jour_paris(t: Timestamp) -> jiff::civil::Date {
    static PARIS: std::sync::LazyLock<TimeZone> =
        std::sync::LazyLock::new(|| TimeZone::get("Europe/Paris").expect("tzdb embarquée"));
    t.to_zoned(PARIS.clone()).date()
}

/// La jumelle par titre et date d'échéance, sur tout le stock ouvert : même
/// titre plié, même date limite, et une seule ligne qui satisfait les deux. Le
/// canal déclaré ne borne rien — l'avis se trompe parfois de plateforme, et
/// l'hôte déclaré n'a parfois aucun moteur connecté. Là où le titre seul
/// collisionne (« entretien des espaces verts »), le département et l'entité
/// acheteur récusent quand ils sont connus des deux côtés, et l'ambiguïté
/// refuse le reste.
fn jumelle_par_titre_echeance<'a>(
    ligne: &lj_store::repository::AnnonceAJumeler,
    index: &HashMap<String, Vec<&'a AnnonceChamps>>,
) -> Jumelage<'a> {
    let Some(echeance) = ligne.echeance.and_then(date_paris) else {
        return Jumelage::Aucune;
    };
    let titre = lj_fold::fold(&ligne.titre);
    let Some(cibles) = (!titre.is_empty()).then(|| index.get(&titre)).flatten() else {
        return Jumelage::Aucune;
    };
    let retenues: Vec<&&AnnonceChamps> = cibles
        .iter()
        .filter(|c| c.echeance.and_then(date_paris) == Some(echeance))
        .filter(|c| zones_compatibles(&ligne.zones, &c.zones))
        .filter(|c| compatible(ligne.acheteur_uid.as_deref(), c.acheteur_uid.as_deref()))
        .collect();
    match retenues.as_slice() {
        [seule] => Jumelage::Une(&seule.uid),
        [] => Jumelage::Aucune,
        _ => Jumelage::Ambigue,
    }
}

/// In the 0.75 to 0.85 band nearly every pair designates the same market, the divergence
/// being a typography, an article, or the reference the buyer prefixes its own title with.
const TITRE_PROCHE: f64 = 0.75;

/// The same buyer on both sides, the title then only having to approach. That
/// corroboration is worth more than the declared canal, which a notice sometimes
/// writes wrong and sometimes not at all. The folded label answers for the rows
/// whose buyer entity no registry resolves, the equivalence
/// [`acheteur_compatible`] already grants. Below `TITRE_PROCHE` two sibling lots
/// of one buyer merge: "véhicules de PTAC inférieur" against "ptac supérieur",
/// 0.56.
fn jumelle_par_acheteur<'a>(
    ligne: &lj_store::repository::AnnonceAJumeler,
    par_entite: &HashMap<&str, Vec<&'a AnnonceChamps>>,
    par_libelle: &HashMap<String, Vec<&'a AnnonceChamps>>,
) -> Jumelage<'a> {
    let cibles = match ligne.acheteur_uid.as_deref() {
        Some(e) => par_entite.get(e),
        None => match lj_fold::fold(&ligne.acteur_libelle) {
            l if l.is_empty() => None,
            l => par_libelle.get(&l),
        },
    };
    let Some(cibles) = cibles else {
        return Jumelage::Aucune;
    };
    let proches: Vec<&'a AnnonceChamps> = cibles
        .iter()
        .copied()
        .filter(|c| lj_annonces::similarite_titres(&ligne.titre, &c.titre) >= TITRE_PROCHE)
        .collect();
    match proches.as_slice() {
        [seule] => Jumelage::Une(&seule.uid),
        [] => Jumelage::Aucune,
        _ => Jumelage::Ambigue,
    }
}

/// The last key, played once neither the reference, nor the exact title, nor
/// the deposit has given a pair. What takes the place of the exactness given up
/// is the vivier: it bounds to the canal, hence to the instance on a
/// multi-tenant engine, where two tenants share no market.
fn jumelle_par_titre_proche<'a>(
    ligne: &lj_store::repository::AnnonceAJumeler,
    vivier: &[&'a AnnonceChamps],
) -> Jumelage<'a> {
    if lj_fold::fold(&ligne.titre).is_empty() {
        return Jumelage::Aucune;
    }
    let proches: Vec<&'a AnnonceChamps> = vivier
        .iter()
        .copied()
        .filter(|c| zones_compatibles(&ligne.zones, &c.zones))
        .filter(|c| compatible(ligne.acheteur_uid.as_deref(), c.acheteur_uid.as_deref()))
        .filter(|c| lj_annonces::similarite_titres(&ligne.titre, &c.titre) >= TITRE_PROCHE)
        .collect();
    match proches.as_slice() {
        [seule] => Jumelage::Une(&seule.uid),
        [] => Jumelage::Aucune,
        _ => Jumelage::Ambigue,
    }
}

/// Résout par la recherche de la plateforme les lignes ouvertes qui n'ont que
/// la racine d'un hôte.
///
/// An instance's listing and its search index do not cover the same set: an open
/// consultation may be in neither, its only door the search by the reference its notice publishes.
///
/// La ligne trouvée entre par le chemin d'écriture de la moisson, et la passe
/// apparie elle-même : une référence qui ne rend qu'une consultation sur l'hôte
/// que l'avis déclare désigne cette consultation. Le plus souvent elle est déjà
/// en stock — la recherche ne l'écrit pas, elle la nomme.
pub async fn annonces_resoudre(limite: i64, applique: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    resoudre(&repo, limite, applique).await
}

async fn resoudre(repo: &DecisionRepository<'_>, limite: i64, applique: bool) -> Result<()> {
    let debut = std::time::Instant::now();
    // A dead census entry is a typo the census probed once and never reached
    // again (`wwww.marches-publics.gouv.fr`, 22 consecutive failures). Left in,
    // it wins `hote_de_service` as the only sub-domain of the real host.
    let muets: HashSet<String> = repo
        .hotes_muets(NUITS_MUETTES)
        .await
        .map_err(|e| anyhow!("hôtes muets: {e}"))?
        .into_iter()
        .collect();
    let moteurs: HashMap<String, String> = repo
        .moteurs_par_hote()
        .await
        .map_err(|e| anyhow!("moteurs par hôte: {e}"))?
        .into_iter()
        .filter(|(h, _)| !muets.contains(h))
        .collect();
    let candidates = repo
        .annonces_a_jumeler(JOURNAL_FOND_MARCHE)
        .await
        .map_err(|e| anyhow!("candidates: {e}"))?;

    // Une candidate résoluble : pas de clé de jointure (sinon la moisson l'a
    // déjà jouée), une référence, et un hôte dont on sait interroger le moteur.
    let mut travail: Vec<(&lj_store::repository::AnnonceAJumeler, &str, String, String)> =
        Vec::new();
    for ligne in &candidates {
        let (Some(reference), true) = (
            ligne
                .reference
                .as_deref()
                .filter(|r| r.chars().count() >= 4),
            lj_annonces::cles_jointure_depuis_urls(
                ligne
                    .urls
                    .iter()
                    .map(String::as_str)
                    .chain(std::iter::once(ligne.depot.as_str())),
            )
            .is_empty(),
        ) else {
            continue;
        };
        let Some((hote, moteur)) = hote_et_moteur(&ligne.depot, &moteurs) else {
            continue;
        };
        travail.push((ligne, reference, hote, moteur));
    }
    travail.sort_by(|a, b| (a.1, &a.2, &a.3).cmp(&(b.1, &b.2, &b.3)));
    let a_faire = travail.len();
    if limite > 0 {
        travail.truncate(limite as usize);
    }

    let mut brutes: Vec<AnnonceBrute> = Vec::new();
    // Une recherche par clé, pas par candidate : deux avis d'un même marché
    // portent la même référence sur le même hôte.
    let mut cache: HashMap<(&str, &str), Vec<String>> = HashMap::new();
    let mut paires: Vec<(String, String)> = Vec::new();
    let (mut trouvees, mut vides, mut echecs, mut sans_adaptateur, mut sans_corroboration) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut par_moteur: BTreeMap<&str, usize> = BTreeMap::new();
    for (ligne, reference, hote, moteur) in &travail {
        let cle = (*reference, hote.as_str());
        let trouvailles = match cache.get(&cle) {
            Some(deja) => deja.clone(),
            None => {
                let rows = match moteur.as_str() {
                    "atline" => Some(annonces::atline::chercher(reference).await),
                    "aws" => Some(annonces::aws::chercher(reference).await),
                    "atexo" => Some(annonces::atexo::chercher(hote, reference).await),
                    "dematis" => Some(annonces::dematis::chercher(reference).await),
                    "achatpublic" => Some(annonces::achatpublic::chercher(reference).await),
                    _ => None,
                };
                let uids = match rows {
                    Some(Ok(rows)) if !rows.is_empty() => {
                        trouvees += 1;
                        *par_moteur.entry(moteur_statique(moteur)).or_default() += 1;
                        // Le moteur rend la page de résultat entière, pas la seule
                        // consultation cherchée. Celles qui portent la référence
                        // demandée départagent la page ; quand aucune ne la porte
                        // — Atline sert sa clé maison quand l'acheteur n'a pas
                        // saisi la sienne —, la page entière fait foi, et il faut
                        // alors qu'elle n'ait qu'une ligne.
                        let cherchee = reference_nue(reference);
                        let uid = |r: &AnnonceBrute| r.source_uid.clone();
                        let exactes: Vec<String> = rows
                            .iter()
                            .filter(|r| {
                                r.reference
                                    .as_deref()
                                    .is_some_and(|r| reference_nue(r) == cherchee)
                            })
                            .map(uid)
                            .collect();
                        let uids = if exactes.is_empty() {
                            rows.iter().map(uid).collect()
                        } else {
                            exactes
                        };
                        brutes.extend(rows);
                        uids
                    }
                    Some(Ok(_)) => {
                        vides += 1;
                        Vec::new()
                    }
                    Some(Err(e)) => {
                        echecs += 1;
                        tracing::warn!(reference, hote, moteur, error = %e, "annonces_resoudre: search failed");
                        Vec::new()
                    }
                    None => {
                        sans_adaptateur += 1;
                        Vec::new()
                    }
                };
                cache.insert(cle, uids.clone());
                if moteur.as_str() == "atline" || moteur.as_str() == "atexo" {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
                uids
            }
        };
        // Une référence qui ne rend qu'une consultation sur l'hôte que l'avis
        // lui-même déclare désigne cette consultation : c'est la jumelle, et
        // cette preuve-là est plus étroite que l'index de références global du
        // jumelage — elle porte l'hôte en plus de la clé.
        if let [source_uid] = trouvailles.as_slice() {
            match brutes.iter().find(|b| &b.source_uid == source_uid) {
                Some(b) if corrobore_brute(ligne, b) => {
                    paires.push((ligne.uid.clone(), source_uid.clone()));
                }
                _ => sans_corroboration += 1,
            }
        }
    }

    // Deux références peuvent rendre la même consultation, et une page de
    // résultat Atline en rend dix : le lot d'upsert ne supporte pas deux fois
    // la même clé.
    let brutes = annonces::dedup_source_uid(brutes);
    let ecrites = if applique && !brutes.is_empty() {
        let aujourdhui = Timestamp::now()
            .to_zoned(jiff::tz::TimeZone::get("Europe/Paris").expect("tzdb embarquée"))
            .date();
        let (_, stats, _) = reconcilier_et_ecrire(repo, brutes, aujourdhui).await?;
        stats.canaux
    } else {
        0
    };

    // La consultation trouvée est déjà en stock le plus souvent : la recherche
    // ne l'écrit pas, elle la nomme. La fusion se lit après l'écriture, pour
    // que les lignes neuves du lot aient leur uid.
    let cibles: HashMap<String, String> = repo
        .annonces_par_source(&paires.iter().map(|(_, s)| s.clone()).collect::<Vec<_>>())
        .await
        .map_err(|e| anyhow!("cibles des paires: {e}"))?
        .into_iter()
        .collect();
    let fusions: Vec<(String, String)> = paires
        .iter()
        .filter_map(|(boamp, source)| {
            cibles
                .get(source)
                .map(|cible| (boamp.clone(), cible.clone()))
        })
        .filter(|(boamp, cible)| boamp != cible)
        .collect();
    let absorbees = if applique && !fusions.is_empty() {
        repo.fusionner_annonces(&fusions)
            .await
            .map_err(|e| anyhow!("fusions: {e}"))?
    } else {
        0
    };

    tracing::info!(
        applique,
        a_faire,
        tentees = travail.len(),
        recherches = cache.len(),
        trouvees,
        vides,
        echecs,
        sans_adaptateur,
        sans_corroboration,
        canaux_ecrits = ecrites,
        paires = fusions.len(),
        absorbees,
        moteurs = %par_moteur.iter().map(|(m, n)| format!("{m}={n}")).collect::<Vec<_>>().join(","),
        elapsed_s = debut.elapsed().as_secs(),
        "annonces_resoudre"
    );
    Ok(())
}

/// Une référence de marché réduite à ses alphanumériques, en minuscules. Les
/// deux côtés la ponctuent à leur guise — `26/AO/014` ici, `26-AO-014` là.
fn reference_nue(reference: &str) -> String {
    reference
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Le nom du moteur en `'static`, pour la clé de comptage.
fn moteur_statique(moteur: &str) -> &'static str {
    match moteur {
        "atline" => "atline",
        "atexo" => "atexo",
        _ => "autre",
    }
}

/// Fusion par jumelles des lignes ouvertes que seul le BOAMP publie.
///
/// Quand l'avis BOAMP déclare une plateforme de dépôt sans identifiant de
/// dossier (`https://www.marches-securises.fr`), la moisson n'a aucune clé de
/// jointure : la plateforme est connue, le marché non. Reste la jumelle —
/// l'annonce que cette plateforme publie sous le même titre et le même acheteur.
/// La ligne plateforme est l'élue (elle porte les liens profonds du dossier),
/// la ligne BOAMP est absorbée.
///
/// Dry-run par défaut : sans `applique`, la passe lit, journalise chaque paire
/// et n'écrit rien.
pub async fn annonces_jumeler(applique: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    jumeler(&repo, applique).await
}

async fn jumeler(repo: &DecisionRepository<'_>, applique: bool) -> Result<()> {
    let debut = std::time::Instant::now();
    // Le recensement `profil_acheteur` est le seul oracle hôte → moteur : il
    // couvre les marques blanches et les instances multi-tenants, qu'aucune
    // liste de domaines ne nomme. Les moteurs qu'on ne moissonne pas n'ont
    // aucune ligne en stock, donc aucune jumelle à offrir.
    let moteurs: HashMap<String, String> = repo
        .moteurs_par_hote()
        .await
        .map_err(|e| anyhow!("moteurs par hôte: {e}"))?
        .into_iter()
        .filter(|(_, m)| annonces::discover::MOTEURS_CONNECTES.contains(&m.as_str()))
        .collect();

    let canaux = repo
        .canaux_plateforme_ouverts(JOURNAL_FOND_MARCHE)
        .await
        .map_err(|e| anyhow!("canaux ouverts: {e}"))?;
    let mut vivier: HashMap<String, Vec<&AnnonceChamps>> = HashMap::new();
    for c in &canaux {
        vivier
            .entry(canal_de_source(&c.source_uid))
            .or_default()
            .push(&c.annonce);
    }
    // Une ligne à deux canaux du même moteur n'est qu'une prétendante.
    for lignes in vivier.values_mut() {
        lignes.sort_by(|a, b| a.uid.cmp(&b.uid));
        lignes.dedup_by(|a, b| a.uid == b.uid);
    }

    // L'index des références est global : la référence désigne le marché, pas
    // l'hôte. Une ligne à deux canaux n'y entre qu'une fois.
    let mut par_reference: HashMap<String, Vec<&AnnonceChamps>> = HashMap::new();
    for c in &canaux {
        if let Some(r) = c.annonce.reference.as_deref().map(lj_fold::fold) {
            if !r.is_empty() && r != lj_fold::fold(&c.annonce.acteur_libelle) {
                par_reference.entry(r).or_default().push(&c.annonce);
            }
        }
    }
    for lignes in par_reference.values_mut() {
        lignes.sort_by(|a, b| a.uid.cmp(&b.uid));
        lignes.dedup_by(|a, b| a.uid == b.uid);
    }
    // L'index des titres est global lui aussi : la jumelle vit parfois sur un
    // autre canal que celui que l'avis déclare. La date d'échéance fait le
    // second facteur de la clé.
    let mut par_titre: HashMap<String, Vec<&AnnonceChamps>> = HashMap::new();
    for c in &canaux {
        let t = lj_fold::fold(&c.annonce.titre);
        if !t.is_empty() {
            par_titre.entry(t).or_default().push(&c.annonce);
        }
    }
    for lignes in par_titre.values_mut() {
        lignes.sort_by(|a, b| a.uid.cmp(&b.uid));
        lignes.dedup_by(|a, b| a.uid == b.uid);
    }
    let mut par_entite: HashMap<&str, Vec<&AnnonceChamps>> = HashMap::new();
    let mut par_libelle: HashMap<String, Vec<&AnnonceChamps>> = HashMap::new();
    for c in &canaux {
        if let Some(e) = c.annonce.acheteur_uid.as_deref() {
            par_entite.entry(e).or_default().push(&c.annonce);
        }
        let l = lj_fold::fold(&c.annonce.acteur_libelle);
        if !l.is_empty() {
            par_libelle.entry(l).or_default().push(&c.annonce);
        }
    }
    for lignes in par_entite.values_mut().chain(par_libelle.values_mut()) {
        lignes.sort_by(|a, b| a.uid.cmp(&b.uid));
        lignes.dedup_by(|a, b| a.uid == b.uid);
    }
    let canal_de_cible: HashMap<&str, String> = canaux
        .iter()
        .map(|c| (c.annonce.uid.as_str(), canal_de_source(&c.source_uid)))
        .collect();

    let candidates = repo
        .annonces_a_jumeler(JOURNAL_FOND_MARCHE)
        .await
        .map_err(|e| anyhow!("candidates au jumelage: {e}"))?;
    let total = candidates.len();
    // Combien de candidates portent chaque référence : à deux, elle ne
    // départage plus rien.
    let portees: HashMap<String, usize> = candidates
        .iter()
        .filter_map(|l| l.reference.as_deref().map(lj_fold::fold))
        .fold(HashMap::new(), |mut m, r| {
            *m.entry(r).or_default() += 1;
            m
        });
    let mut compte = CompteJumelage::default();
    let mut paires: Vec<(&lj_store::repository::AnnonceAJumeler, &str, String)> = Vec::new();
    for ligne in &candidates {
        // An extracted join key is an exact join, so no title overrides it. But a
        // key that attached nothing does not prove the target absent: a unique
        // same-buyer near title can be its only key.
        let avec_cle = !lj_annonces::cles_jointure_depuis_urls(
            ligne
                .urls
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(ligne.depot.as_str())),
        )
        .is_empty();
        if avec_cle {
            compte.avec_cle += 1;
        }
        // La référence d'abord : c'est la clé la plus forte, et la seule qui
        // n'a besoin ni du libellé d'acheteur ni de l'hôte déclaré.
        let declare = hote_et_moteur(&ligne.depot, &moteurs)
            .map(|(h, m)| canal_du_stock(canal_de_depot(&m, &h), &vivier));
        if !avec_cle {
            if let Jumelage::Une(uid) = jumelle_par_reference(
                ligne,
                &par_reference,
                &portees,
                &canal_de_cible,
                declare.as_deref(),
            ) {
                let canal = canal_de_cible.get(uid).cloned().unwrap_or_default();
                paires.push((ligne, uid, canal));
                continue;
            }
            // The title before the deposit: it sees the whole stock, where the
            // deposit pool is bounded to the declared channel and demands a
            // concurring buyer.
            if let Jumelage::Une(uid) = jumelle_par_titre_echeance(ligne, &par_titre) {
                let canal = canal_de_cible.get(uid).cloned().unwrap_or_default();
                paires.push((ligne, uid, canal));
                continue;
            }
        }
        // An exact corroboration, so before the deposit, which has none.
        if let Jumelage::Une(uid) = jumelle_par_acheteur(ligne, &par_entite, &par_libelle) {
            compte.acheteur += 1;
            let canal = canal_de_cible.get(uid).cloned().unwrap_or_default();
            paires.push((ligne, uid, canal));
            continue;
        }
        let Some(canal) = declare else {
            compte.hote_inconnu += 1;
            continue;
        };
        let Some(vivier) = vivier.get(&canal) else {
            compte.sans_jumelle += 1;
            continue;
        };
        match jumelle_de_depot(ligne, vivier) {
            Jumelage::Une(uid) => paires.push((ligne, uid, canal)),
            Jumelage::Ambigue => compte.ambigues += 1,
            Jumelage::Aucune => match jumelle_par_titre_proche(ligne, vivier) {
                Jumelage::Une(uid) => {
                    compte.proches += 1;
                    paires.push((ligne, uid, canal));
                }
                Jumelage::Aucune => compte.sans_jumelle += 1,
                Jumelage::Ambigue => compte.ambigues += 1,
            },
        }
    }
    // Une jumelle revendiquée par deux candidates : au plus l'une a raison, et
    // rien ne dit laquelle. Le garde-fou de l'ambiguïté vaut dans les deux sens.
    let revendications: HashMap<&str, usize> = paires.iter().fold(HashMap::new(), |mut m, p| {
        *m.entry(p.1).or_default() += 1;
        m
    });
    compte.disputees = paires.iter().filter(|p| revendications[p.1] > 1).count();
    compte.jumelles_disputees = revendications.values().filter(|n| **n > 1).count();
    paires.retain(|p| revendications[p.1] == 1);

    let champs: HashMap<&str, &AnnonceChamps> = canaux
        .iter()
        .map(|c| (c.annonce.uid.as_str(), &c.annonce))
        .collect();
    let propose = paires.len();
    paires.retain(|(ligne, uid, _)| champs.get(uid).is_some_and(|j| corrobore(ligne, j)));
    compte.sans_corroboration = propose - paires.len();

    let mut par_moteur: BTreeMap<&str, usize> = BTreeMap::new();
    for (ligne, uid, canal) in &paires {
        *par_moteur
            .entry(canal.split(':').next().unwrap_or(canal))
            .or_default() += 1;
        let jumelle = champs[uid];
        tracing::info!(
            boamp_uid = %ligne.uid,
            plateforme_uid = %uid,
            canal = %canal,
            boamp_titre = %ligne.titre,
            plateforme_titre = %jumelle.titre,
            boamp_acheteur = %ligne.acteur_libelle,
            plateforme_acheteur = %jumelle.acteur_libelle,
            acheteur_uid = ligne.acheteur_uid.as_deref().unwrap_or_default(),
            boamp_echeance = %ligne.echeance.map(|e| e.to_rfc3339()).unwrap_or_default(),
            plateforme_echeance = %jumelle.echeance.map(|e| e.to_rfc3339()).unwrap_or_default(),
            "annonces_jumeler: pair"
        );
    }

    // La ligne plateforme est l'élue : elle porte les liens profonds du
    // dossier (RC, DCE), que l'avis BOAMP n'a pas.
    let fusions: Vec<(String, String)> = paires
        .iter()
        .map(|(l, uid, _)| (l.uid.clone(), uid.to_string()))
        .collect();
    let absorbees = if applique {
        repo.fusionner_annonces(&fusions)
            .await
            .map_err(|e| anyhow!("fusions: {e}"))?
    } else {
        0
    };

    tracing::info!(
        applique,
        candidates = total,
        paires = fusions.len(),
        absorbees,
        moteurs = par_moteur
            .iter()
            .map(|(m, n)| format!("{m}={n}"))
            .collect::<Vec<_>>()
            .join(","),
        avec_cle = compte.avec_cle,
        hote_inconnu = compte.hote_inconnu,
        sans_jumelle = compte.sans_jumelle,
        ambigues = compte.ambigues,
        disputees = compte.disputees,
        jumelles_disputees = compte.jumelles_disputees,
        sans_corroboration = compte.sans_corroboration,
        proches = compte.proches,
        acheteur = compte.acheteur,
        elapsed_s = debut.elapsed().as_secs(),
        "annonces_jumeler"
    );
    Ok(())
}

/// Remonte la chaîne des parents d'un avis de résultat dans le lot : un
/// rectificatif cite l'avis de résultat qu'il corrige, qui cite la
/// consultation. S'arrête au premier id hors du lot (le marché réel) ou au
/// premier id revu (chaîne cyclique).
fn remonte_parent(parent_de: &HashMap<&str, &str>, depuis: Option<&str>) -> Option<String> {
    let mut courant = depuis?;
    let mut vus = HashSet::from([courant]);
    while let Some(&p) = parent_de.get(courant) {
        if !vus.insert(p) {
            break;
        }
        courant = p;
    }
    Some(courant.to_string())
}

async fn ecrire_resultats(
    repo: &DecisionRepository<'_>,
    resultats: Vec<ResultatMarche>,
) -> Result<(usize, AnnonceWriteStats, u64)> {
    // `annonce_lie` est un `source_uid` BOAMP : la consultation est derrière.
    // Mais un rectificatif d'avis de résultat cite l'avis de résultat qu'il
    // corrige, pas la consultation : on remonte la chaîne dans le lot jusqu'au
    // marché réel. Sans ça le rectificatif fonde sa propre ligne et réserve le
    // canal propre de l'original — deux revendications du même source_uid, et le
    // statement canaux rejette le lot entier.
    let parent_de: HashMap<&str, &str> = resultats
        .iter()
        .filter_map(|r| r.parent_idweb.as_deref().map(|p| (r.idweb.as_str(), p)))
        .collect();
    let cles: Vec<String> = resultats
        .iter()
        .filter_map(|r| remonte_parent(&parent_de, r.parent_idweb.as_deref()))
        .map(|id| format!("boamp:{id}"))
        .collect();
    let parents: HashMap<String, String> = repo
        .annonces_par_source(&cles)
        .await
        .map_err(|e| anyhow!("parents des résultats: {e}"))?
        .into_iter()
        .collect();

    // Plusieurs avis d'un même marché absent du stock (rectificatif, lots
    // publiés séparément) frappent le même uid via `parent_idweb` : le plus
    // récent garde la ligne, les canaux et arêtes de tous sont conservés.
    let mut orphelins: HashMap<String, (jiff::civil::Date, AnnonceWriteItem)> = HashMap::new();
    // Le résultat le plus récent gagne la ligne du marché (un résultat republié
    // remplace le premier) — et un seul UPDATE par marché.
    let mut poses: HashMap<String, (jiff::civil::Date, ResultatPose)> = HashMap::new();
    for r in &resultats {
        let lots: Vec<serde_json::Value> = r.attribution.lots.iter().map(lot_json).collect();
        let mut resultat = serde_json::json!({
            "lots": lots,
            "texte": r.attribution.texte,
            "avis_url": r.url_avis,
        });
        // Une seule fonction de forme pour les lots, à l'écriture comme au
        // filet : ce qui sort d'ici est déjà ce que la fiche affiche.
        embellir_resultat(&mut resultat);
        let mut sirens: Vec<String> = r
            .attribution
            .lots
            .iter()
            .filter_map(|l| l.siren.clone())
            .collect();
        sirens.sort();
        sirens.dedup();
        let aretes: Vec<(String, String)> = sirens
            .into_iter()
            .map(|s| (s, "titulaire".to_string()))
            .collect();
        let source = AnnonceSourceWriteItem {
            source_uid: format!("boamp:{}", r.idweb),
            url_avis: Some(r.url_avis.clone()),
        };

        let parent_effectif = remonte_parent(&parent_de, r.parent_idweb.as_deref());
        match parent_effectif
            .as_ref()
            .and_then(|id| parents.get(&format!("boamp:{id}")))
        {
            Some(parent) => {
                let pose = ResultatPose {
                    annonce_uid: parent.clone(),
                    resultat,
                    source,
                    aretes_siren: aretes,
                };
                match poses.entry(parent.clone()) {
                    Entry::Vacant(v) => {
                        v.insert((r.date_parution, pose));
                    }
                    Entry::Occupied(mut o) if o.get().0 < r.date_parution => {
                        o.insert((r.date_parution, pose));
                    }
                    Entry::Occupied(_) => {}
                }
            }
            None => {
                let mut sources = vec![source];
                // La consultation est déclarée mais absente du stock : on
                // réserve son canal, pour qu'elle se raccroche à cette ligne au
                // lieu d'en créer une seconde quand le backfill l'amènera. Une
                // chaîne cyclique retombe sur l'avis lui-même : rien à réserver.
                if let Some(p) = parent_effectif.as_deref().filter(|p| *p != r.idweb) {
                    sources.push(AnnonceSourceWriteItem {
                        source_uid: format!("boamp:{p}"),
                        url_avis: None,
                    });
                }
                let metiers = lj_annonces::metiers::metiers_du_marche(
                    &r.descripteurs,
                    &codes_des_lots(&r.contenu),
                    &r.objet,
                );
                let item = AnnonceWriteItem {
                    uid: marche_uid_de_resultat(&r.idweb, parent_effectif.as_deref()),
                    fond: JOURNAL_FOND_MARCHE.to_string(),
                    famille_uid: r.nature.map(|n| n.facet_uid().to_string()),
                    // Déjà en forme : le connecteur forme à la construction.
                    titre: r.objet.chars().take(400).collect(),
                    acteur_libelle: r.acheteur.clone().unwrap_or_default(),
                    zones: lj_annonces::zones::codes(&r.zones),
                    cpv_uid: r.cpv.as_deref().and_then(cpv_division_uid),
                    date_publication: date_chrono(r.date_parution),
                    // Sans échéance connue, la ligne n'entre jamais dans la fenêtre
                    // vivante — ce qui est juste : le marché est attribué, il ne prend
                    // plus d'offres.
                    echeance: None,
                    url_action: r.url_avis.clone(),
                    description: r.contenu.description.clone(),
                    metiers,
                    details: {
                        let mut d = serde_json::json!({
                            "cpv": r.cpv,
                            "liens": [{"moteur": "boamp", "avis": r.url_avis}],
                            "resultat": resultat,
                        });
                        poser_contenu(&mut d, &r.contenu, &r.descripteurs);
                        d
                    },
                    aretes_siren: aretes,
                    sources,
                };
                match orphelins.entry(item.uid.clone()) {
                    Entry::Vacant(v) => {
                        v.insert((r.date_parution, item));
                    }
                    Entry::Occupied(mut o) => {
                        let (date, en_place) = o.get_mut();
                        let perdant = if *date < r.date_parution {
                            *date = r.date_parution;
                            std::mem::replace(en_place, item)
                        } else {
                            item
                        };
                        for s in perdant.sources {
                            if !en_place
                                .sources
                                .iter()
                                .any(|g| g.source_uid == s.source_uid)
                            {
                                en_place.sources.push(s);
                            }
                        }
                        for a in perdant.aretes_siren {
                            if !en_place.aretes_siren.contains(&a) {
                                en_place.aretes_siren.push(a);
                            }
                        }
                    }
                }
            }
        }
    }

    let orphelins: Vec<AnnonceWriteItem> = orphelins.into_values().map(|(_, i)| i).collect();
    let mut stats = AnnonceWriteStats::default();
    for lot in orphelins.chunks(2_000) {
        let s = repo
            .upsert_annonces(lot)
            .await
            .map_err(|e| anyhow!("upsert résultats: {e}"))?;
        stats.annonces += s.annonces;
        stats.canaux += s.canaux;
        stats.aretes += s.aretes;
    }
    let ecrits: Vec<String> = orphelins.into_iter().map(|i| i.uid).collect();
    replier_les_metiers(repo, Some(&ecrits)).await?;
    let liens: Vec<ResultatPose> = poses.into_values().map(|(_, p)| p).collect();
    let rattaches = repo
        .poser_resultats(&liens)
        .await
        .map_err(|e| anyhow!("poser les résultats: {e}"))?;
    Ok((resultats.len(), stats, rattaches))
}

/// An award is final: the window is replayed only to catch a notice published
/// while the harvest was down.
const RESULTATS_TED_JOURS: u32 = 30;

/// Poses TED awards on markets already in stock. An award whose consultation is
/// missing founds no orphan row, unlike the BOAMP one: the TED consultation is
/// harvested anyway, so its absence is a breakdown, not a gap in coverage.
async fn ecrire_resultats_ted(
    repo: &DecisionRepository<'_>,
    resultats: Vec<lj_sources_annonces::ted::ResultatTed>,
) -> Result<(usize, u64)> {
    let cles: Vec<String> = resultats
        .iter()
        .flat_map(|r| r.cles_consultation.iter().cloned())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let par_cle: HashMap<String, String> = repo
        .annonces_par_source(&cles)
        .await
        .map_err(|e| anyhow!("marchés des attributions TED: {e}"))?
        .into_iter()
        .collect();

    // Lots published apart and corrigenda put several award notices on one
    // market: the most recent keeps the row.
    let mut poses: HashMap<String, (jiff::civil::Date, ResultatPose)> = HashMap::new();
    for r in &resultats {
        let Some(uid) = r.cles_consultation.iter().find_map(|c| par_cle.get(c)) else {
            continue;
        };
        let mut resultat = serde_json::json!({
            "lots": r.lots.iter().map(lot_json).collect::<Vec<_>>(),
            "texte": serde_json::Value::Null,
            "avis_url": r.url_avis,
        });
        embellir_resultat(&mut resultat);
        let pose = ResultatPose {
            annonce_uid: uid.clone(),
            resultat,
            source: AnnonceSourceWriteItem {
                source_uid: format!("ted:{}", r.publication),
                url_avis: Some(r.url_avis.clone()),
            },
            aretes_siren: Vec::new(),
        };
        let paru = r.date_publication.unwrap_or(jiff::civil::date(1970, 1, 1));
        match poses.entry(uid.clone()) {
            std::collections::hash_map::Entry::Occupied(mut e) if e.get().0 < paru => {
                e.insert((paru, pose));
            }
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert((paru, pose));
            }
            _ => {}
        }
    }
    let lot: Vec<ResultatPose> = poses.into_values().map(|(_, p)| p).collect();
    let n = repo
        .poser_resultats(&lot)
        .await
        .map_err(|e| anyhow!("pose des attributions TED: {e}"))?;
    Ok((resultats.len(), n))
}

/// Trigram similarity below which the buyer of an award and the buyer of the
/// market it would close are not the same: an unambiguous reference naming another
/// buyer falls under it.
const SEUIL_ACHETEUR_AWS: f64 = 0.4;

const FICHES_AWS_EN_VOL: usize = 8;

/// Poses AWS awards. The notice carries its own publication reference, so the
/// only handle back to the consultation is the reference the buyer gave, which
/// says nothing unless it designates one market and one buyer: the store keeps
/// the first condition, the similarity here the second.
async fn ecrire_resultats_aws(
    repo: &DecisionRepository<'_>,
    attributions: Vec<lj_sources_annonces::aws::AttributionListee>,
) -> Result<(usize, usize, u64)> {
    let references: Vec<String> = attributions
        .iter()
        .map(|a| a.reference.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let cibles: HashMap<String, (String, String)> = repo
        .marches_par_reference(&references)
        .await
        .map_err(|e| anyhow!("marchés des attributions AWS: {e}"))?
        .into_iter()
        .map(|m| (m.reference, (m.uid, m.acteur_libelle)))
        .collect();

    let vus = attributions.len();
    let candidates: Vec<(String, lj_sources_annonces::aws::AttributionListee)> = attributions
        .into_iter()
        .filter_map(|a| {
            let (uid, libelle) = cibles.get(&a.reference)?;
            if lj_annonces::similarite_titres(&a.acheteur, libelle) < SEUIL_ACHETEUR_AWS {
                return None;
            }
            Some((uid.clone(), a))
        })
        .collect();
    let rattachees = candidates.len();

    let fiches = futures::stream::iter(candidates.into_iter().map(|(uid, a)| async move {
        match annonces::aws::attribution_lots(&a.url_avis).await {
            Ok(lots) if !lots.is_empty() => Some((uid, a, lots)),
            Ok(_) => None,
            Err(e) => {
                tracing::warn!(error = %e, url = %a.url_avis, "annonces: AWS award record unreadable");
                None
            }
        }
    }))
    .buffer_unordered(FICHES_AWS_EN_VOL)
    .filter_map(|r| async move { r })
    .collect::<Vec<_>>()
    .await;

    let mut poses: HashMap<String, (jiff::civil::Date, ResultatPose)> = HashMap::new();
    for (uid, a, lots) in fiches {
        let mut resultat = serde_json::json!({
            "lots": lots.iter().map(lot_json).collect::<Vec<_>>(),
            "texte": serde_json::Value::Null,
            "avis_url": a.url_avis,
        });
        embellir_resultat(&mut resultat);
        let pose = ResultatPose {
            annonce_uid: uid.clone(),
            resultat,
            source: AnnonceSourceWriteItem {
                source_uid: format!("aws:pub:{}", a.publication),
                url_avis: Some(a.url_avis.clone()),
            },
            aretes_siren: Vec::new(),
        };
        let paru = a.date_publication.unwrap_or(jiff::civil::date(1970, 1, 1));
        match poses.entry(uid) {
            Entry::Occupied(mut e) if e.get().0 < paru => {
                e.insert((paru, pose));
            }
            Entry::Vacant(v) => {
                v.insert((paru, pose));
            }
            _ => {}
        }
    }
    let lot: Vec<ResultatPose> = poses.into_values().map(|(_, p)| p).collect();
    let n = repo
        .poser_resultats(&lot)
        .await
        .map_err(|e| anyhow!("pose des attributions AWS: {e}"))?;
    Ok((vus, rattachees, n))
}

/// Walks a DECP export and poses what it can attach.
pub async fn annonces_decp(depuis: Option<&str>, applique: bool) -> Result<()> {
    let debut = std::time::Instant::now();
    let settings = Settings::from_env()?;
    let fenetre: jiff::civil::Date = match depuis {
        Some(s) => s
            .parse()
            .map_err(|e| anyhow!("annonces-decp : date {s} illisible: {e}"))?,
        None => Timestamp::now()
            .to_zoned(TimeZone::UTC)
            .date()
            .saturating_sub(jiff::Span::new().days(FENETRE_DECP_JOURS)),
    };
    let dir = settings.paths().decp();
    let mut modifies = BTreeMap::new();
    for jeu in &annonces::decp::JEUX {
        let modifie = annonces::decp::modifie_le(jeu)
            .await
            .map_err(|e| anyhow!("catalogue DECP {}: {e}", jeu.id))?;
        modifies.insert(jeu.id.to_string(), modifie);
    }
    let mut manifeste =
        annonces::decp::Manifeste::load(&dir).map_err(|e| anyhow!("manifeste DECP: {e}"))?;
    if manifeste.deja_vu(&modifies, fenetre) {
        tracing::info!(
            depuis = %fenetre,
            modifies = ?modifies.values().collect::<Vec<_>>(),
            "annonces: DECP unchanged, nothing to re-read"
        );
        return Ok(());
    }

    // The two datasets overlap on 2022 to 2024 and declare the same contracts
    // there, which would double every lot of the period.
    let mut vues = HashSet::new();
    let mut declarations = Vec::new();
    for jeu in &annonces::decp::JEUX {
        let fichier = annonces::decp::telecharger(&dir, jeu, fenetre)
            .await
            .map_err(|e| anyhow!("export DECP {}: {e}", jeu.id))?;
        for d in annonces::decp::lire(&fichier, jeu)
            .map_err(|e| anyhow!("lecture de l'export DECP {}: {e}", jeu.id))?
        {
            if vues.insert((
                d.acheteur_siren.clone(),
                d.id.clone(),
                d.objet.clone(),
                d.date_notification,
            )) {
                declarations.push(d);
            }
        }
    }

    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let plancher = chrono::DateTime::from_timestamp(
        fenetre
            .to_zoned(TimeZone::UTC)
            .map_err(|e| anyhow!("fenêtre DECP: {e}"))?
            .timestamp()
            .as_second()
            - i64::from(RECUL_DECP_JOURS) * 86_400,
        0,
    )
    .ok_or_else(|| anyhow!("fenêtre DECP hors bornes"))?;
    // The buyer edge is what opens the low band and spares the trigram, and the
    // daily harvest only ever posts it on open rows.
    let aretes = repo
        .relier_annonces_par_denomination(applique.then_some(plancher))
        .await
        .map_err(|e| anyhow!("rattachement par dénomination: {e}"))?;
    let (vus, rattachees, via_cpv, poses) =
        ecrire_resultats_decp(&repo, declarations, plancher, applique).await?;
    // A dry run must not eat the signal: recording what it read would make the
    // next real pass skip a publication nobody ever wrote down.
    if applique {
        manifeste.modifies = modifies;
        manifeste.depuis = Some(fenetre.to_string());
        manifeste
            .save(&dir)
            .map_err(|e| anyhow!("manifeste DECP: {e}"))?;
    }
    tracing::info!(
        vus,
        aretes,
        rattachees,
        via_cpv,
        poses,
        applique,
        depuis = %fenetre,
        elapsed_s = debut.elapsed().as_secs(),
        "annonces: DECP awards"
    );
    Ok(())
}

/// Trigram similarity below which the buyer a declaration names and the buyer
/// of the market it would close are not the same. Same frontier as the AWS
/// channel, measured there on its unambiguous references.
const SEUIL_ACHETEUR_DECP: f64 = 0.4;

/// Trigram similarity below which an object and a title do not name the same
/// contract. On a sample of declarations the band above is right without
/// exception; just under, « Assurances pour les besoins du SDIS du
/// Loir-et-Cher » pairs with « assurances pour les besoins du SIS 68 » at 0.62.
const SEUIL_OBJET_DECP: i32 = 70;

/// Object threshold when the buyer is the same SIREN and the CPV division
/// agrees. Both guards earn their place: the CPV alone at a lower threshold lets
/// more false pairs through.
const SEUIL_OBJET_DECP_CPV: i32 = 55;

/// Months a declaration may look back for the consultation it closes.
const RECUL_DECP_MOIS: i32 = 15;
const RECUL_DECP_JOURS: i32 = RECUL_DECP_MOIS * 31;

/// Days of notification the scheduled pass walks. The export filters on the
/// notification date, so a declaration whose buyer took longer than this to
/// declare it is never in window again: most are declared inside it, and the
/// manual pass fetches the rest of the history.
const FENETRE_DECP_JOURS: i64 = 180;

/// Rare tokens of a title, the blocking key: a pair above the object threshold
/// always shares a word of more than four letters.
fn jetons(s: &str) -> HashSet<String> {
    lj_fold::fold(s)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|m| m.chars().count() > 4)
        .map(str::to_string)
        .collect()
}

/// Poses the awards a DECP export declares. The buyer guards, the object
/// decides, and an object that fits two markets decides nothing: attaching an
/// award to the wrong market costs more than attaching none.
pub async fn ecrire_resultats_decp(
    repo: &DecisionRepository<'_>,
    declarations: Vec<lj_sources_annonces::decp::AttributionDecp>,
    depuis: chrono::DateTime<chrono::Utc>,
    applique: bool,
) -> Result<(usize, usize, usize, u64)> {
    let marches = repo
        .marches_clos_sans_resultat(JOURNAL_FOND_MARCHE, depuis)
        .await
        .map_err(|e| anyhow!("marchés clos sans résultat: {e}"))?;
    let sirens: Vec<String> = declarations
        .iter()
        .flat_map(|d| std::iter::once(d.acheteur_siren.clone()).chain(d.titulaire_siren.clone()))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let denominations: HashMap<String, String> = repo
        .denominations_par_siren(&sirens)
        .await
        .map_err(|e| anyhow!("dénominations DECP: {e}"))?
        .into_iter()
        .collect();

    let jetons_marche: Vec<HashSet<String>> = marches.iter().map(|m| jetons(&m.titre)).collect();
    let mut par_jeton: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, js) in jetons_marche.iter().enumerate() {
        for j in js {
            par_jeton.entry(j.as_str()).or_default().push(i);
        }
    }

    let vus = declarations.len();
    // DECP declares one row per lot, so a market takes as many rows as it has
    // lots: they gather into the single `resultat` block the other channels
    // write, and the last declaration to name the market carries its channel.
    let mut lots_par_marche: BTreeMap<String, Vec<LotAttribue>> = BTreeMap::new();
    let mut apercu: Vec<(i32, String, String)> = Vec::new();
    let mut rattachees = 0usize;
    let mut via_cpv = 0usize;
    let mut bande_cpv: Vec<(i32, String, String)> = Vec::new();
    for d in &declarations {
        let fin = chrono::NaiveDate::from_ymd_opt(
            d.date_notification.year().into(),
            d.date_notification.month() as u32,
            d.date_notification.day() as u32,
        );
        let (Some(fin), Some(denomination)) = (fin, denominations.get(&d.acheteur_siren)) else {
            continue;
        };
        let debut = fin - chrono::Duration::days(RECUL_DECP_JOURS.into());
        // The two rarest tokens of the object: scanning « travaux », present on
        // tens of thousands of titles, would cost the whole stock per
        // declaration, and one token alone misses the pair whose single rare
        // word is the one the two wordings spell differently.
        let mut seaux: Vec<&Vec<usize>> = jetons(&d.objet)
            .iter()
            .filter_map(|j| par_jeton.get(j.as_str()))
            .collect();
        seaux.sort_unstable_by_key(|ids| ids.len());
        if seaux.is_empty() {
            continue;
        }
        let candidats: HashSet<usize> = seaux
            .iter()
            .take(2)
            .flat_map(|v| v.iter().copied())
            .collect();
        let mut meilleur: Option<(i32, usize)> = None;
        let mut ex_aequo = false;
        for i in candidats {
            let m = &marches[i];
            let ech = m.echeance.date_naive();
            if ech > fin || ech < debut {
                continue;
            }
            let siren_sur = m.acheteur_siren.as_deref() == Some(d.acheteur_siren.as_str());
            let concorde = siren_sur
                || (m.acheteur_siren.is_none()
                    && lj_annonces::similarite_titres(&m.acteur_libelle, denomination)
                        >= SEUIL_ACHETEUR_DECP);
            if !concorde {
                continue;
            }
            let cpv_concorde = siren_sur
                && match (&m.cpv_uid, &d.cpv) {
                    (Some(u), Some(c)) => lj_annonces::cpv_division_uid(c).as_ref() == Some(u),
                    _ => false,
                };
            let seuil = if cpv_concorde {
                SEUIL_OBJET_DECP_CPV
            } else {
                SEUIL_OBJET_DECP
            };
            let score = (lj_annonces::similarite_titres(&m.titre, &d.objet) * 100.0) as i32;
            if score < seuil {
                continue;
            }
            match meilleur {
                Some((s, _)) if score < s => {}
                Some((s, _)) if score == s => ex_aequo = true,
                _ => {
                    meilleur = Some((score, i));
                    ex_aequo = false;
                }
            }
        }
        let Some((score, i)) = meilleur.filter(|_| !ex_aequo) else {
            continue;
        };
        rattachees += 1;
        if score < SEUIL_OBJET_DECP {
            via_cpv += 1;
            bande_cpv.push((score, marches[i].titre.clone(), d.objet.clone()));
        }
        if apercu.len() < 25 {
            apercu.push((score, marches[i].titre.clone(), d.objet.clone()));
        }
        let titulaire = d
            .titulaire_siren
            .as_ref()
            .and_then(|s| denominations.get(s))
            .cloned();
        let lot = LotAttribue {
            description: None,
            titulaire: titulaire.unwrap_or_default(),
            siren: d.titulaire_siren.clone(),
            ville: None,
            montant: d.montant,
            devise: Some("EUR".to_string()),
            nb_offres: d.nb_offres,
            date_attribution: Some(d.date_notification),
        };
        lots_par_marche
            .entry(marches[i].uid.clone())
            .or_default()
            .push(lot);
    }

    let mut lot_a_poser: Vec<ResultatPose> = Vec::new();
    for (uid, lots) in lots_par_marche {
        let mut resultat = serde_json::json!({
            "lots": lots.iter().map(lot_json).collect::<Vec<_>>(),
            "texte": serde_json::Value::Null,
            "avis_url": serde_json::Value::Null,
        });
        embellir_resultat(&mut resultat);
        let aretes_siren = lots
            .iter()
            .filter_map(|l| l.siren.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .map(|s| (s, "titulaire".to_string()))
            .collect();
        // A declaration carries no identity of its own: its `id` is the number
        // its buyer gave the contract, and the same buyer repeats it across
        // contracts. The market it closes is what makes the channel unique, and
        // a market takes one DECP channel however many lots it declares.
        let source_uid = format!("decp:{}", uid.trim_start_matches("marche:"));
        lot_a_poser.push(ResultatPose {
            annonce_uid: uid,
            resultat,
            source: AnnonceSourceWriteItem {
                source_uid,
                url_avis: None,
            },
            aretes_siren,
        });
    }
    if !applique {
        for (score, titre, objet) in apercu.iter().take(25) {
            tracing::info!(score, titre, objet, "annonces: DECP award kept");
        }
        for (score, titre, objet) in bande_cpv
            .iter()
            .step_by(1.max(bande_cpv.len() / 40))
            .take(40)
        {
            tracing::info!(score, titre, objet, "annonces: DECP award CPV band");
        }
        return Ok((vus, rattachees, via_cpv, lot_a_poser.len() as u64));
    }
    let n = repo
        .poser_resultats(&lot_a_poser)
        .await
        .map_err(|e| anyhow!("pose des attributions DECP: {e}"))?;
    Ok((vus, rattachees, via_cpv, n))
}

fn lot_json(l: &LotAttribue) -> serde_json::Value {
    serde_json::json!({
        "titulaire": l.titulaire,
        "siren": l.siren,
        "ville": l.ville,
        "description": l.description,
        "montant": l.montant,
        "devise": l.devise,
        "nb_offres": l.nb_offres,
        "date_attribution": l.date_attribution.map(|d| d.to_string()),
    })
}

/// Re-normalise un bloc `details.resultat` en place — la même forme que
/// `lot_json` donne à l'écriture, appliquée au stock par la régularisation.
/// Rend vrai si le bloc a changé. Le `texte` ne bouge pas : ses sauts de
/// ligne sont la structure du paragraphe d'attribution, rendue telle quelle.
fn embellir_resultat(resultat: &mut serde_json::Value) -> bool {
    let Some(lots) = resultat.get_mut("lots").and_then(|l| l.as_array_mut()) else {
        return false;
    };
    let mut change = false;
    for lot in lots {
        if let Some(t) = lot.get("titulaire").and_then(|v| v.as_str()) {
            let propre = lj_annonces::nettoyer_acheteur(t);
            if propre != t {
                lot["titulaire"] = propre.into();
                change = true;
            }
        }
        if let Some(d) = lot.get("description").and_then(|v| v.as_str()) {
            let propre = lj_annonces::embellir_titre(d);
            if propre != d {
                lot["description"] = propre.into();
                change = true;
            }
        }
    }
    change
}

/// Projette une ligne canonique en écriture. `details` porte le spécifique du
/// fond — jamais le payload source brut : les liens par usage, la référence
/// acheteur, la procédure, le CPV complet et les canaux, tous normalisés.
fn ecrire(c: &AnnonceCanonique) -> AnnonceWriteItem {
    let liens: Vec<serde_json::Value> = c
        .sources
        .iter()
        .map(|s| {
            serde_json::json!({
                "moteur": s.moteur,
                "avis": s.url_avis,
                "rc": s.url_rc,
                "dce": s.url_dce,
            })
        })
        .collect();
    let mut details = serde_json::json!({
        "reference": c.reference,
        "procedure": c.procedure,
        "cpv": c.cpv,
        "liens": liens,
        "depot": c.url_depot,
    });
    poser_contenu(&mut details, &c.contenu, &c.descripteurs);
    let metiers = lj_annonces::metiers::metiers_du_marche(
        &c.descripteurs,
        &codes_des_lots(&c.contenu),
        &c.titre,
    );
    AnnonceWriteItem {
        uid: c.uid.clone(),
        fond: JOURNAL_FOND_MARCHE.to_string(),
        famille_uid: c.famille_uid.map(str::to_string),
        titre: c.titre.chars().take(400).collect(),
        description: c.contenu.description.clone(),
        acteur_libelle: c.acheteur_libelle.clone(),
        zones: lj_annonces::zones::codes(&c.zones),
        cpv_uid: c.cpv_uid.clone(),
        date_publication: date_chrono(c.date_publication),
        echeance: c
            .echeance
            .and_then(|t| chrono::DateTime::from_timestamp_millis(t.as_millisecond())),
        url_action: c.url_action.clone(),
        details,
        metiers,
        aretes_siren: c
            .sirens
            .iter()
            .map(|s| (s.clone(), "acheteur".to_string()))
            .collect(),
        sources: c
            .sources
            .iter()
            .map(|s| AnnonceSourceWriteItem {
                source_uid: s.source_uid.clone(),
                url_avis: s.url_avis.clone(),
            })
            .collect(),
    }
}

/// Folds the trades again on the merged row, whose lots and descriptors may come
/// from a channel the batch did not carry. `parmi` bounds the rows.
async fn replier_les_metiers(
    repo: &DecisionRepository<'_>,
    parmi: Option<&[String]>,
) -> Result<u64> {
    let metiers: Vec<(String, Vec<String>)> = repo
        .annonces_a_metier(parmi)
        .await
        .map_err(|e| anyhow!("rows to fold: {e}"))?
        .into_iter()
        .filter_map(|a| {
            let m = lj_annonces::metiers::metiers_du_marche(
                &a.descripteurs,
                &a.codes_des_lots,
                &a.titre,
            );
            (!m.is_empty() && m != a.metiers).then_some((a.uid, m))
        })
        .collect();
    let mut poses = 0;
    for lot in metiers.chunks(2_000) {
        poses += repo
            .replier_metiers(lot)
            .await
            .map_err(|e| anyhow!("fold trades: {e}"))?;
    }
    Ok(poses)
}

fn codes_des_lots(contenu: &ContenuAnnonce) -> Vec<&str> {
    contenu
        .lots
        .iter()
        .filter_map(|l| l.cpv.as_deref())
        .collect()
}

/// Grafts onto a `details` block what the notice carries beyond its title. An
/// empty field does not enter: the blob stays at the grain of its usage.
fn poser_contenu(
    details: &mut serde_json::Value,
    contenu: &ContenuAnnonce,
    descripteurs: &[String],
) {
    let bloc = details
        .as_object_mut()
        .expect("details construit comme objet");
    if !contenu.lots.is_empty() {
        bloc.insert("lots".into(), serde_json::json!(contenu.lots));
    }
    if !contenu.criteres.is_empty() {
        bloc.insert("criteres".into(), serde_json::json!(contenu.criteres));
    }
    if let Some(m) = &contenu.montant_estime {
        bloc.insert("montant_estime".into(), serde_json::json!(m));
    }
    if !descripteurs.is_empty() {
        bloc.insert("descripteurs".into(), serde_json::json!(descripteurs));
    }
}

/// Pause between two notices on the same HOST. Hosts advance in parallel, each
/// at this rate.
const POLITESSE: std::time::Duration = std::time::Duration::from_millis(300);

/// Hosts pulled at once. The grain is the host and not the engine: 40 distinct
/// servers run Atexo, and one of them hanging starved the 39 others.
const HOSTS_IN_FLIGHT: usize = 16;

/// Lots d'écriture — un aller-retour par tranche, pas par fiche.
const LOT_DETAIL: usize = 200;

/// Le bloc `details.detail` d'une page lue. Un champ absent n'entre pas : le
/// bloc vide vaut « visitée, rien à en tirer » et sort quand même la ligne de
/// la file.
fn bloc_detail(d: &annonces::DetailAnnonce) -> serde_json::Value {
    let mut bloc = serde_json::Map::new();
    // The day the current parser read the notice.
    let vu = Timestamp::now().to_zoned(TimeZone::UTC).date().to_string();
    bloc.insert("vu".into(), serde_json::json!(vu));
    if let Some(cpv) = &d.cpv {
        bloc.insert("cpv".into(), serde_json::json!(cpv));
    }
    if let Some(procedure) = &d.procedure {
        bloc.insert("procedure".into(), serde_json::json!(procedure));
    }
    let mut bloc = serde_json::Value::Object(bloc);
    poser_contenu(&mut bloc, &d.contenu, &[]);
    bloc
}

/// Enrichissement par page de détail. La file est dérivée — les annonces
/// ouvertes sans bloc `detail` et sans canal BOAMP — et ne se vide que par la
/// pose du bloc : un fetch en échec n'écrit rien, la ligne repasse la nuit
/// suivante. Un moteur dont l'extracteur n'est pas écrit rend une erreur
/// franche, ses lignes restent en file, et le compteur d'échecs le dit.
///
/// Les moteurs avancent en parallèle mais chacun en série, et un hôte qui
/// rationne ses fiches impose sa cadence à sa branche : `budget_min` empêche
/// cette branche de tenir la chaîne quotidienne des heures durant. Passé le
/// budget elle rend la main, ses lignes comptent en `remises` et repassent la
/// nuit suivante. Un rattrapage manuel monte le plafond pour vider la file
/// d'une traite — c'est ce budget qui borne, jamais l'hôte.
async fn tirer_les_fiches(
    repo: &DecisionRepository<'_>,
    limite: i64,
    budget_min: u64,
    file: FileEnrichissement,
) -> Result<()> {
    let debut = std::time::Instant::now();
    let budget = std::time::Duration::from_secs(budget_min * 60);

    let file = repo
        .annonces_a_enrichir(
            JOURNAL_FOND_MARCHE,
            &lj_annonces::PRIORITE,
            &lj_annonces::SANS_FICHE,
            limite,
            file,
        )
        .await
        .map_err(|e| anyhow!("file d'enrichissement: {e}"))?;
    let mut par_hote: HashMap<(String, String), Vec<AnnonceAEnrichir>> = HashMap::new();
    for ligne in file {
        let hote = annonces::discover::hote_de(&ligne.url_avis).unwrap_or_default();
        par_hote
            .entry((ligne.moteur.clone(), hote))
            .or_default()
            .push(ligne);
    }

    let lus: Vec<_> = futures::stream::iter(par_hote.into_iter().map(
        |((moteur, _hote), lignes)| async move {
            let (mut poses, mut echecs, mut derniere) = (Vec::new(), 0u32, String::new());
            let mut remises = 0usize;
            for (i, ligne) in lignes.iter().enumerate() {
                // Deadline global: an unserved group would otherwise start its own
                // budget on getting a slot, and the run would outlive its watchdog.
                if debut.elapsed() >= budget {
                    remises = lignes.len() - i;
                    break;
                }
                if i > 0 {
                    tokio::time::sleep(POLITESSE).await;
                }
                match annonces::detail(&moteur, &ligne.url_avis).await {
                    Ok(d) => poses.push(DetailPose {
                        annonce_uid: ligne.uid.clone(),
                        cpv_uid: d.cpv.as_deref().and_then(cpv_division_uid),
                        detail: bloc_detail(&d),
                        description: d.contenu.description.clone(),
                        acheteur: d.acheteur.clone(),
                        zones: lj_annonces::zones::codes(&d.zones),
                    }),
                    Err(e) => {
                        echecs += 1;
                        derniere = e.to_string();
                    }
                }
            }
            (moteur, poses, echecs, derniere, remises)
        },
    ))
    .buffer_unordered(HOSTS_IN_FLIGHT)
    .collect()
    .await;

    let (mut total_ok, mut total_echecs, mut ecrits, mut total_remises) =
        (0usize, 0u32, 0u64, 0usize);
    let mut par_moteur: BTreeMap<String, (usize, u32, usize, String)> = BTreeMap::new();
    let mut lues: Vec<String> = Vec::new();
    for (moteur, poses, echecs, derniere, remises) in lus {
        let cumul = par_moteur.entry(moteur.clone()).or_default();
        cumul.0 += poses.len();
        cumul.1 += echecs;
        cumul.2 += remises;
        if !derniere.is_empty() {
            cumul.3 = derniere;
        }
        total_ok += poses.len();
        total_echecs += echecs;
        total_remises += remises;
        for lot in poses.chunks(LOT_DETAIL) {
            ecrits += repo
                .poser_detail_annonces(lot)
                .await
                .map_err(|e| anyhow!("poser les détails ({moteur}): {e}"))?;
        }
        lues.extend(poses.into_iter().map(|p| p.annonce_uid));
    }
    // The page may be the only place a market lists its works lots.
    let metiers_replies = replier_les_metiers(repo, Some(&lues)).await?;
    for (moteur, (ok, echecs, remises, erreur)) in &par_moteur {
        tracing::info!(
            moteur = %moteur,
            ok,
            echecs,
            remises,
            erreur = %erreur,
            "annonces_enrichir: moteur"
        );
    }

    tracing::info!(
        ok = total_ok,
        echecs = total_echecs,
        remises = total_remises,
        ecrits,
        metiers_replies,
        elapsed_s = debut.elapsed().as_secs(),
        "annonces_enrichir"
    );
    Ok(())
}

/// Catch-up only: the nightly pull happens inside the harvest, on the rows it
/// has just written.
pub async fn annonces_enrichir(
    limite: i64,
    budget_min: u64,
    file: FileEnrichissement,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    tirer_les_fiches(&repo, limite, budget_min, file).await
}

/// Recensement des profils d'acheteurs : les hôtes cités par les avis actifs du
/// BOAMP et de TED, plus ceux que les acheteurs déclarent sur data.gouv, classés
/// par moteur. Alimente `profil_acheteur`, dont dépendent les moteurs
/// multi-tenants (Atexo, Safetender). Hebdomadaire : le parc de profils bouge en
/// mois, pas en jours.
pub async fn annonces_discover() -> Result<()> {
    let debut = std::time::Instant::now();
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let recensement = annonces::discover::recenser_profils()
        .await
        .map_err(|e| anyhow!("recensement: {e}"))?;
    let items: Vec<(String, Option<String>, i32)> = recensement
        .profils
        .iter()
        .map(|p| (p.host.clone(), p.moteur.map(str::to_string), p.avis_boamp))
        .collect();
    let ecrits = repo
        .upsert_profils_acheteurs(&items)
        .await
        .map_err(|e| anyhow!("upsert profils: {e}"))?;
    // La couverture du fond, mesurée par avis et par témoin : sa dérive est le
    // signal qu'un moteur a changé d'interface ou qu'un nouvel éditeur prend des
    // parts ; l'écart entre témoins dit lequel voit un gisement que l'autre
    // ignore.
    let avis: usize = recensement.temoins.iter().map(|t| t.avis).sum();
    let avis_couverts: usize = recensement.temoins.iter().map(|t| t.couverts).sum();
    let pct = |couverts: usize, avis: usize| {
        format!(
            "{:.1}",
            if avis > 0 {
                100.0 * couverts as f64 / avis as f64
            } else {
                0.0
            }
        )
    };
    for t in &recensement.temoins {
        tracing::info!(
            canal = t.canal,
            avis = t.avis,
            couverts = t.couverts,
            couverture_pct = pct(t.couverts, t.avis),
            "annonces_discover_temoin"
        );
    }
    rapport_rattrapage!(
        ecrits,
        hotes = recensement.profils.len(),
        ecrits,
        moteurs = recensement
            .profils
            .iter()
            .filter(|p| p.moteur.is_some())
            .count(),
        avis,
        avis_couverts,
        couverture_pct = pct(avis_couverts, avis),
        declares_seuls = recensement.declares_seuls,
        elapsed_s = debut.elapsed().as_secs(),
        "annonces_discover"
    );
    Ok(())
}

/// Counts per verdict and one probed URL per verdict, for one engine and kind.
type CompteVerdicts = (BTreeMap<&'static str, u64>, BTreeMap<&'static str, String>);

/// What the step gives the network. The weekly chain, which carries the daily
/// one plus its maintenance, has 300 s of margin under the cron period.
const BUDGET_SONDE: std::time::Duration = std::time::Duration::from_secs(120);

/// Draws a sample of the links the listings show, probes each one and writes
/// the verdict of the day. No escalation: a platform that rations the caller
/// gets its own verdict, which is not a dead link.
pub async fn annonces_sonder_liens(par_moteur: i64) -> Result<()> {
    let debut = std::time::Instant::now();
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let liens = repo
        .liens_a_sonder(JOURNAL_FOND_MARCHE, par_moteur)
        .await
        .map_err(|e| anyhow!("liens à sonder: {e}"))?;
    let tirs = annonces::sonde::sonder(&liens, BUDGET_SONDE).await;

    let mut par_cle: BTreeMap<(String, String), CompteVerdicts> = BTreeMap::new();
    for t in &tirs {
        let (compte, exemples) = par_cle
            .entry((t.moteur.clone(), t.kind.clone()))
            .or_default();
        *compte.entry(t.verdict.nom()).or_default() += 1;
        exemples.entry(t.verdict.nom()).or_insert(t.url.clone());
    }

    let sondes: Vec<SondeLiens> = par_cle
        .iter()
        .map(|((moteur, kind), (compte, exemples))| SondeLiens {
            moteur: moteur.clone(),
            kind: kind.clone(),
            verdicts: serde_json::json!(compte),
            exemples: serde_json::json!(exemples),
        })
        .collect();
    repo.poser_sonde_liens(&sondes)
        .await
        .map_err(|e| anyhow!("poser sonde: {e}"))?;

    for ((moteur, kind), (compte, exemples)) in &par_cle {
        let tires: u64 = compte.values().sum();
        let ouvrent: u64 = compte
            .iter()
            .filter(|(v, _)| matches!(**v, "fichier" | "formulaire" | "fiche"))
            .map(|(_, n)| *n)
            .sum();
        let ferment = compte.get("accueil").copied().unwrap_or_default()
            + compte.get("mort").copied().unwrap_or_default();
        if ferment > 0 && ouvrent == 0 {
            tracing::error!(
                moteur = moteur.as_str(),
                kind = kind.as_str(),
                tires,
                exemple = exemples
                    .get("accueil")
                    .or_else(|| exemples.get("mort"))
                    .map(String::as_str),
                "annonces_sonde_lien_ferme"
            );
        }
        tracing::info!(
            moteur = moteur.as_str(),
            kind = kind.as_str(),
            tires,
            ouvrent,
            ferment,
            rationne = compte.get("rationne").copied().unwrap_or_default(),
            indecidable = compte.get("indecidable").copied().unwrap_or_default(),
            "annonces_sonde_lien"
        );
    }
    tracing::info!(
        liens = tirs.len(),
        moteurs = par_cle.len(),
        elapsed_s = debut.elapsed().as_secs(),
        "annonces_sonder_liens"
    );
    Ok(())
}

#[cfg(test)]
mod tests {

    use super::*;
    use lj_store::repository::{AnnonceAJumeler, AnnonceOrpheline};

    /// Three live portals, one host already muet, one down for the first time.
    #[test]
    fn l_hote_deja_muet_sort_du_compte_des_echecs_celui_qui_tombe_y_entre() {
        let verdict = |host: &str, joignable| HoteVerdict {
            host: host.into(),
            joignable,
        };
        let hotes = [
            verdict("portail-a.safetender.com", true),
            verdict("portail-b.safetender.com", true),
            verdict("portail-c.safetender.com", true),
            verdict("portail-d.safetender.com", false),
            verdict("portail-e.safetender.com", false),
        ];
        let muets = HashSet::from(["portail-d.safetender.com".to_string()]);
        assert_eq!(echecs_et_muets(&hotes, &muets), (1, 1));

        // Le jour où le mort répond, il rentre au compte sans rien y ajouter.
        let ressuscite = [verdict("portail-d.safetender.com", true)];
        assert_eq!(echecs_et_muets(&ressuscite, &muets), (0, 0));
    }

    /// A fleet mostly down degrades the engine even when the portals still
    /// standing serve all they announce.
    #[test]
    fn la_flotte_qui_tombe_degrade_meme_quand_les_survivants_servent_tout() {
        let brute = AnnonceBrute {
            contenu: ContenuAnnonce::default(),
            descripteurs: Vec::new(),
            moteur: "safetender",
            source_uid: "safetender:h:1".into(),
            cles_jointure: Vec::new(),
            reference: None,
            acheteur: None,
            siren: None,
            objet: "objet".into(),
            nature: None,
            cpv: None,
            zones: Vec::new(),
            date_publication: None,
            echeance: None,
            url_avis: None,
            url_rc: None,
            url_dce: None,
            url_depot: None,
            procedure: None,
        };
        let run = |collecte: usize, annonce: u64| MoteurRun {
            annonce: Some(annonce),
            echecs: 0,
            hotes: Vec::new(),
            rows: vec![brute.clone(); collecte],
        };

        let normale = run(100, 100);
        assert!(!degrade(&normale, 0, 10), "une nuit normale est saine");
        assert!(
            !degrade(&normale, 2, 10),
            "le socle du recensement pas encore muet ne dégrade pas"
        );
        assert!(
            degrade(&run(10, 10), 8, 10),
            "la flotte à terre dégrade, collecté et annoncé fussent-ils égaux"
        );
        assert!(
            !degrade(&run(100, 100), 0, 0),
            "un moteur à point d'entrée unique n'a pas de flotte à juger"
        );
    }

    fn a_jumeler(titre: &str, acteur: &str, echeance: Option<&str>) -> AnnonceAJumeler {
        AnnonceAJumeler {
            uid: "marche:b".into(),
            titre: titre.into(),
            acteur_libelle: acteur.into(),
            echeance: echeance.map(|e| e.parse().expect("timestamp valide")),
            zones: Vec::new(),
            depot: "https://www.marches-securises.fr".into(),
            urls: Vec::new(),
            acheteur_uid: None,
            reference: None,
        }
    }

    /// La même candidate, rattachée à une entité acheteur.
    fn a_jumeler_entite(
        titre: &str,
        acteur: &str,
        echeance: Option<&str>,
        entite: &str,
    ) -> AnnonceAJumeler {
        AnnonceAJumeler {
            acheteur_uid: Some(entite.into()),
            ..a_jumeler(titre, acteur, echeance)
        }
    }

    fn orpheline(titre: &str, acteur: &str, echeance: Option<&str>) -> AnnonceOrpheline {
        AnnonceOrpheline {
            uid: "marche:o".into(),
            titre: titre.into(),
            acteur_libelle: acteur.into(),
            echeance: echeance.map(|e| e.parse().expect("timestamp valide")),
            urls: Vec::new(),
        }
    }

    fn cible(uid: &str, titre: &str, acteur: &str, echeance: Option<&str>) -> AnnonceChamps {
        AnnonceChamps {
            uid: uid.into(),
            titre: titre.into(),
            acteur_libelle: acteur.into(),
            echeance: echeance.map(|e| e.parse().expect("timestamp valide")),
            zones: Vec::new(),
            acheteur_uid: None,
            reference: None,
        }
    }

    /// La référence de marché est la clé quand l'acheteur ne peut pas servir :
    /// sur une instance multi-locataire la plateforme nomme le groupe.
    #[test]
    fn la_reference_unique_apparie_malgre_deux_acheteurs_sans_rapport() {
        fn index<'a>(cibles: &[&'a AnnonceChamps]) -> HashMap<String, Vec<&'a AnnonceChamps>> {
            let mut m: HashMap<String, Vec<&'a AnnonceChamps>> = HashMap::new();
            for c in cibles {
                m.entry(lj_fold::fold(c.reference.as_deref().unwrap_or_default()))
                    .or_default()
                    .push(c);
            }
            m
        }
        let une = |n| HashMap::from([("2026-mapa14".to_string(), n)]);
        let ligne = AnnonceAJumeler {
            reference: Some("2026-MAPA14".into()),
            ..a_jumeler(
                "Fourniture d'équipements",
                "OFFICE FRANCAIS DE LA BIODIVERSITE",
                None,
            )
        };
        let plateforme = AnnonceChamps {
            reference: Some("2026-mapa14".into()),
            ..cible(
                "marche:p",
                "Le présent marché a pour objet",
                "Ministère de la Transition Écologique",
                None,
            )
        };
        let pair = |l: &AnnonceAJumeler, c: &[&AnnonceChamps], p| matches!(jumelle_par_reference(l, &index(c), &p, &HashMap::new(), None), Jumelage::Une(u) if u == "marche:p");
        assert!(pair(&ligne, &[&plateforme], une(1)));

        // Deux candidates portent la référence : elle ne départage plus rien.
        assert!(!pair(&ligne, &[&plateforme], une(2)));

        // Deux lignes plateforme la portent : ambiguë, aucune paire.
        let autre = AnnonceChamps {
            uid: "marche:q".into(),
            ..AnnonceChamps {
                reference: Some("2026-MAPA14".into()),
                ..cible("marche:q", "Autre", "Autre acheteur", None)
            }
        };
        assert!(matches!(
            jumelle_par_reference(
                &ligne,
                &index(&[&plateforme, &autre]),
                &une(1),
                &HashMap::new(),
                None
            ),
            Jumelage::Ambigue
        ));

        // Deux entités acheteur résolues et différentes refusent la paire.
        let ligne_e = AnnonceAJumeler {
            acheteur_uid: Some("siren:1".into()),
            ..ligne
        };
        let plateforme_e = AnnonceChamps {
            acheteur_uid: Some("siren:2".into()),
            ..plateforme
        };
        assert!(!pair(&ligne_e, &[&plateforme_e], une(1)));

        // Une « référence » égale au nom de l'acheteur est un champ mal extrait.
        let mal = AnnonceAJumeler {
            reference: Some("OFFICE FRANCAIS DE LA BIODIVERSITE".into()),
            ..a_jumeler("T", "OFFICE FRANCAIS DE LA BIODIVERSITE", None)
        };
        assert!(matches!(
            jumelle_par_reference(
                &mal,
                &index(&[&plateforme_e]),
                &une(1),
                &HashMap::new(),
                None
            ),
            Jumelage::Aucune
        ));
    }

    /// A reference of digits alone can be a serial number two unrelated buyers both
    /// carry. Without a resolved buyer on both sides it needs the deadline day to depart.
    #[test]
    fn une_reference_de_chiffres_seuls_ne_departage_pas_sans_corroboration() {
        let plateforme = AnnonceChamps {
            reference: Some("20260006".into()),
            ..cible(
                "marche:p",
                "Sortie internet",
                "Agglomération de Mayotte",
                Some("2026-10-09T10:00:00Z"),
            )
        };
        let index = HashMap::from([("20260006".to_string(), vec![&plateforme])]);
        let portees = HashMap::from([("20260006".to_string(), 1)]);
        let ligne = |echeance| AnnonceAJumeler {
            reference: Some("20260006".into()),
            ..a_jumeler("Assistance technique", "Agence du Var", echeance)
        };
        let joue =
            |l: &AnnonceAJumeler| jumelle_par_reference(l, &index, &portees, &HashMap::new(), None);
        assert!(matches!(
            joue(&ligne(Some("2026-10-02T10:00:00Z"))),
            Jumelage::Aucune
        ));
        assert!(matches!(joue(&ligne(None)), Jumelage::Aucune));
        assert!(
            matches!(joue(&ligne(Some("2026-10-09T14:00:00Z"))), Jumelage::Une(u) if u == "marche:p")
        );
    }

    #[test]
    fn une_paire_sans_titre_ni_echeance_ni_acheteur_ne_passe_pas_le_plancher() {
        let mayotte = cible(
            "marche:p",
            "Fourniture d'une sortie internet et de liens Trunk SIP",
            "Agglomération de Dembéni-Mamoudzou",
            Some("2026-10-09T10:00:00Z"),
        );
        let var = a_jumeler(
            "Missions d'assistance technique en assainissement collectif",
            "Agence technique du Var",
            Some("2026-10-02T10:00:00Z"),
        );
        assert!(!corrobore(&var, &mayotte));
        assert!(corrobore(
            &AnnonceAJumeler {
                echeance: mayotte.echeance,
                ..a_jumeler(&var.titre, &var.acteur_libelle, None)
            },
            &mayotte
        ));
        assert!(corrobore(
            &a_jumeler(&mayotte.titre, &var.acteur_libelle, None),
            &mayotte
        ));
        assert!(corrobore(
            &a_jumeler(&var.titre, &mayotte.acteur_libelle, None),
            &mayotte
        ));
    }

    /// Le titre et la date d'échéance font clé sans acheteur ni canal : c'est
    /// la voie des avis dont l'hôte déclaré n'a pas de moteur connecté, ou dont
    /// la plateforme nomme le groupe tenant. L'heure ne compte pas, le jour si ;
    /// département et entité récusent quand les deux côtés les portent.
    #[test]
    fn le_titre_et_la_date_d_echeance_apparient_sans_acheteur_ni_canal() {
        fn index<'a>(cibles: &[&'a AnnonceChamps]) -> HashMap<String, Vec<&'a AnnonceChamps>> {
            let mut m: HashMap<String, Vec<&'a AnnonceChamps>> = HashMap::new();
            for c in cibles {
                m.entry(lj_fold::fold(&c.titre)).or_default().push(c);
            }
            m
        }
        let ligne = a_jumeler(
            "Fourniture de pièces détachées",
            "SDIS du Gard",
            Some("2026-09-08T14:00:00Z"),
        );
        // Heures divergentes, même jour de Paris : la paire tient.
        let plateforme = cible(
            "marche:p",
            "FOURNITURE DE PIECES DETACHEES",
            "SDIS",
            Some("2026-09-08T10:00:00Z"),
        );
        assert!(matches!(
            jumelle_par_titre_echeance(&ligne, &index(&[&plateforme])),
            Jumelage::Une("marche:p")
        ));

        // Un jour d'écart refuse ; une échéance absente d'un côté aussi.
        let veille = cible("marche:v", &ligne.titre, "", Some("2026-09-07T14:00:00Z"));
        assert!(matches!(
            jumelle_par_titre_echeance(&ligne, &index(&[&veille])),
            Jumelage::Aucune
        ));
        let sans = cible("marche:s", &ligne.titre, "", None);
        assert!(matches!(
            jumelle_par_titre_echeance(&ligne, &index(&[&sans])),
            Jumelage::Aucune
        ));

        // Deux prétendantes au même jour : le département départage, et deux
        // départements résolus et différents récusent.
        let gard = AnnonceChamps {
            zones: vec!["30".into()],
            ..cible("marche:g", &ligne.titre, "", Some("2026-09-08T10:00:00Z"))
        };
        let aude = AnnonceChamps {
            zones: vec!["11".into()],
            ..cible("marche:a", &ligne.titre, "", Some("2026-09-08T10:00:00Z"))
        };
        let ligne_gard = AnnonceAJumeler {
            zones: vec!["30".into()],
            ..a_jumeler(&ligne.titre, "", Some("2026-09-08T14:00:00Z"))
        };
        assert!(matches!(
            jumelle_par_titre_echeance(&ligne_gard, &index(&[&gard, &aude])),
            Jumelage::Une("marche:g")
        ));
        assert!(matches!(
            jumelle_par_titre_echeance(&ligne_gard, &index(&[&aude])),
            Jumelage::Aucune
        ));
        assert!(matches!(
            jumelle_par_titre_echeance(&ligne, &index(&[&gard, &aude])),
            Jumelage::Ambigue
        ));
    }

    /// L'avis déclare l'hôte de façade, le stock vit sous l'hôte de service :
    /// le canal se résout vers l'unique sous-domaine peuplé, et deux
    /// sous-domaines ne se départagent pas.
    #[test]
    fn le_canal_declare_se_resout_vers_l_hote_de_service_du_stock() {
        let vivier: HashMap<String, ()> = [
            ("atexo:plateforme.alsacemarchespublics.eu".to_string(), ()),
            ("atexo:marches.megalis.bretagne.bzh".to_string(), ()),
            ("safetender:marches.megalis.bretagne.bzh".to_string(), ()),
            ("atexo:marchespublics.eure.fr".to_string(), ()),
        ]
        .into();
        let r = |c: &str| canal_du_stock(c.to_string(), &vivier);
        assert_eq!(
            r("atexo:alsacemarchespublics.eu"),
            "atexo:plateforme.alsacemarchespublics.eu"
        );
        assert_eq!(
            r("atexo:megalis.bretagne.bzh"),
            "atexo:marches.megalis.bretagne.bzh",
            "le moteur borne la résolution, safetender ne compte pas"
        );
        assert_eq!(
            r("atexo:marchespublics.eure.fr"),
            "atexo:marchespublics.eure.fr"
        );
        assert_eq!(r("atexo:inconnu.fr"), "atexo:inconnu.fr");
        let deux: HashMap<String, ()> = [
            ("atexo:a.exemple.fr".to_string(), ()),
            ("atexo:b.exemple.fr".to_string(), ()),
        ]
        .into();
        assert_eq!(
            canal_du_stock("atexo:exemple.fr".into(), &deux),
            "atexo:exemple.fr"
        );
    }

    /// The deposit declares the front door, the connector questions the serving
    /// instance: `megalis.bretagne.bzh` resolves to `marches.megalis.bretagne.bzh`.
    /// Two sub-domains settle nothing.
    #[test]
    fn the_declared_host_resolves_to_the_instance_a_connector_questions() {
        let moteurs: HashMap<String, String> = [
            ("megalis.bretagne.bzh", "atexo"),
            ("marches.megalis.bretagne.bzh", "atexo"),
            ("sso.megalis.bretagne.bzh", "safetender"),
            ("marchespublics.eure.fr", "atexo"),
            ("exemple.fr", "atexo"),
            ("a.exemple.fr", "atexo"),
            ("b.exemple.fr", "atexo"),
        ]
        .iter()
        .map(|(h, m)| ((*h).to_string(), (*m).to_string()))
        .collect();
        let r = |depot: &str| hote_et_moteur(depot, &moteurs);
        assert_eq!(
            r("https://megalis.bretagne.bzh/"),
            Some((
                "marches.megalis.bretagne.bzh".to_string(),
                "atexo".to_string()
            )),
            "the engine bounds the resolution, safetender does not count"
        );
        assert_eq!(
            r("https://marchespublics.eure.fr/"),
            Some(("marchespublics.eure.fr".to_string(), "atexo".to_string()))
        );
        assert_eq!(
            r("https://exemple.fr/"),
            Some(("exemple.fr".to_string(), "atexo".to_string())),
            "two instances leave the front door alone"
        );
    }

    /// La même cible, rattachée à une entité acheteur.
    fn cible_entite(
        uid: &str,
        titre: &str,
        acteur: &str,
        echeance: Option<&str>,
        entite: &str,
    ) -> AnnonceChamps {
        AnnonceChamps {
            acheteur_uid: Some(entite.into()),
            ..cible(uid, titre, acteur, echeance)
        }
    }

    #[test]
    fn la_chaine_des_parents_remonte_au_marche_reel() {
        let parent_de: HashMap<&str, &str> = [("26-2", "26-1"), ("26-3", "26-2")].into();
        assert_eq!(
            remonte_parent(&parent_de, Some("26-2")),
            Some("26-1".into())
        );
        assert_eq!(
            remonte_parent(&parent_de, Some("26-3")),
            Some("26-1".into())
        );
        assert_eq!(
            remonte_parent(&parent_de, Some("26-9")),
            Some("26-9".into())
        );
        assert_eq!(remonte_parent(&parent_de, None), None);
        let cycle: HashMap<&str, &str> = [("a", "b"), ("b", "a")].into();
        assert_eq!(remonte_parent(&cycle, Some("b")), Some("a".into()));
    }

    #[test]
    fn le_bloc_detail_ne_porte_que_ce_que_la_page_a_donne() {
        // `vu` stamps every visit and dates it, so it is read apart from the
        // fields the page gave.
        let lu = |d: &annonces::DetailAnnonce| {
            let mut bloc = bloc_detail(d);
            let vu = bloc
                .as_object_mut()
                .expect("objet")
                .remove("vu")
                .expect("la visite est estampillée");
            assert_eq!(vu.as_str().map(str::len), Some(10), "date ISO : {vu}");
            bloc
        };
        assert_eq!(
            lu(&annonces::DetailAnnonce::default()),
            serde_json::json!({})
        );
        assert_eq!(
            lu(&annonces::DetailAnnonce {
                contenu: ContenuAnnonce::default(),
                zones: Vec::new(),
                cpv: Some("45233140-2".into()),
                procedure: None,
                acheteur: None,
            }),
            serde_json::json!({"cpv": "45233140-2"})
        );
        assert_eq!(
            lu(&annonces::DetailAnnonce {
                contenu: ContenuAnnonce::default(),
                zones: Vec::new(),
                cpv: Some("45233140-2".into()),
                procedure: Some("Procédure adaptée".into()),
                acheteur: None,
            }),
            serde_json::json!({"cpv": "45233140-2", "procedure": "Procédure adaptée"})
        );
    }

    #[test]
    fn le_filet_donne_aux_lots_la_meme_forme_que_l_ecriture() {
        let mut resultat = serde_json::json!({
            "lots": [{
                "titulaire": "OLYMPIQUE MARQUAGE\n1001 avenue de la Batterie",
                "description": "FOURNITURE DE CHAUDIERES BOIS DECHIQUETE ET SES TRAVAUX ASSOCIES",
                "montant": 144235.5,
            }],
            "texte": "Attribué.\n\nVoies de recours :\nTribunal administratif de Nice.",
        });
        assert!(embellir_resultat(&mut resultat));
        let lot = &resultat["lots"][0];
        assert_eq!(lot["titulaire"], "OLYMPIQUE MARQUAGE");
        assert_eq!(
            lot["description"],
            "Fourniture de chaudieres bois dechiquete et ses travaux associes"
        );
        assert!(resultat["texte"].as_str().unwrap().contains('\n'));
        assert!(!embellir_resultat(&mut resultat));
    }

    #[test]
    fn une_seule_cible_gagne_sans_condition() {
        let o = orpheline("Consultation_TEST", "X", None);
        let c = cible("marche:a", "Tout autre titre", "Y", None);
        assert!(matches!(
            cible_de_regularisation(&o, &[&c]),
            Resolution::Unique("marche:a")
        ));
    }

    #[test]
    fn la_jumelle_se_reconnait_a_la_casse_et_aux_espaces_pres() {
        let o = orpheline(
            "Consultation_TEST_Sans_Signature",
            "Plateforme  X",
            Some("2050-12-31T00:00:00Z"),
        );
        let a = cible(
            "marche:a",
            "Consultation_Test_Sans_Signature",
            "plateforme x",
            Some("2050-12-31T00:00:00Z"),
        );
        let b = cible(
            "marche:b",
            "Une autre consultation",
            "plateforme x",
            Some("2050-12-31T00:00:00Z"),
        );
        assert!(matches!(
            cible_de_regularisation(&o, &[&a, &b]),
            Resolution::Jumelle("marche:a")
        ));
    }

    #[test]
    fn l_echeance_departage_deux_jumelles_de_titre() {
        let o = orpheline(
            "Fourniture de matériel",
            "Ville",
            Some("2026-09-04T12:00:00Z"),
        );
        let a = cible(
            "marche:a",
            "Fourniture de matériel",
            "Ville",
            Some("2026-09-09T12:00:00Z"),
        );
        let b = cible(
            "marche:b",
            "Fourniture de matériel",
            "Ville",
            Some("2026-09-04T12:00:00Z"),
        );
        assert!(matches!(
            cible_de_regularisation(&o, &[&a, &b]),
            Resolution::Jumelle("marche:b")
        ));
    }

    #[test]
    fn des_jumelles_strictes_indiscernables_departagent_par_uid() {
        let o = orpheline(
            "Prestations de plonge",
            "CCAS",
            Some("2026-08-20T12:00:00Z"),
        );
        let a = cible(
            "marche:b",
            "Prestations de plonge",
            "CCAS",
            Some("2026-08-20T12:00:00Z"),
        );
        let b = cible(
            "marche:a",
            "Prestations de plonge",
            "CCAS",
            Some("2026-08-20T12:00:00Z"),
        );
        assert!(matches!(
            cible_de_regularisation(&o, &[&a, &b]),
            Resolution::Indiscernables("marche:a")
        ));
    }

    #[test]
    fn des_jumelles_indiscernables_entre_elles_resolvent_malgre_l_echeance_d_une_autre_generation()
    {
        let o = orpheline(
            "Fourniture de contenants à déchets",
            "E.N.A.C.",
            Some("2026-08-07T16:00:00Z"),
        );
        let a = cible(
            "marche:b",
            "Fourniture de contenants à déchets",
            "",
            Some("2026-08-07T14:00:00Z"),
        );
        let b = cible(
            "marche:a",
            "Fourniture de contenants à déchets",
            "",
            Some("2026-08-07T14:00:00Z"),
        );
        assert!(matches!(
            cible_de_regularisation(&o, &[&a, &b]),
            Resolution::Indiscernables("marche:a")
        ));
    }

    #[test]
    fn des_jumelles_discernables_a_l_echeance_inconnue_ne_resolvent_pas() {
        let o = orpheline("Travaux de voirie", "Ville", Some("2026-08-01T12:00:00Z"));
        let a = cible(
            "marche:a",
            "Travaux de voirie",
            "Ville",
            Some("2026-08-10T12:00:00Z"),
        );
        let b = cible(
            "marche:b",
            "Travaux de voirie",
            "Ville",
            Some("2026-08-20T12:00:00Z"),
        );
        assert!(matches!(
            cible_de_regularisation(&o, &[&a, &b]),
            Resolution::Aucune
        ));
    }

    #[test]
    fn sans_jumelle_on_ne_devine_pas() {
        let o = orpheline(
            "Dépôt de Munitions de NEUBOURG",
            "ESID",
            Some("2026-08-05T12:00:00Z"),
        );
        let a = cible(
            "marche:a",
            "Groupement de Munitions de NEUBOURG",
            "ESID",
            Some("2026-07-27T12:00:00Z"),
        );
        let b = cible("marche:b", "Autre chose", "ESID", None);
        assert!(matches!(
            cible_de_regularisation(&o, &[&a, &b]),
            Resolution::Aucune
        ));
    }

    // ── Jumelage des dépôts sans clé ────────────────────────────────────────

    #[test]
    fn la_jumelle_d_un_depot_se_reconnait_a_la_typographie_pres() {
        let ligne = a_jumeler(
            "Accord-cadre pour l’organisation du Festival",
            "Office de Tourisme de Saint Martin",
            Some("2026-09-04T12:00:00Z"),
        );
        let a = cible(
            "marche:p",
            "Accord-cadre pour l'organisation du Festival",
            "OFFICE DE TOURISME DE SAINT MARTIN",
            Some("2026-09-04T12:00:00Z"),
        );
        let b = cible(
            "marche:q",
            "Marché d'assurances",
            "Office de Tourisme de Saint Martin",
            Some("2026-09-04T12:00:00Z"),
        );
        assert!(matches!(
            jumelle_de_depot(&ligne, &[&a, &b]),
            Jumelage::Une("marche:p")
        ));
    }

    /// Two sibling lots of one buyer, one word apart, stay apart below the
    /// threshold.
    #[test]
    fn deux_lots_freres_ne_se_confondent_pas_sous_le_seuil() {
        let ligne = a_jumeler_entite(
            "L'entretien et la réparation des véhicules de PTAC inférieur à 4,5 Tonnes pour le SDIS de la Mayenne",
            "SDIS",
            Some("2026-09-04T12:00:00Z"),
            "siren:215903923",
        );
        let frere = AnnonceChamps {
            acheteur_uid: Some("siren:215903923".into()),
            ..cible(
                "marche:p",
                "Entretien et réparation des véhicules de ptac supérieur ou égal à 4,5 t",
                "SDIS",
                Some("2026-09-04T12:00:00Z"),
            )
        };
        let mut index = HashMap::new();
        index.insert("siren:215903923", vec![&frere]);
        assert!(matches!(
            jumelle_par_acheteur(&ligne, &index, &HashMap::new()),
            Jumelage::Aucune
        ));
    }

    /// Both spellings come from the fund: the buyer prefixes its title with its
    /// internal reference on the platform and not at the BOAMP.
    #[test]
    fn le_titre_approchant_rattrape_un_prefixe_de_reference() {
        let ligne = a_jumeler(
            "PA 2026-15 : Maintenance préventive des ascenseurs",
            "Ville",
            Some("2026-09-04T12:00:00Z"),
        );
        let a = cible(
            "marche:p",
            "Maintenance préventive des ascenseurs",
            "Ville",
            Some("2026-10-02T12:00:00Z"),
        );
        let b = cible("marche:q", "Entretien des espaces verts", "Ville", None);
        assert!(matches!(
            jumelle_par_titre_proche(&ligne, &[&a, &b]),
            Jumelage::Une("marche:p")
        ));
        // Two pretenders above the threshold are not separated: on this
        // fallback the deadline is worth nothing, being what failed higher up.
        let c = cible(
            "marche:r",
            "PA 2026-15 : Maintenance préventive des ascenseurs, lot 2",
            "Ville",
            Some("2026-09-04T12:00:00Z"),
        );
        assert!(matches!(
            jumelle_par_titre_proche(&ligne, &[&a, &c]),
            Jumelage::Ambigue
        ));
    }

    /// Two resolved and different buyer entities refuse the pair even on
    /// identical titles: the same object goes to tender in two nearby towns.
    #[test]
    fn le_titre_approchant_recuse_une_autre_entite_acheteur() {
        let ligne = a_jumeler_entite(
            "Fourniture et livraison de repas en liaison froide",
            "Commune A",
            Some("2026-09-04T12:00:00Z"),
            "siren:215903923",
        );
        let a = AnnonceChamps {
            acheteur_uid: Some("siren:213100555".into()),
            ..cible(
                "marche:p",
                "Fourniture et livraison de repas en liaison froide",
                "Commune B",
                Some("2026-09-04T12:00:00Z"),
            )
        };
        assert!(matches!(
            jumelle_par_titre_proche(&ligne, &[&a]),
            Jumelage::Aucune
        ));
    }

    #[test]
    fn l_echeance_departage_deux_jumelles_de_depot() {
        let ligne = a_jumeler(
            "Fourniture de matériel",
            "Ville",
            Some("2026-09-04T12:00:00Z"),
        );
        let a = cible(
            "marche:p",
            "Fourniture de matériel",
            "Ville",
            Some("2026-09-09T12:00:00Z"),
        );
        let b = cible(
            "marche:q",
            "Fourniture de matériel",
            "Ville",
            Some("2026-09-04T12:00:00Z"),
        );
        assert!(matches!(
            jumelle_de_depot(&ligne, &[&a, &b]),
            Jumelage::Une("marche:q")
        ));
    }

    #[test]
    fn deux_jumelles_indiscernables_ne_donnent_aucune_paire() {
        let ligne = a_jumeler(
            "Prestations de plonge",
            "CCAS",
            Some("2026-08-20T12:00:00Z"),
        );
        let a = cible(
            "marche:p",
            "Prestations de plonge",
            "CCAS",
            Some("2026-08-20T12:00:00Z"),
        );
        let b = cible(
            "marche:q",
            "Prestations de plonge",
            "CCAS",
            Some("2026-08-20T12:00:00Z"),
        );
        assert!(matches!(
            jumelle_de_depot(&ligne, &[&a, &b]),
            Jumelage::Ambigue
        ));
    }

    #[test]
    fn un_acheteur_different_refuse_la_paire() {
        let ligne = a_jumeler(
            "Prestations de services d'assurance",
            "CC Loire Layon Aubance",
            Some("2026-09-04T12:00:00Z"),
        );
        let a = cible(
            "marche:p",
            "Prestations de services d'Assurance",
            "MAIRIE DE ROBION",
            Some("2026-09-04T12:00:00Z"),
        );
        assert!(matches!(jumelle_de_depot(&ligne, &[&a]), Jumelage::Aucune));
    }

    #[test]
    fn un_acheteur_absent_de_la_plateforme_ne_refute_rien() {
        let ligne = a_jumeler("Travaux de voirie", "Ville de Lille", None);
        let a = cible("marche:p", "Travaux de voirie", "", None);
        assert!(matches!(
            jumelle_de_depot(&ligne, &[&a]),
            Jumelage::Une("marche:p")
        ));
    }

    #[test]
    fn deux_graphies_d_un_meme_siren_font_le_meme_acheteur() {
        let ligne = a_jumeler_entite(
            "Fourniture de repas",
            "Ville de Maubeuge",
            Some("2026-09-04T12:00:00Z"),
            "siren:215903923",
        );
        let a = cible_entite(
            "marche:p",
            "Fourniture de repas",
            "COMMUNE DE MAUBEUGE",
            Some("2026-09-04T12:00:00Z"),
            "siren:215903923",
        );
        assert!(matches!(
            jumelle_de_depot(&ligne, &[&a]),
            Jumelage::Une("marche:p")
        ));
    }

    #[test]
    fn deux_sirens_distincts_refusent_la_paire() {
        let ligne = a_jumeler_entite(
            "Prestations de services d'assurances",
            "BRESTPORT",
            Some("2026-09-04T12:00:00Z"),
            "siren:892370974",
        );
        let a = cible_entite(
            "marche:p",
            "Prestations de services d'assurances",
            "Commune de Callac",
            Some("2026-09-04T12:00:00Z"),
            "siren:212200257",
        );
        assert!(matches!(jumelle_de_depot(&ligne, &[&a]), Jumelage::Aucune));
    }

    #[test]
    fn une_entite_d_un_seul_cote_ne_prouve_rien() {
        let ligne = a_jumeler_entite(
            "Travaux de voirie",
            "Cté Cnes du Grand Pic St Loup",
            None,
            "siren:243400694",
        );
        let a = cible(
            "marche:p",
            "Travaux de voirie",
            "CC du Grand Pic Saint Loup",
            None,
        );
        assert!(matches!(jumelle_de_depot(&ligne, &[&a]), Jumelage::Aucune));
    }

    #[test]
    fn sans_jumelle_aucune_paire() {
        let ligne = a_jumeler("Travaux de voirie", "Ville de Lille", None);
        let a = cible("marche:p", "Travaux de toiture", "Ville de Lille", None);
        assert!(matches!(jumelle_de_depot(&ligne, &[&a]), Jumelage::Aucune));
        assert!(matches!(jumelle_de_depot(&ligne, &[]), Jumelage::Aucune));
    }

    #[test]
    fn le_canal_cible_est_l_instance_pour_un_multi_tenant() {
        assert_eq!(
            canal_de_depot("atexo", "marches.megalis.bretagne.bzh"),
            "atexo:marches.megalis.bretagne.bzh"
        );
        assert_eq!(
            canal_de_source("atexo:marches.megalis.bretagne.bzh:489"),
            "atexo:marches.megalis.bretagne.bzh"
        );
        assert_eq!(canal_de_depot("atline", "marches-securises.fr"), "atline");
        assert_eq!(canal_de_source("aws:pub:MPI-pub-2026219817"), "aws");
        assert_eq!(canal_de_source("atline:1538690665o1tv5rpoutni"), "atline");
    }

    #[test]
    fn l_hote_d_un_depot_est_canonique() {
        assert_eq!(
            hote_de("https://www.marches-securises.fr/"),
            Some("marches-securises.fr".into())
        );
        assert_eq!(
            hote_de("https://marches.maximilien.fr/entreprise"),
            Some("marches.maximilien.fr".into())
        );
        assert_eq!(
            hote_de("https://marches.local-trust.com/?page=Frame.ConsultationsOrganisme"),
            Some("marches.local-trust.com".into())
        );
        assert_eq!(hote_de("www.marches-securises.fr"), None);
    }
}
