use std::collections::HashSet;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use clap::ValueEnum;
use futures::FutureExt;
use lj_store::repository::DecisionRepository;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use tracing::Instrument;

use crate::config::Settings;
use crate::{pipeline, AssertionsCommand, Command, DbCommand};

/// Plancher du délai d'une étape. En dessous, une journée simplement lente
/// (amont chargé, file plus grosse que d'habitude) se ferait couper.
const PLANCHER_SECS: u64 = 900;
/// A fetch took 0.1 to 0.4 s on 15/11/2026, its pace 0.5 s: 300 hold within the floor watchdog.
const CE_CONCLUSIONS_PER_RUN: usize = 300;

/// Une étape de chaîne et son délai de watchdog.
struct Step {
    command: Command,
    watchdog_secs: u64,
}

/// What a summary step leaves its calls in flight before its watchdog: one luna call's timeout.
const SUMMARY_DRAIN_SECS: u64 = 600;

/// Étape et son délai, calé sur ~3× le maximum observé au cron et jamais sous
/// [`PLANCHER_SECS`] : le watchdog vise l'amont pendu, pas la journée lente.
/// Une étape coupée n'arrête pas la chaîne et repart au run suivant, donc un
/// délai serré coûte un run d'étape, quand un délai large coûte la nuit.
fn step(command: Command, watchdog_secs: u64) -> Step {
    Step {
        command,
        watchdog_secs,
    }
}

/// Branches running at once.
const BRANCHES_AT_ONCE: usize = 4;
/// The main thread's stack, which every step ran on: a pool thread's 2 MiB is too small for some.
const BRANCH_STACK: usize = 8 << 20;
const LOCK_POLL: Duration = Duration::from_secs(60);

/// Steps run in order, once the branches named in `after` have ended.
struct Branch {
    name: &'static str,
    after: &'static [&'static str],
    steps: Vec<Step>,
    /// Runs no step on the weekly night, whose turn takes its time; the next day's run catches
    /// up what it left.
    daily_only: bool,
}

fn branch(name: &'static str, after: &'static [&'static str], steps: Vec<Step>) -> Branch {
    Branch {
        name,
        after,
        steps,
        daily_only: false,
    }
}

fn daily_only_branch(
    name: &'static str,
    after: &'static [&'static str],
    steps: Vec<Step>,
) -> Branch {
    Branch {
        daily_only: true,
        ..branch(name, after, steps)
    }
}

fn gazette(source: lj_gazette::Source) -> Command {
    Command::Gazette {
        source,
        stock: false,
        offline: false,
        dry_run: false,
        verify: false,
        reclass: false,
        recount: false,
    }
}

fn norms(country: pipeline::NormsCountry, minutes: Option<u64>) -> Command {
    Command::SyncNorms {
        country,
        scan: pipeline::NormsScan::Daily,
        only: None,
        limit: None,
        fresh: false,
        minutes,
        dry_run: false,
    }
}

/// Scheduled chain. `Weekly` = the daily one followed by the weekly
/// maintenance, in a single process.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum ChainKind {
    /// The day's sources, freshness, summaries, publication.
    Daily,
    /// The daily one, then the weekly maintenance.
    Weekly,
    /// Before the quotas reset: what the day has not spent.
    Drain,
}

impl ChainKind {
    /// Stable label for the breadcrumbs and the alert (`chain="daily"`).
    fn label(self) -> &'static str {
        match self {
            ChainKind::Daily => "daily",
            ChainKind::Weekly => "weekly",
            ChainKind::Drain => "drain",
        }
    }
}

/// The branches writing decisions, which the summaries wait for.
const DECISION_SOURCES: [&str; 21] = [
    "judilibre",
    "dila",
    "cedh",
    "cjue",
    "cnda",
    "ddd",
    "juricaf",
    "entscheidsuche",
    "legimonaco",
    "juportal",
    "rechtspraak",
    "justice-lu",
    "cce",
    "scc-csc",
    "legalis",
    "amf",
    "juradm-site",
    "cc-site",
    "consiliaweb",
    "adlc",
    "comeu",
];

/// Sources du jour + publication. Aucune étape ne dépend du **succès** de la
/// précédente : l'ordre porte une fraîcheur (les résumés couvrent ce que
/// l'ingest vient d'écrire, IndexNow soumet ce que `generate-summaries` vient
/// de bumper), pas une précondition.
///
/// Listed in the order a free slot takes them: the decision sources the summaries wait for come
/// first, the longest of them at the head.
fn daily() -> Vec<Branch> {
    vec![
        branch(
            "judilibre",
            &[],
            vec![
                step(Command::Sync, 1_200),
                step(
                    Command::Ingest {
                        mode: pipeline::IngestMode::MissingHash,
                    },
                    1_800,
                ),
                // A week of margin: a missed day still sees the pairs it left.
                step(Command::AbsorbCopies { days: 7 }, PLANCHER_SECS),
            ],
        ),
        branch(
            "dila",
            &[],
            vec![
                step(
                    Command::SyncDila {
                        fonds: vec![pipeline::Fond::Jade],
                    },
                    PLANCHER_SECS,
                ),
                step(
                    Command::SyncDila {
                        fonds: vec![pipeline::Fond::Constit],
                    },
                    PLANCHER_SECS,
                ),
                step(
                    Command::SyncDila {
                        fonds: vec![pipeline::Fond::Cnil],
                    },
                    PLANCHER_SECS,
                ),
                // One watchdog for both judicial funds: their weekly increments take
                // seconds, and INCA skips what CASS has just written.
                step(
                    Command::SyncDila {
                        fonds: vec![pipeline::Fond::Cass, pipeline::Fond::Inca],
                    },
                    PLANCHER_SECS,
                ),
                // After JADE, whose new CE decisions the census compares with the three ArianeWeb fonds:
                // three requests when nothing moved, a year of pages when a year moved.
                step(
                    Command::SyncAriane {
                        years: Vec::new(),
                        census_only: false,
                        limit: None,
                    },
                    PLANCHER_SECS,
                ),
                // The conclusions the bundles just named; the stock before the step runs by hand.
                step(
                    Command::SyncCeConclusions {
                        limit: Some(CE_CONCLUSIONS_PER_RUN),
                    },
                    PLANCHER_SECS,
                ),
            ],
        ),
        branch(
            "cedh",
            &[],
            vec![step(Command::SyncCedh { stubs: false }, PLANCHER_SECS)],
        ),
        // The procurement fonds, written by this step alone: channels, life cycle, new notices, awards.
        branch(
            "annonces",
            &[],
            vec![step(
                Command::Annonces {
                    centrale_jours: lj_sources_annonces::centraledesmarches::FENETRE_JOURS,
                    relancer_muets: false,
                },
                // The internal budgets bound the phases, not this.
                4_500,
            )],
        ),
        // The texts come from DILA's host too, which serves one branch at a time.
        branch(
            "dila-texts",
            &["dila"],
            vec![
                step(Command::SyncLegi, PLANCHER_SECS),
                step(Command::SyncKali, PLANCHER_SECS),
                step(Command::SyncJorf, PLANCHER_SECS),
                step(Command::TreatyParties, PLANCHER_SECS),
                step(gazette(lj_gazette::Source::Bodacc), PLANCHER_SECS),
                step(gazette(lj_gazette::Source::Joafe), PLANCHER_SECS),
                step(Command::SyncCirculaires, PLANCHER_SECS),
                step(
                    Command::SyncCirculairesBodies { ocr_tenant: None },
                    PLANCHER_SECS,
                ),
                step(Command::StampFreshness, PLANCHER_SECS),
            ],
        ),
        // No batch of 100 pages starts past 1 200 s.
        branch(
            "juricaf",
            &[],
            vec![step(
                Command::SyncJuricaf {
                    countries: Vec::new(),
                    full: false,
                    list: false,
                },
                2_400,
            )],
        ),
        // No page starts past 1 200 s.
        branch(
            "entscheidsuche",
            &[],
            vec![step(
                Command::SyncEntscheidsuche {
                    full: false,
                    limit: None,
                    courts: Vec::new(),
                    missing: false,
                    rewrite: false,
                },
                2_400,
            )],
        ),
        // No page starts past 1 200 s.
        branch(
            "legimonaco",
            &[],
            vec![step(
                Command::SyncLegimonaco {
                    full: false,
                    limit: None,
                    dry_run: false,
                },
                2_400,
            )],
        ),
        // One feed, then a page per new decision 2 s apart; no batch of 100 starts past 1 200 s.
        branch(
            "juportal",
            &[],
            vec![step(
                Command::SyncJuportal {
                    full: false,
                    limit: None,
                    dry_run: false,
                },
                2_400,
            )],
        ),
        // The documents modified since the last run, 1 s apart, about 450 a day; no batch of 100
        // starts past 600 s, the rest left to the next night.
        branch(
            "rechtspraak",
            &[],
            vec![step(
                Command::SyncRechtspraak {
                    full: false,
                    month: None,
                    limit: None,
                    dry_run: false,
                },
                PLANCHER_SECS,
            )],
        ),
        // No archive starts past 1 200 s.
        branch(
            "justice-lu",
            &[],
            vec![step(
                Command::SyncJusticeLu {
                    full: false,
                    datasets: Vec::new(),
                    list: false,
                },
                2_400,
            )],
        ),
        // One request every 10 s, none past 1 200 s: the counts, the months whose count moved, then
        // each new PDF.
        branch(
            "cce",
            &[],
            vec![step(
                Command::SyncCce {
                    full: false,
                    limit: None,
                    dry_run: false,
                    only: Vec::new(),
                },
                1_800,
            )],
        ),
        // One request every 30 s, none past 600 s: the newest listing page, then each new judgment.
        branch(
            "scc-csc",
            &[],
            vec![step(
                Command::SyncSccCsc {
                    full: false,
                    limit: None,
                    dry_run: false,
                },
                PLANCHER_SECS,
            )],
        ),
        // 3 requests a run, then one per page new or modified, 3 s apart, none past 600 s.
        branch(
            "legalis",
            &[],
            vec![step(
                Command::SyncLegalis {
                    full: false,
                    limit: None,
                    dry_run: false,
                },
                PLANCHER_SECS,
            )],
        ),
        // 3 requests per list, then 2 per new notice, 30 s apart: past 12 new ones,
        // the next day carries on from the cache.
        branch(
            "ddd",
            &[],
            vec![step(Command::SyncDdd { full: false }, PLANCHER_SECS)],
        ),
        branch(
            "amf",
            &[],
            vec![
                step(Command::SyncAmf { full: false }, PLANCHER_SECS),
                step(Command::ExpireNamed, PLANCHER_SECS),
            ],
        ),
        // About 180 listing pages a second apart, then two requests per new item.
        branch(
            "juradm-site",
            &[],
            vec![step(
                Command::SyncJuradmSite {
                    full: false,
                    courts: Vec::new(),
                    minutes: Some(10),
                },
                PLANCHER_SECS,
            )],
        ),
        branch(
            "cc-site",
            &[],
            vec![step(Command::SyncCcSite, PLANCHER_SECS)],
        ),
        branch(
            "consiliaweb",
            &[],
            vec![step(
                Command::SyncConsiliaweb {
                    full: false,
                    from_base: false,
                },
                PLANCHER_SECS,
            )],
        ),
        branch("cjue", &[], vec![step(Command::SyncCjue, PLANCHER_SECS)]),
        branch("cnda", &[], vec![step(Command::SyncCnda, PLANCHER_SECS)]),
        // One request on the days the file holds still, 601 s at most to 20/11/2026 on a new one.
        branch(
            "adlc",
            &[],
            vec![step(
                Command::SyncAdlc {
                    mode: pipeline::IngestMode::MissingHash,
                },
                1_800,
            )],
        ),
        // The week's documents, 1,031 s at most over 12 days to 20/11/2026.
        branch(
            "comeu",
            &[],
            vec![step(Command::SyncComeu { full: false }, 3_100)],
        ),
        // A run cut at its watchdog keeps the datasets it wrote, loaded by the next one: 604 s at
        // most over 10 nights to 19/11/2026.
        branch(
            "eu-law",
            &[],
            vec![
                step(
                    Command::SyncEuLaw {
                        limit: None,
                        only: None,
                        force: false,
                        backlog: false,
                        concurrency: 4,
                    },
                    PLANCHER_SECS,
                ),
                step(
                    Command::EuBodies {
                        limit: Some(500),
                        refresh: None,
                        dry_run: false,
                    },
                    PLANCHER_SECS,
                ),
            ],
        ),
        // The cantons' and Belgium's budgets hold their relist, one request per act or document;
        // the Swiss federal one, a week's changes; Luxembourg's, the states whose file moved.
        // Their first reads, about 3, 2, 5 and 17 h, are run by hand, as is Canada's, 18 h,
        // before its step joins the chain.
        branch(
            "norms",
            &[],
            vec![
                // Reads acts never visited, and RO acts never read, for 15 minutes at most.
                step(norms(pipeline::NormsCountry::Ch, Some(15)), 1_200),
                // Reads no act never visited while the first read, by hand, is under way.
                step(norms(pipeline::NormsCountry::Lu, Some(0)), 1_200),
                // Reads acts never visited for an hour at most, a first read carried over to
                // the next night.
                step(norms(pipeline::NormsCountry::ChCantons, Some(60)), 4_500),
                step(norms(pipeline::NormsCountry::Be, None), 1_200),
                step(norms(pipeline::NormsCountry::BeWal, None), 4_500),
            ],
        ),
        // Beside `norms`: in it, the weekly chain's worst case reaches 87,600 s, past the cron period.
        branch(
            "codex",
            &[],
            vec![step(norms(pipeline::NormsCountry::BeVlg, None), 2_400)],
        ),
        // Ontario's sitemaps, then the texts whose versions moved, on the daily night alone: the
        // weekly one has no room left. The first read of the 4,853 runs by hand.
        daily_only_branch(
            "ontario",
            &[],
            vec![step(norms(pipeline::NormsCountry::CaOn, Some(0)), 1_200)],
        ),
        // Off the compile's path: a journal's texts carry no dated state for it to wait on.
        branch(
            "journals",
            &[],
            vec![step(norms(pipeline::NormsCountry::Dj, None), 2_400)],
        ),
        // On the daily night alone: the weekly one has no room left under the cron period.
        daily_only_branch(
            "africa",
            &[],
            vec![
                step(norms(pipeline::NormsCountry::Bj, None), PLANCHER_SECS),
                // Adala's French and Arabic trees walked in about 6 min on 21/11/2026.
                step(norms(pipeline::NormsCountry::Ma, None), 1_500),
                // One host, one reader at a time. SenegalLII changes by batches, 129 works on
                // 14/11/2026: ten minutes of them, scans read by OCR, the rest the next night.
                step(norms(pipeline::NormsCountry::Ohada, None), PLANCHER_SECS),
                step(norms(pipeline::NormsCountry::Sn, Some(10)), PLANCHER_SECS),
                // Each type's newest hundred listed in about 4.5 min, then ten minutes of the
                // texts never visited: 5 to 8 new a day once the first read is through.
                step(norms(pipeline::NormsCountry::Mg, Some(10)), PLANCHER_SECS),
                // The courts' own sites still publishing, each at its pace, five minutes of listing
                // pages a site: the CCJA's 300 s between pages reads its two newest years.
                step(
                    Command::SyncCourtSites {
                        sites: Vec::new(),
                        full: false,
                        limit: None,
                        minutes: 5,
                        dry_run: false,
                        ocr_tenant: None,
                    },
                    4_500,
                ),
            ],
        ),
        // After `norms`, whose federal step reads the same host, and off the compile's path, on the
        // daily night alone: on the weekly one the worst case passes the cron period, and a daily
        // run relists a listing a week old. An hour of acts never visited.
        daily_only_branch(
            "justel",
            &["norms"],
            vec![step(norms(pipeline::NormsCountry::BeBru, Some(60)), 4_500)],
        ),
        // After every writer of the referential: the states of the texts the day
        // touched, and of those never compiled.
        branch(
            "compile",
            &["dila-texts", "eu-law", "norms", "codex", "ontario"],
            vec![step(
                Command::Assertions(AssertionsCommand::Compile {
                    full: false,
                    days: 3,
                    text_uid: None,
                    dry_run: false,
                }),
                2_400,
            )],
        ),
        branch(
            "bulletins",
            &[],
            vec![step(
                Command::SyncBulletins {
                    site: None,
                    full: false,
                    dry_run: false,
                },
                // Education's site map answering 502 to its eight pages held the step 13
                // minutes on 17/11/2026, and 1,400 s cut it before interior and economy.
                2_400,
            )],
        ),
        // After the bulletins, the page gateway serving one page at a time: 31 pages at 5 s.
        daily_only_branch(
            "boss",
            &["bulletins"],
            vec![step(Command::SyncBoss { dry_run: false }, PLANCHER_SECS)],
        ),
        branch(
            "service-public",
            &[],
            vec![step(Command::SyncServicePublic, PLANCHER_SECS)],
        ),
        branch(
            "summaries",
            &DECISION_SOURCES,
            vec![
                // Web doctrine, after the decision sources: a note attaches to a
                // decision already in the stock, which the day's harvest just wrote.
                step(
                    Command::SyncHal {
                        scan: pipeline::HalScan::Daily,
                    },
                    PLANCHER_SECS,
                ),
                step(
                    Command::SyncDoctrineOai {
                        scan: pipeline::DoctrineScan::Daily,
                        dry_run: false,
                    },
                    PLANCHER_SECS,
                ),
                step(
                    // By jurisdiction, newest first: an id sweep reads a loaded
                    // foreign stock before the day's decisions.
                    Command::GenerateSummaries {
                        limit: None,
                        refresh_stale: false,
                        cited_min: None,
                        by_jurisdiction: true,
                        reverse: false,
                        models: Vec::new(),
                        accounts: Vec::new(),
                        seat_caps: Vec::new(),
                        jurisdictions: Vec::new(),
                        named: false,
                        langs: Vec::new(),
                        reader: Some("fr".to_owned()),
                        stop_after: Some(5_400 - SUMMARY_DRAIN_SECS),
                    },
                    5_400,
                ),
                // After the summaries, so a decision of the day is embedded once, behind
                // its own; done before the watchdog, so no batch in flight is lost.
                step(
                    Command::EmbedMissing {
                        limit: None,
                        stock: false,
                        shard: (0, 1),
                        summarised: true,
                        by_jurisdiction: true,
                        multilingual: false,
                        stop_after: Some(1_680),
                    },
                    1_800,
                ),
            ],
        ),
        // After the decisions' passages, on the same host; a version the referential's writers
        // add later waits for the next day.
        daily_only_branch(
            "norm-vectors",
            &["summaries"],
            vec![step(
                Command::EmbedNorms {
                    limit: None,
                    stock: false,
                    stop_after: Some(600),
                },
                PLANCHER_SECS,
            )],
        ),
        branch(
            "publication",
            &[
                "judilibre",
                "dila",
                "cedh",
                "annonces",
                "dila-texts",
                "juricaf",
                "entscheidsuche",
                "legimonaco",
                "juportal",
                "rechtspraak",
                "justice-lu",
                "cce",
                "scc-csc",
                "legalis",
                "ddd",
                "amf",
                "juradm-site",
                "cc-site",
                "consiliaweb",
                "cjue",
                "cnda",
                "adlc",
                "comeu",
                "eu-law",
                "norms",
                "codex",
                "ontario",
                "journals",
                "africa",
                "justel",
                "compile",
                "bulletins",
                "boss",
                "service-public",
                "summaries",
                "norm-vectors",
            ],
            vec![
                step(Command::Db(DbCommand::Analyze), PLANCHER_SECS),
                step(Command::Db(DbCommand::GcMcp), PLANCHER_SECS),
                // After every writer of decisions and before the sitemap, which takes its page counts
                // from these counters.
                step(Command::Precompute, PLANCHER_SECS),
                step(Command::Sitemap { dry_run: false }, PLANCHER_SECS),
                step(
                    Command::Indexnow {
                        since_hours: 24.0,
                        max_urls: 10_000,
                    },
                    PLANCHER_SECS,
                ),
                // Last: the invariants measure the state the chain has just produced, and never fail it.
                step(Command::Invariants, PLANCHER_SECS),
            ],
        ),
    ]
}

/// A source's weekly read, beside the sources; the maintenance waits for them.
fn weekly_sources() -> Vec<Branch> {
    vec![
        // OpenCaseLaw's Obwalden file, read whole: it was last scraped in February 2026.
        branch(
            "opencaselaw",
            &[],
            vec![step(
                Command::SyncOpencaselaw { dry_run: false },
                PLANCHER_SECS,
            )],
        ),
        branch(
            "eu-nets",
            &["eu-law", "dila-texts"],
            vec![
                // The net of the registry delta the daily sync-eu-law opens with: the
                // whole registry, a few minutes of SPARQL. The next daily sync-eu-law
                // gives the catalogue entries it opens their body.
                step(
                    Command::SyncEuCatalog {
                        full: true,
                        limit: None,
                    },
                    PLANCHER_SECS,
                ),
                // What the daily JO ingest keeps depositing: an act named by DILA, its
                // body at EUR-Lex. Left alone the two identities drift apart again.
                step(Command::SyncEuJorf { limit: None }, PLANCHER_SECS),
            ],
        ),
        branch(
            "cedh-stubs",
            &["cedh"],
            vec![
                // The HUDOC rows the daily sweep stopped asking for, each asked again monthly.
                step(Command::SyncCedh { stubs: true }, PLANCHER_SECS),
            ],
        ),
        branch(
            "avocats",
            &[],
            vec![step(
                Command::Registries {
                    source: pipeline::RegistrySource::Avocats,
                    with_history: false,
                },
                PLANCHER_SECS,
            )],
        ),
        // After the day's HAL and OAI reads, in the summaries branch.
        branch(
            "doctrine-repair",
            &["summaries"],
            vec![
                // Net under the daily scan: 180 days instead of 30, and a `warn!` on what
                // it repairs. A week repairing nothing says the daily pass holds.
                step(
                    Command::SyncHal {
                        scan: pipeline::HalScan::Repair,
                    },
                    PLANCHER_SECS,
                ),
                step(
                    Command::SyncDoctrineOai {
                        scan: pipeline::DoctrineScan::Repair,
                        dry_run: false,
                    },
                    PLANCHER_SECS,
                ),
            ],
        ),
    ]
}

/// Weekly maintenance, `turn` taken from [`rotation`].
fn weekly(turn: Step) -> Vec<Step> {
    vec![
        step(Command::JudilibreReconcile { apply: true }, PLANCHER_SECS),
        step(Command::StampTexts, PLANCHER_SECS),
        step(Command::Db(DbCommand::ReindexSearch), 9_000),
        // Filet de forme du pot Annonces : l'ingest écrit propre à la
        // construction, ce passage porte une règle nouvelle au stock clos que
        // la moisson ne revisite jamais (titres, acheteurs, lots).
        step(Command::AnnoncesRegulariser { formes: true }, PLANCHER_SECS),
        step(Command::PartiesRelink { full: false }, PLANCHER_SECS),
        turn,
        // 75 fiches, 81 ancres, et le Conseil d'État en ajoute quelques-unes par
        // an : hebdomadaire suffit. Avant les renvois, qui recomposent le titre
        // indexé des décisions qu'elle vient de nommer.
        step(Command::SyncCeList, PLANCHER_SECS),
        // About fifteen requests, before the renvois that recompose the titles it names.
        step(Command::SyncWikiNames, PLANCHER_SECS),
        step(
            Command::SyncDoctrineNames {
                scan: pipeline::DoctrineNamesScan::Weekly,
                limit: None,
                dry_run: false,
            },
            1_800,
        ),
        // Renvois reproject every ArianeWeb bundle and repose the usage names; the
        // law nickname mining reads the bodies carrying « dite loi » beside them.
        step(Command::Renvois, PLANCHER_SECS),
        step(Command::AliasCandidats { revue: None }, PLANCHER_SECS),
        step(Command::ExtractTextRefs, 2_400),
        // Affiliations tirées des appositions : une fonction du corpus, donc à
        // recalculer quand le corpus a bougé. Après la ré-extraction (qui rafraîchit
        // `firm_ord`) et le relink (qui résout les deux bouts).
        step(Command::DeriveAffiliations { apply: true }, PLANCHER_SECS),
        // Reads what the week's harvest failed to reach against the last census
        // of `annonces-discover`, which tells an unconnected engine from an
        // unlisted tenant. Writes nothing.
        step(Command::AnnoncesCouverture { limite: 40 }, PLANCHER_SECS),
        // What the daily chain could not write as a collage, its court having
        // published nothing yet: ArianeWeb lands months after the decision, so the
        // summary exists before the words that beat it. The CJUE's and the CEDH's
        // summaries every year, from the cache.
        step(
            Command::UpgradeSummariesCommentaries { limit: None },
            PLANCHER_SECS,
        ),
        // Filet de dérive du régime « dérivé à l'écriture » : ferme la marche, il
        // doit voir l'état final du run.
        step(Command::Reconcile, 3_600),
    ]
}

/// Passes over the whole stock whose result moves slowly: one per week, in
/// turn, each back every four weeks.
fn rotation() -> Vec<Step> {
    vec![
        step(Command::BuildSuggest, 3_600),
        step(
            Command::UsageTerms {
                chunks: 32,
                min_citations: 10,
                workers: 6,
            },
            3_600,
        ),
        step(
            Command::ReextractFields {
                dry_run: false,
                full: true,
                jurisdiction_type: Vec::new(),
                jurisdiction_code: Vec::new(),
                field: Vec::new(),
                citing_ref_uid: None,
                text_query: None,
                several_provenances: false,
                // The watchdog below holds at 8 workers.
                workers: Some(8),
            },
            12_900,
        ),
        // What weighs is the share of hosts that does not answer, four probes in
        // series each, and nothing is written before the end.
        step(Command::AnnoncesDiscover, 2_700),
    ]
}

/// What the day's quotas still hold, spent before they reset.
///
/// It serves the cited decisions first, read more often than the others. Only
/// the supreme courts' are rewritten, where generation 11 gains on generation 10.
fn night_tail() -> Vec<Step> {
    vec![step(
        Command::GenerateSummaries {
            limit: None,
            refresh_stale: true,
            cited_min: Some(1),
            by_jurisdiction: false,
            reverse: false,
            models: Vec::new(),
            accounts: Vec::new(),
            seat_caps: Vec::new(),
            jurisdictions: vec!["CC".to_owned(), "CE".to_owned()],
            named: false,
            langs: Vec::new(),
            reader: None,
            stop_after: Some(64_800 - SUMMARY_DRAIN_SECS),
        },
        64_800,
    )]
}

/// The chain's branches, `week` picking the turn of [`rotation`].
fn plan(kind: ChainKind, week: usize) -> Vec<Branch> {
    if matches!(kind, ChainKind::Drain) {
        return vec![branch("drain", &[], night_tail())];
    }
    let mut branches = daily();
    if matches!(kind, ChainKind::Weekly) {
        for s in branches.iter_mut().flat_map(|b| &mut b.steps) {
            match &mut s.command {
                // The only pass that pays for the tenants silent seven nights: a
                // platform given back is found within a week, the other six do not
                // probe it.
                Command::Annonces { relancer_muets, .. } => *relancer_muets = true,
                // Ahead of the compile, which then sees the same night what the relist repairs.
                Command::SyncNorms { scan, .. } => *scan = pipeline::NormsScan::Relist,
                // Whole reads at about 3x their longest run to 20/11/2026: 945, 1,739, 1,660 s.
                Command::SyncDdd { full } => {
                    *full = true;
                    s.watchdog_secs = 2_850;
                }
                Command::SyncAmf { full } => {
                    *full = true;
                    s.watchdog_secs = 4_500;
                }
                Command::SyncComeu { full } => {
                    *full = true;
                    s.watchdog_secs = 5_000;
                }
                // Its catch-up starts no fetch past 4,200 s.
                Command::SyncEntscheidsuche { missing, .. } => {
                    *missing = true;
                    s.watchdog_secs = 5_400;
                }
                _ => {}
            }
        }
        for b in branches.iter_mut().filter(|b| b.daily_only) {
            b.steps.clear();
        }
        let mut rotation = rotation();
        let turn = rotation.swap_remove(week % rotation.len());
        branches.extend(weekly_sources());
        branches.push(branch(
            "weekly",
            &[
                "publication",
                "opencaselaw",
                "eu-nets",
                "cedh-stubs",
                "avocats",
                "doctrine-repair",
            ],
            weekly(turn),
        ));
    }
    branches
}

/// Runs the chain once the previous one has ended. A branch runs its steps in order, each under
/// its own watchdog, and a failed step does not stop the next ones. Returns `Err` when a step
/// failed, for the cron to mark the run as failed.
///
/// `LIBREJUSTICE_INGEST_WATCHDOG_SECS` caps a direct invocation only: the chain carries its own.
pub(crate) async fn run(kind: ChainKind) -> Result<()> {
    let chain = kind.label();
    let settings = Settings::from_env()?;
    let pool = crate::pool_only(&settings, 1).await?;
    let lock = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let mut waited = false;
    while !DecisionRepository::new(&lock).chain_lock().await? {
        if !waited {
            tracing::warn!(chain, "chain: another chain holds the lock, waiting for it");
            waited = true;
        }
        tokio::time::sleep(LOCK_POLL).await;
    }

    let branches = plan(kind, this_week());
    let total: usize = branches.iter().map(|b| b.steps.len()).sum();
    let started = Instant::now();
    tracing::info!(chain, total, "chain: start");
    let failed = schedule(branches, move |branch| run_branch(chain, branch)).await?;

    let elapsed_s = started.elapsed().as_secs();
    if failed.is_empty() {
        tracing::info!(chain, total, elapsed_s, "chain: success");
        return Ok(());
    }
    let steps_failed = failed.join(",");
    tracing::error!(
        chain,
        total,
        chain_failed = failed.len(),
        steps_failed,
        elapsed_s,
        "chain: steps failed"
    );
    bail!(
        "chain {chain} : {}/{total} étapes en échec ({steps_failed})",
        failed.len()
    )
}

fn this_week() -> usize {
    (chrono::Utc::now().timestamp() / (7 * 86_400)) as usize
}

/// Walks the plan as [`run`] would, each branch on its thread once those it waits for have ended,
/// a step named rather than run, after the schema check every step makes.
pub(crate) async fn walk(kind: ChainKind) -> Result<()> {
    let chain = kind.label();
    let settings = Settings::from_env()?;
    let pool = crate::pool_only(&settings, 1).await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let branches = plan(kind, this_week());
    let total: usize = branches.iter().map(|b| b.steps.len()).sum();
    schedule(branches, move |branch| walk_branch(chain, branch)).await?;
    tracing::info!(chain, total, "chain: plan walked");
    Ok(())
}

async fn walk_branch(chain: &'static str, branch: Branch) -> (&'static str, Vec<&'static str>) {
    for step in &branch.steps {
        tracing::info!(
            chain,
            branch = branch.name,
            step = step.command.label(),
            watchdog_secs = step.watchdog_secs,
            "chain: step planned"
        );
    }
    (branch.name, Vec::new())
}

/// Runs `pending` side by side, at most [`BRANCHES_AT_ONCE`], each once those it waits for have
/// ended, in the order listed among those ready; returns the steps that failed.
async fn schedule<R, F>(mut pending: Vec<Branch>, run: R) -> Result<Vec<&'static str>>
where
    R: Fn(Branch) -> F + Clone + Send + 'static,
    F: Future<Output = (&'static str, Vec<&'static str>)>,
{
    let mut ended: HashSet<&'static str> = HashSet::new();
    let mut running = JoinSet::new();
    let mut failed = Vec::new();
    loop {
        while running.len() < BRANCHES_AT_ONCE {
            let Some(ready) = pending
                .iter()
                .position(|b| b.after.iter().all(|a| ended.contains(a)))
            else {
                break;
            };
            let (handle, branch, run) = (Handle::current(), pending.remove(ready), run.clone());
            let (done, ended_rx) = tokio::sync::oneshot::channel();
            // On a thread of its own, so that a step's future need not be `Send`.
            std::thread::Builder::new()
                .name(format!("chain-{}", branch.name))
                .stack_size(BRANCH_STACK)
                .spawn(move || done.send(handle.block_on(run(branch))))?;
            running.spawn(ended_rx);
        }
        let Some(joined) = running.join_next().await else {
            break;
        };
        let (name, branch_failed) = joined
            .map_err(|e| anyhow!("chain branch: {e}"))?
            .map_err(|_| anyhow!("chain branch: its thread ended without an answer"))?;
        ended.insert(name);
        failed.extend(branch_failed);
    }
    Ok(failed)
}

/// A branch's steps in order, a panic failing its step alone.
async fn run_branch(chain: &'static str, branch: Branch) -> (&'static str, Vec<&'static str>) {
    let mut failed = Vec::new();
    for Step {
        command,
        watchdog_secs,
    } in branch.steps
    {
        let step = command.label();
        let depart = Instant::now();
        let span = tracing::info_span!("chain_step", branch = branch.name, step);
        let ok = AssertUnwindSafe(crate::run_with_watchdog(command, watchdog_secs))
            .catch_unwind()
            .instrument(span)
            .await
            .is_ok_and(|result| result.is_ok());
        if !ok {
            failed.push(step);
        }
        // A step cut by the watchdog logs nothing of its own, and the delay to
        // compare a duration against is here rather than in the command.
        tracing::info!(
            chain,
            branch = branch.name,
            step,
            ok,
            watchdog_secs,
            elapsed_s = depart.elapsed().as_secs(),
            "chain: step"
        );
    }
    (branch.name, failed)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;

    /// Période du cron : deux runs consécutifs sont à 24 h.
    const PERIODE_CRON_SECS: u64 = 86_400;

    /// Every step cut at its watchdog still ends before the next run. A list schedule on
    /// [`BRANCHES_AT_ONCE`] slots ends by (work - critical path) / slots + critical path, so a
    /// source added beside the others costs a quarter of its watchdog.
    #[test]
    fn the_worst_case_holds_under_the_cron_period() {
        for kind in [ChainKind::Daily, ChainKind::Weekly, ChainKind::Drain] {
            for week in 0..rotation().len() {
                let branches = plan(kind, week);
                let length = |b: &Branch| b.steps.iter().map(|s| s.watchdog_secs).sum::<u64>();
                let work: u64 = branches.iter().map(length).sum();
                let mut ends: HashMap<&str, u64> = HashMap::new();
                for b in &branches {
                    let start = b.after.iter().map(|a| ends[a]).max().unwrap_or(0);
                    ends.insert(b.name, start + length(b));
                }
                let path = ends.values().copied().max().unwrap_or(0);
                let slots = BRANCHES_AT_ONCE as u64;
                let worst = (work - path).div_ceil(slots) + path;
                assert!(
                    worst < PERIODE_CRON_SECS,
                    "chain {} week {week}: worst case {worst} s, period {PERIODE_CRON_SECS} s",
                    kind.label()
                );
            }
        }
    }

    /// A branch waits only for branches listed before it, so every plan runs to its end.
    #[test]
    fn a_branch_waits_for_branches_listed_before_it() {
        for kind in [ChainKind::Daily, ChainKind::Weekly, ChainKind::Drain] {
            let branches = plan(kind, 0);
            for (i, b) in branches.iter().enumerate() {
                for a in b.after {
                    assert!(
                        branches[..i].iter().any(|x| x.name == *a),
                        "chain {}: {} waits for {a}, not listed before it",
                        kind.label(),
                        b.name
                    );
                }
            }
            let mut names: Vec<_> = branches.iter().map(|b| b.name).collect();
            names.sort_unstable();
            names.dedup();
            assert_eq!(
                names.len(),
                branches.len(),
                "chain {}: a name twice",
                kind.label()
            );
        }
    }

    /// Run for real on the runtime: a branch starts once those it waits for have ended, and no
    /// more than [`BRANCHES_AT_ONCE`] run at once.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_branch_starts_after_those_it_waits_for_within_the_slots() {
        let plan = vec![
            branch("a", &[], vec![step(Command::Sync, 900)]),
            branch("b", &[], vec![step(Command::Sync, 900)]),
            branch("c", &[], vec![step(Command::Sync, 900)]),
            branch("d", &[], vec![step(Command::Sync, 900)]),
            branch("e", &[], vec![step(Command::Sync, 900)]),
            branch("f", &["a"], vec![step(Command::Sync, 900)]),
            branch("g", &["b", "f"], vec![step(Command::Sync, 900)]),
        ];
        let after: HashMap<&str, &[&str]> = plan.iter().map(|b| (b.name, b.after)).collect();
        let log = Arc::new(Mutex::new(Vec::new()));
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let failed = schedule(plan, {
            let (log, live, peak) = (log.clone(), live.clone(), peak.clone());
            move |b: Branch| {
                let (log, live, peak) = (log.clone(), live.clone(), peak.clone());
                async move {
                    peak.fetch_max(live.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                    log.lock().unwrap().push((b.name, true));
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    log.lock().unwrap().push((b.name, false));
                    live.fetch_sub(1, Ordering::SeqCst);
                    (b.name, vec!["x"])
                }
            }
        })
        .await
        .expect("schedule");
        assert_eq!(failed.len(), 7);
        assert!(peak.load(Ordering::SeqCst) <= BRANCHES_AT_ONCE);
        let log = log.lock().unwrap();
        for (i, (name, start)) in log.iter().enumerate() {
            if *start {
                for a in after[name] {
                    assert!(
                        log[..i].contains(&(*a, false)),
                        "{name} started before {a} ended"
                    );
                }
            }
        }
    }

    /// A branch runs on the main thread's stack, which some steps need beyond a pool thread's.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_branch_has_the_stack_of_the_main_thread() {
        #[inline(never)]
        fn deep(n: usize) -> usize {
            let frame = [n; 1 << 17];
            std::hint::black_box(&frame);
            if n == 0 {
                0
            } else {
                deep(n - 1) + frame[0]
            }
        }
        let failed = schedule(
            vec![branch("deep", &[], Vec::new())],
            |b: Branch| async move { (b.name, vec![if deep(3) == 6 { "4 MiB" } else { "wrong" }]) },
        )
        .await
        .expect("schedule");
        assert_eq!(failed, ["4 MiB"]);
    }

    /// The publication waits for every branch the day runs before it.
    #[test]
    fn the_publication_waits_for_every_source() {
        let branches = daily();
        let publication = branches
            .iter()
            .find(|b| b.name == "publication")
            .expect("publication");
        for b in branches.iter().filter(|b| b.name != "publication") {
            assert!(
                publication.after.contains(&b.name),
                "publication does not wait for {}",
                b.name
            );
        }
    }

    /// Aucun délai sous le plancher : une journée simplement lente ne doit pas
    /// suffire à faire couper une étape.
    #[test]
    fn aucun_delai_sous_le_plancher() {
        for kind in [ChainKind::Daily, ChainKind::Weekly, ChainKind::Drain] {
            for s in (0..rotation().len())
                .flat_map(|week| plan(kind, week))
                .flat_map(|b| b.steps)
            {
                assert!(
                    s.watchdog_secs >= PLANCHER_SECS,
                    "étape {} : délai {} s sous le plancher {PLANCHER_SECS} s",
                    s.command.label(),
                    s.watchdog_secs
                );
            }
        }
    }
}
