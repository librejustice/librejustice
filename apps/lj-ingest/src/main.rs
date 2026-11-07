//! Binaire `lj-ingest` : CLI d'ingest (clap).

mod alias_candidats;
mod analyze;
mod case_names;
mod chain;
mod citations_maintenance;
mod config;
mod credential;
mod disposable_domains;
mod indexnow;
mod invariants;
mod judilibre_reconcile;
mod logging;
mod ocr;
mod pipeline;
mod renvois;
mod reverses;
mod sitemap;
mod summary;
mod summary_batch;
mod tombstones;
mod usage_terms;

use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use lj_store::repository::Courts;
use lj_telemetry::{InitOpts, OtlpCreds, TelemetryGuard};
use pipeline::Worklist;
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::EnvFilter;

use crate::config::Settings;

/// URL de prod de l'API PISTE Judilibre.
const JUDILIBRE_BASE_URL: &str = "https://api.piste.gouv.fr/cassation/judilibre/v1.0";

/// Construit un client Judilibre depuis les identifiants OAuth2 PISTE partagés
/// (`LIBREJUSTICE_PISTE_CLIENT_ID`/`_SECRET`, mêmes que Légifrance).
fn judilibre_client(settings: &Settings) -> Result<lj_sources::judilibre::JudilibreClient> {
    let client_id = settings
        .piste_client_id
        .clone()
        .ok_or_else(|| anyhow!("LIBREJUSTICE_PISTE_CLIENT_ID requis pour la source judilibre."))?;
    let client_secret = settings.piste_client_secret.clone().ok_or_else(|| {
        anyhow!("LIBREJUSTICE_PISTE_CLIENT_SECRET requis pour la source judilibre.")
    })?;
    Ok(lj_sources::judilibre::JudilibreClient::new(
        JUDILIBRE_BASE_URL,
        client_id,
        client_secret,
    ))
}

#[derive(Parser)]
#[command(
    name = "librejustice",
    about = "CLI ingest LibreJustice (pipelines offline)."
)]
struct Cli {
    /// Niveau de log global (trace/debug/info/warning/error ou entier).
    #[arg(
        long,
        global = true,
        env = "LIBREJUSTICE_LOG_LEVEL",
        default_value = "INFO"
    )]
    log_level: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum SubmissionsAction {
    /// The pending submissions: id, arrival, size, account.
    List,
    /// A submission's text, then what the anonymisation detector finds in it.
    Show { id: i64 },
    /// Writes the decision under the source `contribution` and deletes the submission.
    Approve {
        id: i64,
        /// Court type, as `jurisdiction_type`: CNDA, TA, CAA, CE, CA, TJ, CC…
        #[arg(long)]
        court: String,
        #[arg(long)]
        number: String,
        /// Reading date, YYYY-MM-DD. A CNDA decision reads it from its text.
        #[arg(long)]
        date: Option<String>,
        /// Court name, as the decision writes it.
        #[arg(long)]
        name: Option<String>,
        /// Registry code of an administrative court (TA75), Judilibre location of a judicial one (ca_paris).
        #[arg(long)]
        code: Option<String>,
    },
    /// Deletes the submission.
    Reject { id: i64 },
    /// Writes a folder of anonymised decisions under the source `contribution`:
    /// `decisions.jsonl` with each court code, date and numbers, `texte/<uid>.txt`.
    Load {
        dir: std::path::PathBuf,
        /// Builds every decision and counts them, writes nothing.
        #[arg(long)]
        dry_run: bool,
        /// `all` rewrites a decision already written, keeping its public id.
        #[arg(long, value_enum, default_value_t = pipeline::IngestMode::MissingHash)]
        mode: pipeline::IngestMode,
    },
}

#[derive(Subcommand)]
enum CredentialAction {
    #[command(about = "Verse dans un pool les clés que porte une variable")]
    Import {
        provider: String,
        #[arg(help = "Nom de la variable qui porte les clés, séparées par des virgules")]
        from: String,
        #[arg(
            long,
            help = "Variable CSV nommant la frontière de quota de chaque clé, dans le même ordre"
        )]
        tenants: Option<String>,
        #[arg(long, help = "Compte de console qui détient ces frontières")]
        account: Option<String>,
    },
    #[command(about = "Remplit les pools vides depuis les variables d'amorce, laisse les autres")]
    Seed,
    #[command(about = "Affiche le pool tel que la base le tient, jamais le secret")]
    List,
    #[command(about = "Nomme la frontière de quota d'une clé et le compte qui la détient")]
    Name {
        fingerprint: String,
        #[arg(long)]
        tenant: Option<String>,
        #[arg(long)]
        account: Option<String>,
    },
    #[command(about = "Connexion device d'un siège OpenAI : affiche un code, attend, range")]
    Login,
    #[command(about = "Rafraîchit les jetons OAuth qui périment sous deux jours")]
    Refresh,
    #[command(
        about = "Quota vivant des comptes ChatGPT : fenêtres et crédits de réinitialisation"
    )]
    Usage,
    #[command(about = "Brûle un crédit de réinitialisation, la semaine devant être épuisée")]
    ResetCredit {
        #[arg(help = "Nom du compte dans le pool")]
        label: String,
        #[arg(
            long,
            help = "Brûle même si la fenêtre de sept jours a encore de la place"
        )]
        force: bool,
    },
    #[command(about = "Retire une clé du pool")]
    Forget { fingerprint: String },
    #[command(about = "Gare une clé jusqu'au prochain reset mensuel, ou jusqu'à `enable`")]
    Disable {
        fingerprint: String,
        #[arg(long, default_value_t = 402, help = "Statut HTTP constaté au refus")]
        status: i16,
        #[arg(
            long,
            help = "Garée jusqu'à `credential enable`, pour un compte que son titulaire utilise aussi"
        )]
        until_enabled: bool,
    },
    #[command(about = "Rend au pool une clé garée")]
    Enable { fingerprint: String },
}

#[derive(Subcommand)]
enum Command {
    /// Déroule une chaîne planifiée (cron). Chaque étape a son propre délai de
    /// watchdog et un échec n'interrompt pas les suivantes ; la chaîne sort
    /// non-zéro après un `error!` récapitulatif.
    Chain {
        #[arg(value_enum)]
        kind: chain::ChainKind,
    },
    /// Sync incrémental des sources (opendata ZIP + Judilibre).
    Sync,
    /// Calcule les plages de dates Judilibre à synchroniser.
    JudilibreRanges,
    /// Analyse statistique d'un corpus local.
    Analyze,
    /// Applique les migrations Postgres.
    Migrate,
    #[command(about = "Pool de clés LLM : verser, nommer, lire ce que la base tient")]
    Credential {
        #[command(subcommand)]
        action: CredentialAction,
    },
    /// The decisions accounts submit at `/decision/contribute`, reviewed one by one.
    Submissions {
        #[command(subcommand)]
        action: SubmissionsAction,
    },
    /// Projette les renvois des analyses du Conseil d'État en relation de
    /// citation (`decision_renvoi`) et résout les cibles.
    Renvois,
    /// Remplit `alias_candidate` : surnoms d'arrêts et de lois minés depuis les
    /// renvois du Conseil d'État et la locution « dite loi ». N'active rien.
    AliasCandidats {
        /// N'extrait rien : relit les N candidats de tête, par exclusivité.
        #[arg(long, num_args = 0..=1, default_missing_value = "60")]
        revue: Option<i64>,
    },
    /// Matérialise le champ `legal_article.usage_terms` (sacs de n-grammes
    /// des contextes de citation + seeds curatés).
    UsageTerms {
        /// Nombre de chunks du pool (grain de progression et de répartition).
        #[arg(long, default_value_t = 32)]
        chunks: usize,
        /// Seuil de citations pour matérialiser un sac.
        #[arg(long, default_value_t = 10)]
        min_citations: i64,
        /// Connexions parallèles (le coût est I/O-bound : lecture TOAST des
        /// full_text échantillonnés).
        #[arg(long, default_value_t = 6)]
        workers: usize,
    },
    /// Writes the local archives into the base, `embed-missing` cutting the
    /// passages of what it writes.
    Ingest {
        /// Triage. `missing-hash` (default): skip an unchanged hash, and a file
        /// the manifest says is ingested. `all`: rereads everything and rewrites
        /// every decision, after a change of the pure pipeline.
        #[arg(long, value_enum, default_value_t = pipeline::IngestMode::MissingHash)]
        mode: pipeline::IngestMode,
    },
    /// Re-fetch ciblé de décisions Judilibre par id (`/decision`) puis ingest.
    /// Répare quelques décisions désynchronisées (ex. résurrection) sans reculer
    /// le watermark global.
    Refetch {
        /// Ids Judilibre à re-fetch (ex. `6a28fc8ecdc6046d47cb0167`).
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Compares every id Judilibre serves with the base: fetches into the cache
    /// what the base lacks, tombstones what Judilibre answers 404 for, both
    /// for the next `ingest`.
    JudilibreReconcile {
        /// Without it, counts and writes nothing.
        #[arg(long)]
        apply: bool,
    },
    /// Ré-extrait les champs structurés depuis les payloads stockés.
    ReextractFields {
        /// Sonde : n'écrit RIEN, compte les lignes que la passe réécrirait
        /// (même comparaison NULL-safe que le skip-tuple de l'UPDATE) et les
        /// couches satellites qu'elle purgerait.
        #[arg(long)]
        dry_run: bool,
        /// Passe intégrale : TOUT le fonds `extract_version < 1000`, même déjà à
        /// la version courante — c'est le relink hebdomadaire (un texte nouveau au
        /// catalogue attire ses citations anciennes ; le skip des sets de citations
        /// inchangés borne les écritures au delta).
        #[arg(long, conflicts_with_all = ["jurisdiction_type", "jurisdiction_code", "citing_ref_uid"])]
        full: bool,
        /// Cible des `jurisdiction_type` (CSV, ex. `CNDA,CEDH,CJUE,CONSTIT,TC`) à
        /// re-extraire **quelle que soit la `extract_version`** (sans bump global).
        /// Pour activer un comportement d'extraction nouveau sur un sous-ensemble.
        /// Vide = comportement par défaut (toutes les versions divergentes).
        #[arg(long, value_delimiter = ',')]
        jurisdiction_type: Vec<String>,
        /// Courts (`decisions.jurisdiction_code`, CSV, e.g. `tj_papeete,ca_papeete`)
        /// to re-extract whatever their `extract_version`, as `--jurisdiction-type`
        /// does for a whole order of courts.
        #[arg(long, value_delimiter = ',', conflicts_with = "jurisdiction_type")]
        jurisdiction_code: Vec<String>,
        /// Champs à re-extraire (CSV, ex. `legal_references`). Vide = tous les
        /// champs ré-extractibles. Restreindre à `legal_references` ne ré-écrit
        /// que les citations (via `replace_citations`), sans toucher les colonnes.
        #[arg(long, value_delimiter = ',')]
        field: Vec<String>,
        /// Cible les décisions **citant** ce texte (`ref_text_uid`), quelle que
        /// soit la `extract_version`. Pour rejouer extract+link sur le seul gisement
        /// d'un instrument, sans bump global.
        #[arg(long)]
        citing_ref_uid: Option<String>,
        /// Expression BM25 sur le texte (`parse_with_field`, termes pliés sans
        /// accent : `pourvoi`, `"decision deferee"`), avec
        /// `--jurisdiction-type` : ne repasse que le gisement du littéral.
        #[arg(long, requires = "jurisdiction_type")]
        text_query: Option<String>,
        /// The decisions whose fields come from several provenances, whatever
        /// their `extract_version`.
        #[arg(long, conflicts_with_all = ["full", "jurisdiction_type", "jurisdiction_code", "citing_ref_uid"])]
        several_provenances: bool,
        /// Concurrent workers = simultaneous DB connections (each one runs
        /// fetch, extract and write in parallel). This is the pressure lever on the
        /// database: empty = 3, gentle on a prod that serves traffic; the nightly
        /// chain asks for 8, and a pass in a hurry can go as high.
        #[arg(long)]
        workers: Option<usize>,
    },
    /// Replays the room resolution on the stored seat, without parsing:
    /// rewrites `seat_paths` where the court vocabulary moved.
    ResolveSeats {
        /// Counts the rows that would change, writes nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Derives `formation_paths` from the stored seat paths and bench axes,
    /// without parsing.
    ResolveFormations {
        /// Counts the rows that would change and prints each path's decisions,
        /// writes nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Re-chunks and re-embeds from the stored text and its summary, on the
    /// local embedder, the decisions whose passages lack an embedding, were
    /// never cut, or were embedded before their summary.
    EmbedMissing {
        /// Bounds the decisions visited (default: all).
        #[arg(long)]
        limit: Option<usize>,
        /// Also every decision below `EMBED_VERSION` or embedded behind another
        /// summary than its text's.
        #[arg(long)]
        stock: bool,
        /// `k/n`: only the decisions with `id % n == k`, one machine each.
        #[arg(long, value_parser = pipeline::parse_shard, default_value = "0/1")]
        shard: (i64, i64),
    },
    /// Purge les liens de réversion (reverses).
    PurgeReverses,
    /// Withdraws a decision at its address: no ingest writes it back until the
    /// withdrawal is lifted.
    Withdraw {
        /// The `public_id`, last segment of the decision's URL.
        public_id: String,
        /// Why, kept with the address.
        #[arg(long, required_unless_present = "lift")]
        reason: Option<String>,
        /// Lifts the withdrawal instead.
        #[arg(long, conflicts_with = "reason")]
        lift: bool,
    },
    /// Génère les résumés LLM manquants.
    GenerateSummaries {
        #[arg(long)]
        limit: Option<usize>,
        /// Régénère AUSSI les résumés dont le prompt est plus ancien que la
        /// version courante (`summary_prompt_version < SUMMARY_PROMPT_VERSION`).
        /// Sans ce flag, seuls les résumés manquants (`summary IS NULL`) sont
        /// générés.
        #[arg(long)]
        refresh_stale: bool,
        #[arg(
            long,
            help = "Sert d'abord les décisions les plus citées dans le fonds, à partir de ce nombre de citations"
        )]
        cited_min: Option<i32>,
        /// Serves supreme courts first, then appeal, then first instance, most recent first in each.
        #[arg(long, conflicts_with = "cited_min")]
        by_jurisdiction: bool,
        /// The same order backwards, first instance and oldest first: a second
        /// run that never meets the first.
        #[arg(long, requires = "by_jurisdiction")]
        reverse: bool,
        /// Models this run spends, each until its quota refuses; the chain's when absent.
        #[arg(long = "model", value_enum)]
        models: Vec<summary::Model>,
        /// Credentials this run may spend, by tenant or fingerprint; every live one when absent.
        #[arg(long = "account")]
        accounts: Vec<String>,
        /// Jurisdiction types the cited or tiered order takes, `CC` and `CE` for the
        /// supreme courts; every one when absent.
        #[arg(long = "jurisdiction")]
        jurisdictions: Vec<String>,
        /// Only the decisions with a usage name (`decisions.case_name`), on the cited
        /// or tiered order.
        #[arg(long)]
        named: bool,
    },
    /// Overwrites the summary of the decisions whose court has since published its
    /// own commentary, pasted rather than generated.
    /// The CJUE's and the CEDH's own summaries come with it, every year swept.
    UpgradeSummariesCommentaries {
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Passes back through the current engines what a retired engine wrote, collage or
    /// rewrite, one jurisdiction at a time. Selects on `decisions.summary_engine`, the
    /// text having changed without its generation moving: `--refresh-stale` would
    /// take the whole stale corpus along.
    RecollageSummaries {
        /// Engine whose summaries are taken back, 7 for `LunaCollageV10`, 27 for a
        /// court's summary pasted whole.
        #[arg(long)]
        from_engine: i16,
        /// Jurisdiction type, `CE` for the Conseil d'État. All of them by default.
        #[arg(long)]
        jurisdiction: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
        /// Models this run spends, each until its quota refuses; the chain's when absent.
        #[arg(long = "model", value_enum)]
        models: Vec<summary::Model>,
    },
    /// Haiku through the Message Batches API: collects the batches that have ended,
    /// then submits up to `--limit` stale decisions in jurisdiction order.
    SummaryBatch {
        #[arg(long)]
        limit: Option<usize>,
        /// First instance and oldest first.
        #[arg(long)]
        reverse: bool,
        /// Polls until no batch is left open.
        #[arg(long)]
        wait: bool,
    },
    /// Replays the deterministic cleaning of `lj-core` over the stored summaries.
    /// Idempotent, a no-op in steady state; to be run after that cleaning gains a
    /// rule, which leaves the stock carrying what it now refuses.
    RecleanSummaries,
    /// Writes what a summary run would send for the decisions of a bench file.
    SummaryInputs {
        bench: std::path::PathBuf,
        out: std::path::PathBuf,
    },
    /// Turns a bench's raw answers into what the store would hold.
    SummaryFinish {
        #[arg(value_enum)]
        answer: summary::Answer,
        inputs: std::path::PathBuf,
        answers: std::path::PathBuf,
        out: std::path::PathBuf,
    },
    /// Recompute des arrays légaux dénormalisés depuis `legal_citation`
    /// (réparation). Idempotent, lots autocommit ; en régime normal l'écrivain
    /// de citations tient les arrays à jour.
    ResyncLegalArrays,
    /// Instantané unique du fonds : enchaîne les trois recomptes que rien ne
    /// tient à l'écriture, compteurs par juridiction, référentiel `chamber` et
    /// `corpus_counts`. Idempotent ; planifié dans la chaîne quotidienne.
    Precompute,
    /// Recompte `jurisdiction_year_counts` depuis `decisions` : les compteurs
    /// que lisent les hubs juridiction et les nombres de pages du sitemap.
    /// Idempotent.
    ResyncJurisdictionCounts,
    /// Recompte `corpus_counts` : total et facettes du fonds entier que sert le
    /// mode parcours, et les trois totaux de la page d'accueil. Idempotent.
    ResyncCorpusCounts,
    /// Re-key one-shot du namespace cnb: vers l'identité CNBF : ponte les uids
    /// legacy {barreau}:{nom}-{prenom} vers cnb:{avCnbfCode} (snapshot courant),
    /// purge les entités pieds de fichier, réhydrate les liens danglants.
    /// Dry-run par défaut ; préalable au premier reload upsert de
    /// `registries avocats`.
    RekeyCnb {
        /// Écrit réellement (sans ce flag : plan + compteurs, aucune écriture).
        #[arg(long)]
        apply: bool,
    },
    /// Rejeu des snapshots CNB publiés depuis 2022 : plie la présence observée
    /// en spans d'activité (come and go) et le siren de structure en arêtes
    /// d'affiliation datées, sous la provenance `cnb-hist`. Recalcule ses
    /// évidences en entier à chaque run et remplace les précédentes. Dry-run par
    /// défaut.
    CnbHistory {
        /// Écrit réellement (sans ce flag : plan + compteurs, aucune écriture).
        #[arg(long)]
        apply: bool,
    },
    /// Affiliations avocat ↔ cabinet dérivées des appositions des décisions,
    /// sous la provenance `decision` : couples `firm_ord` dont les deux bouts
    /// sont résolus, attestés par ≥ 2 décisions distinctes. Fonction du corpus —
    /// recalcule en entier à chaque run. Dry-run par défaut.
    DeriveAffiliations {
        /// Écrit réellement (sans ce flag : compteurs, aucune écriture).
        #[arg(long)]
        apply: bool,
    },
    /// Filet unique de dérive : enchaîne tous les filets du régime « dérivé à
    /// l'écriture » — rôles de textes + dénorm articles, arrays légaux,
    /// liens/citations pendants. Idempotent, ~no-op en régime ; planifié en fin
    /// de chaîne dominicale. Le volet parties vit dans `parties-relink`.
    Reconcile,
    /// Génère les sitemaps et les publie en base (servis par lj-server).
    Sitemap {
        /// Construit les sitemaps sans les écrire en base (vérification locale).
        #[arg(long)]
        dry_run: bool,
    },
    /// Pousse les URLs modifiées via IndexNow.
    Indexnow {
        /// Pousse les décisions dont updated_at est plus récent que N heures.
        #[arg(long, default_value_t = 24.0)]
        since_hours: f64,
        /// Borne dure sur le nombre d'URLs soumises (évite de flooder IndexNow).
        #[arg(long, default_value_t = 10_000)]
        max_urls: usize,
    },
    /// Refreshes the disposable mail domains no account opens on.
    DisposableDomains,
    /// Ingère un stock ou un incrément LEGI (référentiel versionné) depuis un
    /// `tar.gz` bulk DILA local (`Freemium_legi_global_*.tar.gz` ou incrément
    /// `LEGI_*.tar.gz`, téléchargé hors-bande).
    Legi {
        /// Chemins locaux des archives `tar.gz` LEGI, dans l'ordre chronologique.
        #[arg(required = true)]
        paths: Vec<std::path::PathBuf>,
        /// Restricts the pass to these text cids, repeatable. A named pass reads each
        /// archive whole and applies no suppression list.
        #[arg(long = "text")]
        textes: Vec<String>,
        /// The archives predate what the base holds, so differing bytes never win:
        /// set it to replay an old global stock, never to feed increments.
        #[arg(long = "stock-ancien")]
        stock_ancien: bool,
    },
    /// Audits a text's chronicles: compares the versions the `VERSIONS` block of
    /// the LEGI files declares to the ones we hold, and qualifies each missing one
    /// (file present, named by a suppression list, or never delivered). Read only.
    AuditLegiVersions {
        /// Cid of the audited text (`LEGITEXT000006073189` = code de la sécurité sociale).
        #[arg(long, required = true)]
        text: String,
        /// LEGI `tar.gz` archives to read (global stock, then increments).
        #[arg(required = true)]
        paths: Vec<std::path::PathBuf>,
    },
    /// Re-applies, from its latest delivered file in the DILA cache, every LEGI and
    /// KALI version the base holds at more than one start date (entry into force
    /// postponed, unknown date fixed later): the delivered date stays, the others
    /// go, the ends they capped reopen. Counts only, unless `--apply`.
    RedateDilaVersions {
        /// Writes (without it: counters, no write).
        #[arg(long)]
        apply: bool,
    },
    /// Sync incrémental LEGI : télécharge les incréments `LEGI_*.tar.gz`
    /// postérieurs au watermark puis les ingère (le stock global se bootstrape
    /// hors-bande via `legi <path>`). Idempotent.
    SyncLegi,
    /// Backfill du graphe `legal_link` : streame des tarballs DILA locaux
    /// (stock global puis incréments, LEGI et/ou KALI, dans l'ordre
    /// chronologique) et remplace les arêtes `<LIENS>` par propriétaire, sans
    /// upsert d'articles (hors gate `content_checksum`). Rejouable ; purge les
    /// arêtes orphelines en fin de run.
    BackfillLinks {
        /// Archives `tar.gz` DILA, dans l'ordre chronologique.
        #[arg(required = true)]
        paths: Vec<std::path::PathBuf>,
    },
    /// Backfill de l'arbre structurel `legal_toc_edge` : streame des tarballs
    /// LEGI locaux (stock global puis incréments, ordre chronologique) et
    /// remplace les arêtes `TEXTELR`/`SECTION_TA` par propriétaire, hors gate
    /// `content_checksum`. Rejouable ; purge les arbres orphelins en fin de run.
    BackfillToc {
        /// Archives `tar.gz` LEGI, dans l'ordre chronologique.
        #[arg(required = true)]
        paths: Vec<std::path::PathBuf>,
    },
    /// Backfill des métadonnées de textes : rejoue l'upsert `legal_text` +
    /// `upcoming_versions` depuis les `TEXTE_VERSION` des tarballs LEGI locaux.
    /// Rejouable.
    BackfillTextes {
        /// Archives `tar.gz` LEGI, dans l'ordre chronologique.
        #[arg(required = true)]
        paths: Vec<std::path::PathBuf>,
    },
    /// Ingère un stock ou un incrément KALI (conventions collectives
    /// nationales, bulk DILA) depuis un `tar.gz` local
    /// (`Freemium_kali_global_*.tar.gz` ou incrément `KALI_*.tar.gz`).
    /// KALICONT → `legal_text`, KALIARTI → `legal_article` (ancrés sur la
    /// convention). Idempotent.
    Kali {
        /// Chemins locaux des archives `tar.gz` KALI, dans l'ordre chronologique.
        #[arg(required = true)]
        paths: Vec<std::path::PathBuf>,
        /// Restricts the pass to these container cids, repeatable. A named pass reads
        /// each archive whole and applies no suppression list.
        #[arg(long = "text")]
        textes: Vec<String>,
        /// The archives predate what the base holds, so differing bytes never win:
        /// set it to replay an old global stock, never to feed increments.
        #[arg(long = "stock-ancien")]
        stock_ancien: bool,
    },
    /// Sync incrémental KALI : cold start télécharge le stock global puis les
    /// incréments `KALI_*.tar.gz` postérieurs au watermark, chacun ingéré.
    /// Idempotent.
    SyncKali,
    /// Fait passer les conventions collectives à la clé qualifiée par la pièce :
    /// requalification sur place puis entrée des articles que l'ancienne clé
    /// refusait, un lot de recueils par passage. Reprenable et idempotent.
    RequalifierKali {
        /// Recueils traités par passage. Borne la mémoire du parcours du cache.
        #[arg(long, default_value_t = 200)]
        lot: i64,
    },
    /// Moisson quotidienne du fond marchés du pot Annonces : les 14 canaux
    /// (BOAMP Opendatasoft, TED, 11 moteurs de profils d'acheteurs, et le dump
    /// de presse quand une session en a laissé un) ré-énumérés en entier,
    /// réconciliés en lignes canoniques, upsertés. Aucun état stampé ni
    /// supprimé : la moisson n'écrit que ce qu'elle voit, la fenêtre vivante se
    /// calcule à la lecture. Idempotent.
    Annonces {
        #[arg(
            long,
            default_value_t = lj_sources_annonces::centraledesmarches::FENETRE_JOURS,
            help = "Jours de publication parcourus chez la Centrale des marchés. Le défaut borne la quotidienne à ce qui a paru depuis la veille ; un rattrapage manuel l'élargit, à 14 pages par jour demandé."
        )]
        centrale_jours: i32,
        #[arg(
            long,
            help = "Reprend les tenants silencieux depuis sept nuits, que la quotidienne écarte. Réservé à l'hebdomadaire : une plateforme rendue est ainsi retrouvée en sept jours au pire, sans que la quotidienne paie ses sondes tous les soirs."
        )]
        relancer_muets: bool,
    },
    /// Mesure la complétude de la moisson des plateformes : les consultations
    /// ouvertes que seuls les canaux de publication portent (BOAMP, TED,
    /// presse), classées par tenant et par cause — tenant non recensé, moteur
    /// non connecté, ou faux négatif d'un connecteur. Rapport pur, n'écrit rien.
    AnnoncesCouverture {
        /// Tenants détaillés dans le rapport, les plus gros d'abord.
        #[arg(long, default_value_t = 40)]
        limite: usize,
    },
    /// Backfill de l'historique BOAMP dans le fond `marche` : mois par mois,
    /// les avis de marché puis les résultats de la même fenêtre, depuis 2010.
    /// Manuel, jamais planifié. Idempotent et reprenable : relancer sur une
    /// fenêtre déjà passée ne réécrit rien.
    AnnoncesBackfill {
        /// Premier mois inclus (`YYYY-MM-DD`, ramené au 1er du mois). Défaut :
        /// 2010-01-01, le plancher du dataset. Après une coupure, reprendre au
        /// dernier `mois` du journal.
        #[arg(long)]
        depuis: Option<String>,
        /// Borne de fin exclue (`YYYY-MM-DD`).
        #[arg(long)]
        jusqu: String,
    },
    /// Fills the description, the lots, the award criteria and the estimated
    /// value on rows already written, by re-reading the BOAMP export month by
    /// month. Touches neither identity nor channels. Manual, never scheduled.
    AnnoncesContenu {
        /// First month included (`YYYY-MM-DD`, pulled back to the 1st). Defaults
        /// to 2010-01-01, the floor of the dataset.
        #[arg(long)]
        depuis: Option<String>,
        /// Excluded end bound (`YYYY-MM-DD`).
        #[arg(long)]
        jusqu: String,
    },
    /// Enrichit les annonces ouvertes par leur page de détail : le code CPV et
    /// la procédure, que les 11 moteurs de scraping ne portent pas en liste.
    /// File dérivée (ouvertes, sans bloc `detail`, sans canal BOAMP), une page
    /// par annonce et une seule fois dans sa vie. Quotidien. Idempotent.
    AnnoncesEnrichir {
        /// Pages tirées au plus par run.
        #[arg(long, default_value_t = 2_000)]
        limite: i64,
        /// Minutes accordées à chaque moteur. Le défaut borne la quotidienne ; un
        /// rattrapage manuel monte le plafond pour vider une file rationnée d'une
        /// traite, à la cadence de son hôte.
        #[arg(long, default_value_t = 15)]
        budget_min: u64,
        /// Rattrapage : tire les annonces sans libellé d'acheteur au lieu des
        /// non visitées. Rouvre les fiches lues avant que leur extracteur ne
        /// sache lire l'acheteur (Atexo). Manuel, jamais planifié.
        #[arg(long, default_value_t = false)]
        sans_acheteur: bool,
        #[arg(
            long,
            default_value_t = false,
            help = "Rattrapage : tire les annonces sans zone au lieu des non visitées. Rouvre les fiches lues avant que leur extracteur ne sache lire le lieu d'exécution. Manuel, jamais planifié."
        )]
        sans_zone: bool,
        #[arg(
            long,
            default_value_t = false,
            help = "Rattrapage : tire les annonces sans description ni lots ni critères ni montant au lieu des non visitées. Rouvre les fiches lues avant que leur extracteur ne sache lire le contenu de l'avis. Manuel, jamais planifié."
        )]
        sans_contenu: bool,
    },
    #[command(
        about = "Pose les attributions que déclarent les données essentielles de la commande publique",
        long_about = "Pose les attributions que déclarent les données essentielles de la commande publique. Quotidien. Sort sans rien télécharger quand le catalogue sert les trois dates de publication de la dernière passe écrite et qu'aucune fenêtre plus large n'est demandée. Idempotent."
    )]
    AnnoncesDecp {
        #[arg(
            long,
            help = "Début de la fenêtre de notification (`YYYY-MM-DD`). Le défaut couvre les 180 derniers jours, ce qu'une passe quotidienne rattrape du retard que les acheteurs prennent à déclarer ; un rattrapage manuel remonte à 2018, où commence l'obligation."
        )]
        depuis: Option<String>,
        #[arg(
            long,
            default_value_t = false,
            help = "Écrit les attributions. Sans ce drapeau, la commande compte et montre ce qu'elle poserait."
        )]
        applique: bool,
    },
    /// Recense les profils d'acheteurs cités par les avis BOAMP actifs et les
    /// classe par moteur (`profil_acheteur`) — les moteurs multi-tenants
    /// (Atexo, Safetender, Modula) en dépendent pour savoir quels hôtes énumérer.
    /// Hebdomadaire. Idempotent.
    AnnoncesDiscover,
    #[command(
        about = "Sonde les liens que les listes affichent et classe ce que la porte sert",
        long_about = "Tire un échantillon des liens que les listes affichent, ouvre chacun et classe ce que la porte sert : le fichier, le formulaire de retrait, la fiche, l'accueil de la plateforme, un rationnement de l'appelant, un lien mort. Verdict du jour dans `annonce_lien_sonde`. Quotidien. Idempotent."
    )]
    AnnoncesSonderLiens {
        #[arg(
            long,
            default_value_t = 8,
            help = "Liens tirés par moteur et par nature de lien"
        )]
        par_moteur: i64,
    },
    /// Tient le stock `marche` en conformité : chaque ligne orpheline (uid
    /// abandonné par un re-groupage, plus aucun canal) est résolue par ses
    /// `details.liens` vers la ligne qui porte aujourd'hui ses canaux, puis
    /// absorbée (redirection 301 + suppression). Étape quotidienne de la chaîne.
    /// Idempotent.
    AnnoncesRegulariser {
        /// Réapplique aussi les règles de forme (titres, acheteurs, lots
        /// d'attribution) à tout le stock — le filet hebdomadaire qui porte
        /// une règle nouvelle au stock clos, que la moisson ne revisite pas.
        #[arg(long)]
        formes: bool,
    },
    /// Va chercher sur la plateforme, par la référence de marché que l'avis
    /// publie, les consultations des lignes `marche` ouvertes qui n'ont que la
    /// racine d'un hôte. L'énumération d'une instance et son index de recherche
    /// ne couvrent pas le même ensemble : une consultation absente de
    /// la liste sort de la recherche. Les lignes trouvées entrent par le chemin
    /// d'écriture de la moisson ; c'est `annonces-jumeler` qui les rattache.
    /// **Dry-run par défaut**. Idempotent.
    AnnoncesResoudre {
        /// Écrit réellement les lignes trouvées. Sans ce flag : dry-run.
        #[arg(long)]
        applique: bool,
        /// Plafond de références interrogées (0 = pas de plafond).
        #[arg(long, default_value_t = 0)]
        limite: i64,
    },
    /// Fusionne par leur jumelle les lignes `marche` ouvertes que seul le
    /// BOAMP publie et dont l'avis déclare une plateforme de dépôt **sans
    /// identifiant de dossier** : sans clé de jointure, la seule attache est
    /// l'annonce que cette plateforme publie sous les mêmes titre et acheteur.
    /// La ligne plateforme est l'élue, la ligne BOAMP est absorbée (redirection
    /// 301 + suppression). **Dry-run par défaut** : sans `--applique`, la passe
    /// journalise les paires et n'écrit rien. Idempotent.
    AnnoncesJumeler {
        /// Applique réellement les fusions (write). Sans ce flag : dry-run.
        #[arg(long)]
        applique: bool,
    },
    /// Ingère un export JSONL `bofip-vigueur` local (doctrine fiscale DGFiP) :
    /// document BOI → `legal_text` (nature=BOFIP, préambule en `body`), §
    /// numérotés → `legal_article`. Idempotent (snapshot : purge des versions/§
    /// hors export).
    Bofip {
        /// Chemin local de l'export `bofip-vigueur.jsonl`.
        #[arg(required = true)]
        path: std::path::PathBuf,
    },
    /// Sync BOFiP : re-télécharge l'export open data `bofip-vigueur`
    /// (data.economie.gouv.fr, snapshot des publications en vigueur) puis
    /// l'ingère. Idempotent.
    SyncBofip,
    /// Sync du fond DILA CIRCULAIRES : stocks 2009-2014 + abrogations
    /// historiques + flux chronologiques (last-write-wins par ID) →
    /// `legal_text` nature=CIRCULAIRE (métadonnées + résumé ; corps = passe
    /// séparée sync-circulaires-bodies). Reprend au dernier fichier ingéré
    /// (manifest). Idempotent.
    SyncCirculaires,
    /// Passe corps des circulaires : stocks PDF + PDF compagnons des flux
    /// (last-write-wins par ID) → `legal_text.body` (pdftotext natif, OCR
    /// Mistral cache-first en repli scanné). Reprend au dernier tarball rejoué
    /// (manifest pdf). Idempotent.
    SyncCirculairesBodies,
    /// Ministerial bulletins: the circulars, instructions and notes each ministry published on its own site since 2019 and the DILA fonds lacks, written to `legal_text` with their PDF body. Lists the current and previous year.
    SyncBulletins {
        /// One site only.
        #[arg(long, value_parser = lj_sources::bulletins::BulletinSite::parse)]
        site: Option<lj_sources::bulletins::BulletinSite>,
        /// Every year since 2019.
        #[arg(long)]
        full: bool,
        /// Lists and counts, writes nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Sync of the Service-Public.fr « vos droits » sheets: both feeds
    /// re-downloaded, every sheet upserted with its Légifrance edges, the ones
    /// gone from the feeds turned `ABROGE`. Idempotent.
    SyncServicePublic,
    /// Corps des traités (TRAITE) et accords interprofessionnels (TI) depuis
    /// les blocs sans numéro des stocks bulk JORF/KALI du cache :
    /// concaténation dans l'ordre du document → `legal_text.body` des fiches
    /// sans articles ni corps. Aucun téléchargement. Idempotent.
    BackfillTreatyBodies,
    /// Sync ArianeWeb, les trois fonds en un job : le recensement (une requête
    /// par fond, facette d'année comptée serveur) désigne les décisions CE que le
    /// fonds n'a pas et les années dont les commentaires ont bougé, et rien
    /// d'autre n'est visité. Cache HTML sous state_dir. Idempotent.
    SyncAriane {
        /// Années à traiter (défaut : celles que le recensement désigne).
        #[arg(long, value_delimiter = ',')]
        years: Vec<u16>,
        /// S'arrêter au tableau du recensement, sans rien télécharger.
        #[arg(long)]
        census_only: bool,
        /// Plafond de documents AJCE téléchargés (sonde) : le run s'arrête
        /// proprement sans marquer d'année au manifeste.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Sync des grandes décisions que le Conseil d'État publie : le titre de
    /// l'entrée nomme la décision, le lien ArianeWeb de sa fiche l'ancre
    /// → `decisions.case_name` en `case_name_source = 'ce-list'`. Cache des
    /// fiches sous state_dir. Idempotent.
    SyncCeList,
    /// Sync of the usage names French Wikipedia and Wikidata give: decision
    /// pages, the list of the major administrative decisions, the Wikidata items
    /// carrying an ECLI → `decisions.case_name` as `case_name_source =
    /// 'wikimedia'`, and `decisions.wikidata_id`. Idempotent.
    SyncWikiNames,
    /// Sync of the usage names HAL's French law deposits write beside a reference given by
    /// number and day → `decisions.case_name` as `case_name_source = 'doctrine'`.
    SyncDoctrineNames {
        /// The last 30 days, or every deposit not read yet.
        #[arg(long, value_enum, default_value_t = pipeline::DoctrineNamesScan::Weekly)]
        scan: pipeline::DoctrineNamesScan,
        /// Deposits read at most.
        #[arg(long)]
        limit: Option<usize>,
        /// Read and elect without writing, what would change on stdout.
        #[arg(long)]
        dry_run: bool,
    },
    /// Sync HAL: reads the open archive's recent law deposits and attaches
    /// those citing a docket number as `kind:"note"` commentaries (outbound
    /// links). Idempotent, not searchable.
    SyncHal {
        /// Scan depth: 30 days, 180 days, or the whole fonds.
        #[arg(long, value_enum, default_value_t = pipeline::HalScan::Daily)]
        scan: pipeline::HalScan,
    },
    /// Sync of the law journals and blogs harvested over OAI-PMH: a note writing
    /// a cassation docket becomes a `kind:"note"` link. Idempotent.
    SyncDoctrineOai {
        /// Datestamp depth: 30 days, 180 days, or every record.
        #[arg(long, value_enum, default_value_t = pipeline::DoctrineScan::Daily)]
        scan: pipeline::DoctrineScan,
        /// Writes nothing and logs every link it would write.
        #[arg(long)]
        dry_run: bool,
    },
    /// Charge les commentaires de norme curés depuis
    /// `<state_dir>/ingest/article-commentaires.json` → `article_commentaire`
    /// (liens `kind:"note"` sur des articles de loi/traité). Entrées ancrées par
    /// `slug` d'URL, résolues et vérifiées à chaque passage ; les entrées
    /// absentes du dataset sont tombstonées. Idempotent, réconciliant.
    SeedArticleCommentaires,
    /// Extrait les renvois norme→article (text_legal_citation) et les
    /// citations de jurisprudence (text_case_citation) de TOUS les corps du
    /// référentiel — versions d'articles + corps monolithiques — en une passe
    /// doc_extract. Idempotent.
    ExtractTextRefs,
    /// Ingère un stock ou un incrément JORF (Journal officiel, bulk DILA)
    /// depuis un `tar.gz` local (`Freemium_jorf_global_*.tar.gz` ou incrément
    /// `JORF_*.tar.gz`). Deux passes : textes (`referential_texts`, tag
    /// `treaty`/`jorf`) puis articles numérotés (`referential_articles`).
    /// Idempotent.
    Jorf {
        /// Chemins locaux d'archives `tar.gz` JORF (traitées dans l'ordre,
        /// finisseurs une fois en fin de lot).
        #[arg(required = true)]
        paths: Vec<std::path::PathBuf>,
    },
    /// Sync du fond JORF complet (fiches + rôles + liens) : télécharge
    /// stock/incréments DILA postérieurs au watermark puis les ingère (mécanique
    /// `sync-legi`). Idempotent, reprenable.
    SyncJorf,
    /// Writes the text of the unnumbered JORF articles as the body of the texts
    /// with neither body nor article, from every cached JORF archive. Replayable.
    JorfBodies,
    /// Replays every cached JORF archive into the JO issues and the publication
    /// of each text. Replayable.
    JorfPublications,
    /// Cuts the articles of every empty treaty record out of the decree that
    /// publishes it. Replayable.
    TreatyArticles {
        /// Prints what each treaty would get, writes nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Rewrites the article texte and nota, and the JORF bodies, that an unchanged
    /// cached DILA member reads differently, with their citation spans. Replayable.
    BackfillArticleText {
        /// One fond; the three when absent.
        #[arg(long, value_enum)]
        fond: Option<pipeline::ArticleFond>,
        /// Counts what would change and writes nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Assigne les slugs manquants de `legal_text` : backfill puis no-op (la
    /// passe tourne aussi en fin de chaque ingest référentiel).
    AssignSlugs,
    #[command(
        about = "Retire des slugs étrangers le suffixe pays alpha-3 que le premier segment d'URL porte désormais, et lègue chaque ancienne adresse en redirection. Rejouable : un slug déjà nu est laissé tel quel"
    )]
    DropCountrySuffix,
    #[command(
        about = "Renames every text slug the cascade no longer mints from its text, each old address kept as a redirect. Dry-run without --applique; replays as a no-op"
    )]
    RemintTextSlugs {
        #[arg(long)]
        applique: bool,
    },
    #[command(
        about = "Audits the section paths of every text at today's date: sections addressed, merged, and those that still needed an ordinal. Reads only"
    )]
    AuditTocPaths,
    #[command(
        about = "Range sous EU les textes de la pile internationale que l'Union édicte : son droit primaire et ce qui le modifie, et ce que ses États membres signent entre eux sur la base de ses traités. Rejouable"
    )]
    RefileUnionTexts,
    /// Recalcule `legal_text.role` (classification v1 conservatrice) et aligne
    /// `legal_link.verb` sur le repli verbe/nom courant. Idempotent.
    BackfillTextRoles,
    /// Restamps every text's date and jurisdiction paths, then its articles'.
    /// Idempotent; whatever it rewrites is reported.
    StampTexts,
    /// Déménage la chronique LEGI d'un instrument consolidée par DILA sous le
    /// cid du véhicule : lignes LEGIARTI + arêtes TOC vers la fiche instrument
    /// (arête publie). Liste fermée vérifiée à la main ; re-runnable après une
    /// retouche DILA du dossier.
    MoveChronicle {
        /// Uid JORFTEXT du véhicule dont le dossier LEGI porte la chronique.
        #[arg(long)]
        vehicle: String,
        /// Couples `<num_key>@<YYYY-MM-DD>` qui restent au véhicule (jumeaux
        /// LEGI du dispositif dans un dossier mixte). Répétable.
        #[arg(long)]
        exclude: Vec<String>,
    },
    /// Re-frappe des uids de fiches par dataset JSONL
    /// `{from, to, jurisdiction?}` (droit étranger par ordre juridique, stubs
    /// EUR-Lex → EU/). Cascade articles/TOC/liens/citations texte, slug
    /// inchangé ; suivre du drain reextract-fields --full --field
    /// legal_references. Résumable.
    RenameTexts {
        /// Chemin du dataset JSONL de re-frappe.
        dataset: std::path::PathBuf,
    },
    /// Rebase les clés d'article (num_key et colonnes liées) vers la clé
    /// publique slug. One-shot, idempotent.
    RekeyArticleKeys,
    /// Rebase `legal_article.num_key` (+ liens possédés) et
    /// `legal_toc_edge.child_num_key` vers la clé d'IDENTITÉ recalculée depuis
    /// le num source. One-shot, idempotent.
    RekeyIdentityKeys,
    /// Retire l'astérisque Légifrance (`R*600-1`) des clés d'article, colonnes
    /// porteuses et blobs de citation compris ; `num` la garde. **Dry-run par
    /// défaut**. Idempotent.
    DestarArticleKeys {
        /// Applique réellement le désétoilage. Sans ce flag : dry-run.
        #[arg(long)]
        applique: bool,
    },
    LoadLegalCorpus {
        /// Restreint le chargement aux datasets dont le nom contient cette sous-chaîne
        /// (chargement chirurgical, ex. `--only eu-rproc`). Défaut : tous les datasets.
        #[arg(long)]
        only: Option<String>,
    },
    /// Génère les datasets catalogue EUR-Lex (règlements/directives UE cités
    /// mais absents) sous `<state_dir>/ingest/corpus/`, via SPARQL Cellar,
    /// pilotés par les slashnums réellement cités (spans non liés de
    /// `legal_citation`). Entrées catalogue-seul (`articles: []`) ; auto-validées
    /// (le titre FR doit porter le slashnum cité). À enchaîner avec
    /// `load-legal-corpus` ; les citations s'attachent à la passe intégrale
    /// suivante. `--limit` borne aux N slashnums les plus cités (rodage).
    /// Idempotent (skip datasets présents).
    IngestEuCatalog {
        #[arg(long)]
        limit: Option<usize>,
    },
    /// EUR-Lex registry: every regulation and directive (sector 3, base,
    /// delegated and implementing acts) enters the catalogue, cited or not, as
    /// a catalogue-only entry loaded right away. Delta by default, on the acts
    /// CELLAR registered since the watermark of the eu-law manifest; `--full`
    /// sweeps the whole registry, which is also the bootstrap when the
    /// watermark is absent, and stamps `ABROGE` on the acts with no article
    /// whose end of validity is past. Idempotent.
    SyncEuCatalog {
        #[arg(long)]
        full: bool,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Gives their CELLAR text, French HTML else the PDF's text layer, to the
    /// European acts with neither body nor article. Replayable.
    EuBodies {
        #[arg(long)]
        limit: Option<usize>,
        /// Prints each body's length and head, writes nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Third door onto the EU corpus, the one the French JO opens: every DILA
    /// fiche of European nature is a shell, an act named with no body. Each is
    /// paired with its EUR-Lex act by the number its title carries, that act is
    /// minted at the catalogue when the corpus lacks it, then the shell
    /// collapses into it, carrying over the date and the NOR that EUR-Lex does
    /// not publish and leaving its slug as a redirect. `--limit` bounds the pass
    /// (rodage). Idempotent, the merged shells leaving the source query.
    SyncEuJorf {
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Merges the `EU/` fiches an old ingest left under two uids, number and year
    /// swapped, the CELEX their articles carry naming the one that survives. The
    /// other keeps its slug as a redirect, and its body moves when it is the one
    /// that has it. Dry run listing the pairs unless `--apply`. One-off: no code
    /// produces the swapped form any more.
    MergeEuSwappedUids {
        #[arg(long)]
        apply: bool,
    },
    /// Daily sync of the EU fund: every act of the registry (`eu-*.json`
    /// datasets) carries its whole consolidated series, sourced as Formex 4
    /// through CELLAR (`.FRA.fmx4` manifestation, cellar item). A version is
    /// written per change of body, not per consolidation. Each pass asks the
    /// consolidated versions of every act with a body by SPARQL, and probes
    /// those the dataset has not read. An act whose dataset already lists every
    /// applicable snapshot is skipped, unless the DB lags its last written
    /// version (run interrupted between write and load). Acts with no Formex
    /// keep their stock. `--limit` bounds to the first N acts, `--only` filters
    /// by dataset name substring (both leave the manifest alone), `--force`
    /// refetches the whole series and reloads. A pass also probes a batch of
    /// the acts never visited; `--backlog` probes every act never visited, the
    /// stock catch-up run by hand, with `--concurrency` CELLAR fetches at once.
    SyncEuLaw {
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        only: Option<String>,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        backlog: bool,
        #[arg(long, default_value_t = 4)]
        concurrency: usize,
    },
    /// Ingestion catalogue des règlements de procédure des juridictions UE
    /// (Cour de justice, Tribunal, ex-TFP). Écrit des datasets catalogue curés
    /// (variantes = alias TSV du linker) ; enchaîner avec `load-legal-corpus`.
    /// `--dry-run` liste sans écrire. Idempotent.
    IngestEuRproc {
        #[arg(long)]
        dry_run: bool,
    },
    /// Rafraîchit la fraîcheur « as-of » des sources *live* autoritaires
    /// (legifrance/kali/jorf) à la date du jour dans `ingest_freshness` :
    /// re-confirme que la copie reflète le droit en vigueur. À lancer après
    /// chaque ingest LEGI/KALI/JORF (cron). Idempotent (une ligne par source).
    StampFreshness,
    /// Joue les invariants de fin de chaîne : des contrôles de données qui
    /// publient leur nombre de lignes fautives dans `ingest.invariant.breach`
    /// et nomment chaque fautif au journal. Ne fait jamais échouer la chaîne.
    Invariants,
    /// Recalcule l'ordre de lecture (`legal_article.reading_rank`) des textes
    /// sales — ceux dont un article a un rang `NULL`. Appelé en fin de chaque
    /// pipeline qui écrit des articles ; en manuel pour le rattrapage.
    /// Idempotent : une seconde passe n'écrit rien.
    ReadingRanks,
    /// Reclasse les `source` catégorie/méthode hérités (`treaty`, `eu-law`,
    /// `official-fr`, `traduction-automatique`) en libellés de diffuseur réels,
    /// dérivés de `source_url`. Idempotent ; lignes sans URL hors bulk JORF
    /// laissées au reload de curation (loguées). One-shot après déploiement.
    RelabelSources,
    /// Canonicalise les variantes d'hôte d'un diffuseur (`jafbase.fr` →
    /// `jafbase`) vers son libellé unique. Idempotent. One-shot
    /// post-déploiement.
    CanonicalizeSourceLabels,
    /// Ingère un fond bulk DILA (`tar.gz` locaux sous
    /// `<cache>/dila/<fond>/tarballs/`) : `jade` (admin CE/CAA/TC), `constit`
    /// (Conseil constitutionnel). Idempotent ; dédup par `canonical_ref`.
    IngestDila {
        /// Fond DILA à ingérer.
        #[arg(value_enum)]
        fond: pipeline::Fond,
    },
    /// Rewrites the decision texts an unchanged cached JADE, CONSTIT or CNIL member
    /// reads differently, and the CJUE texts holding a reference the strip now
    /// decodes or a table of contents, re-extracts their fields and leaves their passages to
    /// `embed-missing`. Replayable.
    BackfillDecisionText {
        /// One fond; all four when absent.
        #[arg(long, value_enum)]
        fond: Option<pipeline::TextFond>,
        /// Counts what would change and writes nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Recases the French decision texts stored in capitals, as the ingest now
    /// writes them, re-extracts the fields of the decisions read in them and
    /// leaves their passages to `embed-missing`. Replayable.
    RecaseTexts {
        /// Counts what would change and writes nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Writes where the moyens annexés of each stored text start. Replayable.
    AnnexStarts,
    /// Tags the passages of the decisions whose text carries the court's
    /// summary. Replayable.
    HeadnoteTags,
    /// Sync incrémental d'un fond DILA (télécharge les incréments postérieurs
    /// au watermark). N'ingère rien — `ingest-dila` consomme les archives.
    SyncDila {
        /// DILA funds to sync, in this order.
        #[arg(value_enum, required = true, num_args = 1..)]
        fonds: Vec<pipeline::Fond>,
    },
    /// Backfill de la colonne `decisions.ecli` depuis
    /// `source_fields->>'ecli'` : expose l'ECLI sur les lignes historiques
    /// (Judilibre le porte dans son payload, colonne NULL).
    /// Keyset batché, idempotent.
    BackfillEcli,
    /// Construit le vocabulaire d'autocomplétion : n-grammes 1-3 comptés sur
    /// un échantillon des décisions + articles + titres, FST déposé en blob dans
    /// `suggest_index`. Relançable (remplace le blob).
    BuildSuggest,
    /// Re-dérive `decisions.canonical_ref`. Sans `--force` : ne traite que les
    /// `NULL` (peuplement). Avec `--force` : **re-dérive toutes** les décisions
    /// (`full_text` présent) pour migrer les clés 3-champs historiques
    /// `{nom}|{rg}|{date}` en 4-champs `{type}|{location}|{rg}|{date}`. Pur
    /// calcul, aucun re-embed ; une clé `None` laisse l'existante (jamais
    /// d'écrasement vers `NULL`).
    BackfillCanonicalRef {
        /// Re-dérive toutes les décisions (pas seulement `canonical_ref IS NULL`).
        #[arg(long)]
        force: bool,
    },
    /// Fusion rétroactive des **faux splits cross-source** : décisions
    /// distinctes au même `canonical_ref` avec sources **disjointes** = même
    /// décision multi-sources. Garde l'autorité (rang max), fusionne les autres
    /// dedans. Jamais de fusion intra-source. Aucun re-embed. Idempotent.
    MergeCrossSourceDuplicates,
    /// Absorbs each Judilibre record holding no ruling into the decision of the
    /// same court, day and chamber that joins its number.
    AbsorbCopies {
        /// Reads the court days Judilibre fed in the last N days.
        #[arg(long)]
        days: i32,
    },
    /// Read-only: recomputes `canonical_ref` per provenance on all-judilibre multi-provenance
    /// decisions and counts legitimate, ambiguous and false merges (diverging keys).
    AnalyzeFalseMerges,
    /// Writes the courts the referential holds by their own name, closed ones
    /// and their successors, from the versioned table. Replays without writing.
    SeedJurisdictions,
    /// Read only: resolves each provenance ingested in the last `--days` days as
    /// if it arrived now, and writes to `--out` every resolution that differs
    /// from where it sits.
    ProbeIdentity {
        #[arg(long, default_value_t = 30)]
        days: i32,
        /// `decision_sources.source` (`judilibre`, `opendata`…), repeatable; all
        /// by default.
        #[arg(long)]
        source: Vec<String>,
        #[arg(long)]
        out: std::path::PathBuf,
        /// Moves the provenances alone on their decision to where they resolve.
        #[arg(long)]
        apply: bool,
    },
    /// Bootstrap CEDH (HUDOC) : balaye toutes les années, fetch les arrêts FR
    /// (HFJUD) et les ingère. Idempotent par `content_checksum`.
    IngestCedh {
        /// Première année balayée (défaut : 1960). Backfill ciblé d'un trou
        /// d'ingest sans re-lister le fonds ancien.
        #[arg(long)]
        from_year: Option<i32>,
    },
    /// Réchauffe le cache disque des corps CEDH (`<cache_dir>/cedh/bodies/`) sans
    /// toucher la base — découple le fetch réseau throttlé de l'ingest. Ensuite
    /// `ingest-cedh` relit le cache (sans réseau), seul l'embedding reste.
    CacheCedh {
        /// Première année réchauffée (défaut : 1960).
        #[arg(long)]
        from_year: Option<i32>,
    },
    /// CJUE bootstrap (EUR-Lex, then Curia): sweeps every year from 1954 and ingests
    /// the judgments, orders and opinions in their languages of procedure, French and
    /// English. Idempotent.
    IngestCjue {
        /// Reads again every text held under 6,000 bytes, which may be EUR-Lex's summary.
        #[arg(long)]
        short: bool,
    },
    /// Sync incrémental CEDH : ne balaye que l'année courante.
    SyncCedh,
    /// Sync incrémental CJUE : ne balaye que l'année courante.
    SyncCjue,
    /// Pastes the CJUE's and the CEDH's own summaries on the texts of their decisions,
    /// each in its language. Idempotent.
    SyncCourtSummaries {
        /// First year swept, every year by default.
        #[arg(long)]
        from_year: Option<i32>,
        /// Last year swept, the current one by default.
        #[arg(long)]
        to_year: Option<i32>,
    },
    /// Catches the CJUE corpus up on joined cases: sweeps
    /// `case-law_joins_case_court` at CELLAR and poses it in `source_fields`.
    /// Poses no docket number: chain `reextract-fields --jurisdiction-type CJUE
    /// --field docket_numbers`, which reads them back.
    BackfillCjueJoinedCases {
        #[arg(long)]
        dry_run: bool,
    },
    /// Catches the CJUE corpus up on the CELLAR French title, whose second field
    /// names the parties. Poses no name: chain `reextract-fields
    /// --jurisdiction-type CJUE --field case_name`, then `--field search_title`.
    BackfillCjueTitles {
        #[arg(long)]
        dry_run: bool,
    },
    /// Bootstrap CNDA (Cour nationale du droit d'asile) : crawl la liste
    /// jurisprudentielle paginée de `cnda.fr`, fusionne fiche HTML + PDF lié par
    /// numéro et les ingère (`source_uid = cnda/<numero>`, ECLI fabriqué).
    /// Idempotent par `content_checksum`. `--mode all` force la ré-extraction
    /// complète (re-OCR + ré-chunk/ré-embed) de toutes les décisions, même à
    /// PDF inchangé. `--only <numéro>` (répétable) : ne ré-ingère que ces
    /// décisions précises — crawl la liste mais ne traite/persiste que les
    /// numéros visés (re-OCR forcé, ne touche pas le watermark).
    IngestCnda {
        #[arg(long, value_enum, default_value_t = pipeline::IngestMode::MissingHash)]
        mode: pipeline::IngestMode,
        #[arg(long)]
        only: Vec<String>,
    },
    /// Sync incrémental CNDA : reprend le crawl à la page suivant le
    /// watermark.
    SyncCnda,
    /// The Défenseur des droits decisions held in the cache, written without reading its catalogue.
    IngestDdd {
        #[arg(long, value_enum, default_value_t = pipeline::IngestMode::MissingHash)]
        mode: pipeline::IngestMode,
    },
    /// The newest page of the Défenseur des droits decisions list, every page with `--full`: fetches the notices the cache lacks, then writes the decisions listed.
    SyncDdd {
        #[arg(long)]
        full: bool,
    },
    /// The Autorité de la concurrence decisions of its data.gouv dataset, when a new file is published; `--mode all` writes the file held again.
    SyncAdlc {
        #[arg(long, value_enum, default_value_t = pipeline::IngestMode::MissingHash)]
        mode: pipeline::IngestMode,
    },
    /// The European Commission's competition decisions of the documents published in the last week, of every document with `--full`.
    SyncComeu {
        #[arg(long)]
        full: bool,
    },
    /// The prefectures' recueils des actes administratifs: lists what Attrap dates since `--since`, then reads the queue newest first, each PDF from its prefecture through the egress proxy, the scanned pages through the `mistral-vibe` pool.
    Raa {
        /// First day listed, two weeks back by default.
        #[arg(long)]
        since: Option<chrono::NaiveDate>,
        /// Seconds between two requests to the prefectures, all of them together.
        #[arg(long, default_value_t = 20)]
        pace: u64,
        /// Stops reading after this many minutes.
        #[arg(long)]
        minutes: Option<u64>,
        /// Lists, reads nothing.
        #[arg(long)]
        list_only: bool,
    },
    /// BODACC or JOAFE from the DILA bulk: mirrors the published files, then writes each container not yet written or changed since.
    Gazette {
        #[arg(value_parser = pipeline::parse_gazette_source)]
        source: lj_gazette::Source,
        /// The whole history, not the current year alone.
        #[arg(long)]
        stock: bool,
        /// Reads the local mirror as it stands, without fetching.
        #[arg(long)]
        offline: bool,
        /// Writes nothing: checks every notice comes back whole from its record, and prints what the specs leave in the rest.
        #[arg(long, conflicts_with = "verify")]
        dry_run: bool,
        /// Rebuilds every stored notice from its row and compares it with the source.
        #[arg(long, conflicts_with = "reclass")]
        verify: bool,
        /// Reads every container again and rewrites the family, action or kind of each notice whose rules now give another.
        #[arg(long, conflicts_with = "dry_run")]
        reclass: bool,
    },
    /// The JuriCAF decision pages of the fonds no primary source serves that appeared or changed since the last run, read from the sitemaps modified since and the country feeds, two seconds apart, each batch loaded then written to the folder; a 403, 429 or 503 stops it.
    SyncJuricaf {
        /// A slug head, `SENEGAL-` or `LUXEMBOURG-TRIBUNALADMINISTRATIF-`, repeatable.
        #[arg(long = "prefix")]
        prefixes: Vec<String>,
        /// Reads every sitemap whole and fetches every page the folder lacks, with no time budget.
        #[arg(long)]
        full: bool,
        /// Counts the sitemaps' slugs by country and court, and fetches no page.
        #[arg(long, conflicts_with_all = ["prefixes", "full"])]
        list: bool,
    },
    /// Loads the JuriCAF pages fetched by `sync-juricaf`, under `<state_dir>/ingest/juricaf-probe/html/`.
    JuricafProbe {
        /// Folder of the HTML pages, `<state_dir>/ingest/juricaf-probe/html` by default.
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
        #[arg(long, value_enum, default_value_t = pipeline::IngestMode::MissingHash)]
        mode: pipeline::IngestMode,
    },
    /// Backfill `article_scope` depuis les compteurs d'applicabilité
    /// outre-mer : `etend`/`ecarte` vers NC/PF/WF/TF, version cible résolue par
    /// l'ancre de rédaction ou gelée à la date du compteur. Dry-run par défaut.
    ScopeBackfillCompteurs {
        /// Collectivité : PF, NC, WF ou TF.
        #[arg(long, default_value = "PF")]
        collectivite: String,
        /// Restreint à un texte (LEGITEXT…) pour un pilote ciblé.
        #[arg(long)]
        text_uid: Option<String>,
        /// Écrit en base (sinon dry-run).
        #[arg(long)]
        apply: bool,
    },
    /// Backfill Alsace-Moselle : clauses des trois départements →
    /// restreint/ecarte `geo:FR-ALS`, textes entièrement locaux →
    /// `legal_text.default_scope`. Dry-run par défaut.
    ScopeBackfillAlsace {
        /// Écrit en base (sinon dry-run).
        #[arg(long)]
        apply: bool,
    },
    /// Backfill `solution_uid` dans les `source_fields` CNDA : les provenances
    /// écrites avant que le parser ne pose la clé ne la portent pas, et la
    /// ré-extraction rend alors la déduction générique au lieu du dispositif lu.
    /// Recalculé depuis `(full_text, source_fields)`, sans re-télécharger.
    /// Dry-run par défaut.
    BackfillCndaSolution {
        /// Écrit en base (sinon dry-run).
        #[arg(long)]
        apply: bool,
    },
    /// Charge un registre d'entités : SIRENE, RNA, annuaire des avocats (CNB)
    /// ou avocats aux Conseils (`oacc:`, snapshot curé) — découverte data.gouv +
    /// download streamé (bulk) ou lecture du JSON curé sous
    /// `ingest/corpus/registries/`, remplacement de namespace.
    Registries {
        #[arg(value_enum)]
        source: pipeline::RegistrySource,
        /// SIRENE : charge aussi l'historique des dénominations (périodes closes).
        #[arg(long)]
        with_history: bool,
    },
    /// Resolves the pending actor keys against `entity` (after the monthly
    /// registry reload).
    PartiesRelink {
        /// Toutes les passes pleines (dépliage SIRENE/RNA) — one-shot post-reload
        /// de registre. Sans le flag : passe « cabinets normalisés » + recount
        /// annuaire (hebdo).
        #[arg(long)]
        full: bool,
    },
    /// Charge la vérité terrain du quantum en base, à `PC_GOLD_VERSION`. La
    /// source reste les JSON versionnés en LFS. Idempotent.
    LoadQuantumGold {
        /// Répertoire des records, un JSON par décision nommé de son
        /// `source_uid`, barre doublée en tirets bas.
        #[arg(long)]
        dir: std::path::PathBuf,
    },
    /// Two readings of a campaign, JSONL `{source_uid, quantum}`: the first one's
    /// record at `PC_READING_VERSION`, its rows under the publication guards.
    /// Idempotent.
    LoadQuantumReadings {
        /// The two readings, first then second.
        #[arg(long = "lecture", required = true, num_args = 2)]
        lectures: Vec<std::path::PathBuf>,
    },
    /// Deterministic quantum pass, sure mode, over the fonds decisions that name
    /// a bodily-injury poste; written at `PC_EXTRACT_VERSION`, under the manual
    /// revisions. Idempotent.
    QuantumExtract {
        /// Reads only the first N decisions of the fonds (tuning).
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Applicability: the assertions (places, window, version) compiled from the
    /// versions, the text's counters and the repository's default table.
    #[command(subcommand)]
    Assertions(AssertionsCommand),
    /// Maintenance DB (sous-commandes).
    #[command(subcommand)]
    Db(DbCommand),
}

#[derive(Subcommand, Clone)]
enum AssertionsCommand {
    /// Compiles the texts never compiled or modified within `--days` days;
    /// `--full` rereads the whole fund and writes only what changed.
    Compile {
        #[arg(long)]
        full: bool,
        #[arg(long, default_value_t = 3)]
        days: i32,
        /// A single text, whose rows are printed in dry-run.
        #[arg(long)]
        text_uid: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// The three-term control per unfolded place: overlap, gap, inversion, and
    /// the versions without a state. Reports, breaks nothing.
    Control,
    /// Rebuilds `text_place_coverage` and the natures from the assertions
    /// already written, without recompiling. Replayable.
    Coverage,
}

#[derive(Subcommand)]
enum DbCommand {
    /// ANALYZE des tables.
    Analyze,
    /// Maintenance hebdomadaire des tables de recherche : REINDEX TABLE
    /// CONCURRENTLY + VACUUM, sans indisponibilité.
    ReindexSearch,
    /// VACUUM FULL des tables de recherche — repli MANUEL : compacte heap et
    /// TOAST en rendant l'espace à l'OS, au prix d'un lock exclusif par table.
    /// Hors chaîne planifiée.
    VacuumFull,
    /// GC des tokens/clients MCP expirés.
    GcMcp,
    /// Préchauffe le cache (prewarm).
    Prewarm,
    /// Purge les tombstones **orphelins** (retraits totaux : retirés, vidés,
    /// sans provenance active). Dry-run par défaut ; `--apply` pour supprimer.
    PurgeOrphanTombstones {
        /// Supprime réellement. Sans ce flag : compte + échantillon, aucune
        /// suppression.
        #[arg(long)]
        apply: bool,
    },
}

impl Command {
    fn label(&self) -> &'static str {
        match self {
            Command::Chain { .. } => "chain",
            Command::Sync => "sync",
            Command::JudilibreRanges => "judilibre-ranges",
            Command::Analyze => "analyze",
            Command::Migrate => "migrate",
            Command::Credential { .. } => "credential",
            Command::Submissions { .. } => "submissions",
            Command::Renvois => "renvois",
            Command::AliasCandidats { .. } => "alias-candidats",
            Command::UsageTerms { .. } => "usage-terms",
            Command::Ingest { .. } => "ingest",
            Command::Refetch { .. } => "refetch",
            Command::JudilibreReconcile { .. } => "judilibre-reconcile",
            Command::ReextractFields { .. } => "reextract-fields",
            Command::ResolveSeats { .. } => "resolve-seats",
            Command::ResolveFormations { .. } => "resolve-formations",
            Command::EmbedMissing { .. } => "embed-missing",
            Command::PurgeReverses => "purge-reverses",
            Command::Withdraw { .. } => "withdraw",
            Command::GenerateSummaries { .. } => "generate-summaries",
            Command::UpgradeSummariesCommentaries { .. } => "upgrade-summaries-commentaries",
            Command::RecollageSummaries { .. } => "recollage-summaries",
            Command::RecleanSummaries => "reclean-summaries",
            Command::SummaryInputs { .. } => "summary-inputs",
            Command::SummaryFinish { .. } => "summary-finish",
            Command::SummaryBatch { .. } => "summary-batch",
            Command::ResyncLegalArrays => "resync-legal-arrays",
            Command::Precompute => "precompute",
            Command::ResyncJurisdictionCounts => "resync-jurisdiction-counts",
            Command::ResyncCorpusCounts => "resync-corpus-counts",
            Command::RekeyCnb { .. } => "rekey-cnb",
            Command::CnbHistory { .. } => "cnb-history",
            Command::DeriveAffiliations { .. } => "derive-affiliations",
            Command::Reconcile => "reconcile",
            Command::Sitemap { .. } => "sitemap",
            Command::Indexnow { .. } => "indexnow",
            Command::DisposableDomains => "disposable-domains",
            Command::Legi { .. } => "legi",
            Command::SyncLegi => "sync-legi",
            Command::AuditLegiVersions { .. } => "audit-legi-versions",
            Command::RedateDilaVersions { .. } => "redate-dila-versions",
            Command::BackfillLinks { .. } => "backfill-links",
            Command::BackfillToc { .. } => "backfill-toc",
            Command::BackfillTextes { .. } => "backfill-textes",
            Command::Kali { .. } => "kali",
            Command::SyncKali => "sync-kali",
            Command::RequalifierKali { .. } => "requalifier-kali",
            Command::Annonces { .. } => "annonces",
            Command::AnnoncesCouverture { .. } => "annonces-couverture",
            Command::AnnoncesBackfill { .. } => "annonces-backfill",
            Command::AnnoncesContenu { .. } => "annonces-contenu",
            Command::AnnoncesEnrichir { .. } => "annonces-enrichir",
            Command::AnnoncesDecp { .. } => "annonces-decp",
            Command::AnnoncesDiscover => "annonces-discover",
            Command::AnnoncesSonderLiens { .. } => "annonces-sonder-liens",
            Command::AnnoncesRegulariser { .. } => "annonces-regulariser",
            Command::AnnoncesResoudre { .. } => "annonces-resoudre",
            Command::AnnoncesJumeler { .. } => "annonces-jumeler",
            Command::Bofip { .. } => "bofip",
            Command::SyncBofip => "sync-bofip",
            Command::SyncCirculaires => "sync-circulaires",
            Command::SyncCirculairesBodies => "sync-circulaires-bodies",
            Command::SyncBulletins { .. } => "sync-bulletins",
            Command::SyncServicePublic => "sync-service-public",
            Command::BackfillTreatyBodies => "backfill-treaty-bodies",
            Command::SyncAriane { .. } => "sync-ariane",
            Command::SyncCeList => "sync-ce-list",
            Command::SyncWikiNames => "sync-wiki-names",
            Command::SyncDoctrineNames { .. } => "sync-doctrine-names",
            Command::SyncHal { .. } => "sync-hal",
            Command::SyncDoctrineOai { .. } => "sync-doctrine-oai",
            Command::SeedArticleCommentaires => "seed-article-commentaires",
            Command::ExtractTextRefs => "extract-text-refs",
            Command::Jorf { .. } => "jorf",
            Command::SyncJorf => "sync-jorf",
            Command::JorfBodies => "jorf-bodies",
            Command::JorfPublications => "jorf-publications",
            Command::TreatyArticles { .. } => "treaty-articles",
            Command::BackfillArticleText { .. } => "backfill-article-text",
            Command::BackfillDecisionText { .. } => "backfill-decision-text",
            Command::RecaseTexts { .. } => "recase-texts",
            Command::AnnexStarts => "annex-starts",
            Command::HeadnoteTags => "headnote-tags",
            Command::AssignSlugs => "assign-slugs",
            Command::DropCountrySuffix => "drop-country-suffix",
            Command::RemintTextSlugs { .. } => "remint-text-slugs",
            Command::AuditTocPaths => "audit-toc-paths",
            Command::RefileUnionTexts => "refile-union-texts",
            Command::BackfillTextRoles => "backfill-text-roles",
            Command::StampTexts => "stamp-texts",
            Command::MoveChronicle { .. } => "move-chronicle",
            Command::RenameTexts { .. } => "rename-texts",
            Command::RekeyArticleKeys => "rekey-article-keys",
            Command::RekeyIdentityKeys => "rekey-identity-keys",
            Command::DestarArticleKeys { .. } => "destar-article-keys",
            Command::LoadLegalCorpus { .. } => "load-legal-corpus",
            Command::IngestEuCatalog { .. } => "ingest-eu-catalog",
            Command::SyncEuCatalog { .. } => "sync-eu-catalog",
            Command::EuBodies { .. } => "eu-bodies",
            Command::SyncEuJorf { .. } => "sync-eu-jorf",
            Command::MergeEuSwappedUids { .. } => "merge-eu-swapped-uids",
            Command::SyncEuLaw { .. } => "sync-eu-law",
            Command::IngestEuRproc { .. } => "ingest-eu-rproc",
            Command::StampFreshness => "stamp-freshness",
            Command::Invariants => "invariants",
            Command::ReadingRanks => "reading-ranks",
            Command::RelabelSources => "relabel-sources",
            Command::CanonicalizeSourceLabels => "canonicalize-source-labels",
            Command::IngestDila { .. } => "ingest-dila",
            Command::SyncDila { .. } => "sync-dila",
            Command::BackfillEcli => "backfill-ecli",
            Command::BuildSuggest => "build-suggest",
            Command::BackfillCanonicalRef { .. } => "backfill-canonical-ref",
            Command::MergeCrossSourceDuplicates => "merge-cross-source-duplicates",
            Command::AbsorbCopies { .. } => "absorb-copies",
            Command::AnalyzeFalseMerges => "analyze-false-merges",
            Command::ProbeIdentity { .. } => "probe-identity",
            Command::SeedJurisdictions => "seed-jurisdictions",
            Command::IngestCedh { .. } => "ingest-cedh",
            Command::CacheCedh { .. } => "cache-cedh",
            Command::IngestCjue { .. } => "ingest-cjue",
            Command::SyncCedh => "sync-cedh",
            Command::SyncCjue => "sync-cjue",
            Command::SyncCourtSummaries { .. } => "sync-court-summaries",
            Command::BackfillCjueJoinedCases { .. } => "backfill-cjue-joined-cases",
            Command::BackfillCjueTitles { .. } => "backfill-cjue-titles",
            Command::IngestCnda { .. } => "ingest-cnda",
            Command::SyncCnda => "sync-cnda",
            Command::IngestDdd { .. } => "ingest-ddd",
            Command::SyncDdd { .. } => "sync-ddd",
            Command::SyncAdlc { .. } => "sync-adlc",
            Command::SyncComeu { .. } => "sync-comeu",
            Command::Raa { .. } => "raa",
            Command::Gazette { .. } => "gazette",
            Command::SyncJuricaf { .. } => "sync-juricaf",
            Command::JuricafProbe { .. } => "juricaf-probe",
            Command::ScopeBackfillCompteurs { .. } => "scope-backfill-compteurs",
            Command::ScopeBackfillAlsace { .. } => "scope-backfill-alsace",
            Command::BackfillCndaSolution { .. } => "backfill-cnda-solution",
            Command::Registries { .. } => "registries",
            Command::PartiesRelink { .. } => "parties-relink",
            Command::LoadQuantumGold { .. } => "load-quantum-gold",
            Command::LoadQuantumReadings { .. } => "load-quantum-readings",
            Command::QuantumExtract { .. } => "quantum-extract",
            Command::Assertions(AssertionsCommand::Compile { .. }) => "assertions:compile",
            Command::Assertions(AssertionsCommand::Control) => "assertions:control",
            Command::Assertions(AssertionsCommand::Coverage) => "assertions:coverage",
            Command::Db(DbCommand::Analyze) => "db:analyze",
            Command::Db(DbCommand::ReindexSearch) => "db:reindex-search",
            Command::Db(DbCommand::VacuumFull) => "db:vacuum-full",
            Command::Db(DbCommand::GcMcp) => "db:gc-mcp",
            Command::Db(DbCommand::Prewarm) => "db:prewarm",
            Command::Db(DbCommand::PurgeOrphanTombstones { .. }) => "db:purge-orphan-tombstones",
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let settings = Settings::from_env()?;

    let _telemetry_guard = init_telemetry(&cli.log_level, &settings)?;
    install_panic_hook();
    lj_sources_annonces::poser_sortie(Some(settings.egress_proxy.clone()));

    let result = match cli.command {
        Command::Chain { kind } => chain::run(kind).await,
        command => run_with_watchdog(command, settings.watchdog_secs).await,
    };
    arm_teardown_sentinel();
    result
}

/// Délai accordé au teardown post-commande (flush télémétrie + drop du
/// runtime) avant que la sentinelle ne considère le process comme coincé.
const TEARDOWN_SENTINEL_SECS: u64 = 60;

fn arm_teardown_sentinel() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(TEARDOWN_SENTINEL_SECS));
        eprintln!(
            "lj-ingest: teardown stuck > {TEARDOWN_SENTINEL_SECS} s, thread state before abort:"
        );
        if let Ok(tasks) = std::fs::read_dir("/proc/self/task") {
            for task in tasks.flatten() {
                let path = task.path();
                let read = |f: &str| {
                    std::fs::read_to_string(path.join(f))
                        .unwrap_or_default()
                        .trim()
                        .to_string()
                };
                // stat : champ 3 = état (R/S/D/…), juste après le `comm`
                // parenthésé (qui peut contenir espaces et parenthèses → on
                // coupe après la DERNIÈRE parenthèse fermante).
                let stat = read("stat");
                let state = stat
                    .rsplit(')')
                    .next()
                    .and_then(|rest| rest.split_whitespace().next())
                    .unwrap_or("?")
                    .to_string();
                eprintln!(
                    "  tid={} comm={} state={} wchan={}",
                    task.file_name().to_string_lossy(),
                    read("comm"),
                    state,
                    read("wchan"),
                );
            }
        }
        std::process::abort();
    });
}

async fn run_with_watchdog(command: Command, watchdog_secs: u64) -> Result<()> {
    let phase = command.label();
    let started = Instant::now();
    tracing::info!(phase, "lj-ingest command: start");

    let result = if watchdog_secs == 0 {
        dispatch(command).await.map(Some)
    } else {
        match tokio::time::timeout(Duration::from_secs(watchdog_secs), dispatch(command)).await {
            Ok(inner) => inner.map(Some),
            Err(_) => Ok(None),
        }
    };

    let elapsed_s = started.elapsed().as_secs();
    match result {
        Ok(Some(())) => {
            tracing::info!(phase, elapsed_s, "lj-ingest command: success");
            Ok(())
        }
        Ok(None) => {
            tracing::error!(
                phase,
                elapsed_s,
                watchdog_secs,
                "lj-ingest command: watchdog deadline exceeded, aborting"
            );
            Err(anyhow!(
                "watchdog: la commande `{phase}` a dépassé {watchdog_secs} s"
            ))
        }
        Err(e) => {
            // `{e:#}` et non `{e}` : l'alternate déplie la chaîne anyhow sur une ligne.
            tracing::error!(phase, elapsed_s, error = %format!("{e:#}"), "lj-ingest command: failed");
            Err(e)
        }
    }
}

fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(panic = %info, "panic lj-ingest");
        default(info);
    }));
}

fn init_telemetry(log_level: &str, settings: &Settings) -> Result<TelemetryGuard> {
    let level = logging::resolve_level(Some(log_level));
    // `rustls_platform_verifier` loggue en ERROR chaque certificat invalide : le
    // recensement des profils d'acheteurs sonde des milliers de sites publics dont
    // beaucoup ont un certificat qui ne couvre pas leur propre nom. Le niveau global
    // est répété dans la chaîne de directives : la directive par défaut du builder ne
    // s'applique que si la chaîne est vide.
    let filter = EnvFilter::builder()
        .with_default_directive(level.into())
        .parse_lossy(format!("{level},rustls_platform_verifier=off"));

    let otlp = match (
        settings.grafana_otlp_endpoint.clone(),
        settings.grafana_otlp_user.clone(),
        settings.grafana_cloud_api_key.clone(),
    ) {
        (Some(endpoint), Some(user), Some(api_key)) => Some(OtlpCreds {
            endpoint,
            user,
            api_key,
        }),
        _ => None,
    };

    lj_telemetry::init(InitOpts {
        filter,
        json: logging::json_format(),
        service_name: settings.otel_service_name.clone(),
        deployment_environment: settings.deployment_environment.clone(),
        host_name: settings.host_name.clone(),
        container_name: settings.container_name.clone(),
        otlp_log_filter: Targets::new().with_default(LevelFilter::INFO),
        otlp,
    })
}

async fn dispatch(command: Command) -> Result<()> {
    match command {
        Command::Chain { .. } => unreachable!("Command::Chain est déroulée par main"),
        Command::Sync => cmd_sync().await,
        Command::JudilibreRanges => cmd_judilibre_ranges().await,
        Command::Analyze => cmd_analyze(),
        Command::Migrate => cmd_migrate().await,
        Command::Submissions { action } => match action {
            SubmissionsAction::List => pipeline::submissions::list().await,
            SubmissionsAction::Show { id } => pipeline::submissions::show(id).await,
            SubmissionsAction::Approve {
                id,
                court,
                number,
                date,
                name,
                code,
            } => {
                let approval = pipeline::submissions::Approval {
                    court,
                    number,
                    date,
                    name,
                    code,
                };
                pipeline::submissions::approve(id, approval).await
            }
            SubmissionsAction::Reject { id } => pipeline::submissions::reject(id).await,
            SubmissionsAction::Load { dir, dry_run, mode } => {
                pipeline::submissions::load(&dir, dry_run, mode).await
            }
        },
        Command::Credential { action } => match action {
            CredentialAction::Import {
                provider,
                from,
                tenants,
                account,
            } => credential::import(&provider, &from, tenants.as_deref(), account.as_deref()).await,
            CredentialAction::Seed => credential::seed().await,
            CredentialAction::List => credential::list().await,
            CredentialAction::Name {
                fingerprint,
                tenant,
                account,
            } => credential::name(&fingerprint, tenant.as_deref(), account.as_deref()).await,
            CredentialAction::Forget { fingerprint } => credential::forget(&fingerprint).await,
            CredentialAction::Disable {
                fingerprint,
                status,
                until_enabled,
            } => credential::disable(&fingerprint, status, until_enabled).await,
            CredentialAction::Enable { fingerprint } => credential::enable(&fingerprint).await,
            CredentialAction::Login => credential::login().await,
            CredentialAction::Refresh => credential::refresh().await,
            CredentialAction::Usage => credential::usage().await,
            CredentialAction::ResetCredit { label, force } => {
                credential::reset_credit(&label, force).await
            }
        },
        Command::Renvois => renvois::run().await,
        Command::AliasCandidats { revue } => alias_candidats::run(revue).await,
        Command::UsageTerms {
            chunks,
            min_citations,
            workers,
        } => usage_terms::run(chunks, min_citations, workers).await,
        Command::Ingest { mode } => cmd_ingest(mode).await,
        Command::Refetch { ids } => cmd_refetch(ids).await,
        Command::JudilibreReconcile { apply } => judilibre_reconcile::run(apply).await,
        Command::ReextractFields {
            dry_run,
            full,
            jurisdiction_type,
            jurisdiction_code,
            field,
            citing_ref_uid,
            text_query,
            several_provenances,
            workers,
        } => {
            cmd_reextract_fields(
                dry_run,
                full,
                jurisdiction_type,
                jurisdiction_code,
                field,
                citing_ref_uid,
                text_query,
                several_provenances,
                workers,
            )
            .await
        }
        Command::ResolveSeats { dry_run } => pipeline::resolve_seats(dry_run).await,
        Command::ResolveFormations { dry_run } => pipeline::resolve_formations(dry_run).await,
        Command::EmbedMissing {
            limit,
            stock,
            shard,
        } => pipeline::embed_missing(limit, stock, shard).await,
        Command::PurgeReverses => cmd_purge_reverses().await,
        Command::Withdraw {
            public_id,
            reason,
            lift,
        } => cmd_withdraw(&public_id, reason.as_deref(), lift).await,
        Command::GenerateSummaries {
            limit,
            refresh_stale,
            cited_min,
            by_jurisdiction,
            reverse,
            models,
            accounts,
            jurisdictions,
            named,
        } => {
            let order = match (cited_min, by_jurisdiction) {
                (Some(min), _) => summary::Order::Cited {
                    min,
                    only: jurisdictions,
                    named,
                },
                (None, true) => summary::Order::Jurisdiction {
                    reverse,
                    only: jurisdictions,
                    named,
                },
                (None, false) if jurisdictions.is_empty() && !named => summary::Order::Sweep,
                (None, false) => {
                    anyhow::bail!(
                        "--jurisdiction and --named need --cited-min or --by-jurisdiction"
                    )
                }
            };
            summary_pipeline_generate(limit, refresh_stale, order, models, accounts).await
        }
        Command::UpgradeSummariesCommentaries { limit } => {
            summary::upgrade_summaries_commentaries(500, limit.map(|l| l as i64)).await?;
            pipeline::sync_court_summaries(None, None).await
        }
        Command::RecollageSummaries {
            from_engine,
            jurisdiction,
            limit,
            models,
        } => summary_pipeline_recollage(from_engine, jurisdiction, limit, models).await,
        Command::RecleanSummaries => summary::reclean_summaries().await,
        Command::SummaryInputs { bench, out } => summary::export_inputs(&bench, &out).await,
        Command::SummaryFinish {
            answer,
            inputs,
            answers,
            out,
        } => summary::finish_answers(answer, &inputs, &answers, &out),
        Command::SummaryBatch {
            limit,
            reverse,
            wait,
        } => summary_batch::run(limit, reverse, wait).await,
        Command::ResyncLegalArrays => citations_maintenance::resync_arrays().await,
        Command::Precompute => pipeline::precompute_all().await,
        Command::ResyncJurisdictionCounts => pipeline::resync_jurisdiction_counts().await,
        Command::ResyncCorpusCounts => pipeline::resync_corpus_counts().await,
        Command::RekeyCnb { apply } => pipeline::rekey_cnb(apply).await,
        Command::CnbHistory { apply } => pipeline::replay_cnb_history(apply).await,
        Command::DeriveAffiliations { apply } => pipeline::derive_affiliations(apply).await,
        Command::Reconcile => pipeline::reconcile_all().await,
        Command::Sitemap { dry_run } => cmd_sitemap(dry_run).await,
        Command::Indexnow {
            since_hours,
            max_urls,
        } => cmd_indexnow(since_hours, max_urls).await,
        Command::DisposableDomains => disposable_domains::refresh().await,
        Command::Legi {
            paths,
            textes,
            stock_ancien,
        } => pipeline::ingest_legi(&paths, &textes, stock_ancien).await,
        Command::SyncLegi => pipeline::sync_legi().await,
        Command::AuditLegiVersions { text, paths } => {
            pipeline::audit_legi_versions(&text, &paths).await
        }
        Command::RedateDilaVersions { apply } => pipeline::redate_dila_versions(apply).await,
        Command::BackfillLinks { paths } => pipeline::backfill_links(&paths).await,
        Command::BackfillToc { paths } => pipeline::backfill_toc(&paths).await,
        Command::BackfillTextes { paths } => pipeline::backfill_textes(&paths).await,
        Command::Kali {
            paths,
            textes,
            stock_ancien,
        } => pipeline::ingest_kali(&paths, &textes, stock_ancien).await,
        Command::SyncKali => pipeline::sync_kali().await,
        Command::RequalifierKali { lot } => pipeline::requalifier_kali(lot).await,
        Command::Annonces {
            centrale_jours,
            relancer_muets,
        } => pipeline::annonces(centrale_jours, relancer_muets).await,
        Command::AnnoncesCouverture { limite } => pipeline::annonces_couverture(limite).await,
        Command::AnnoncesBackfill { depuis, jusqu } => {
            let date = |s: &str| {
                s.parse::<jiff::civil::Date>()
                    .map_err(|e| anyhow!("date {s} illisible: {e}"))
            };
            let depuis = depuis.as_deref().map(date).transpose()?;
            pipeline::annonces_backfill(depuis, date(&jusqu)?).await
        }
        Command::AnnoncesContenu { depuis, jusqu } => {
            let date = |s: &str| {
                s.parse::<jiff::civil::Date>()
                    .map_err(|e| anyhow!("date {s} illisible: {e}"))
            };
            let depuis = depuis.as_deref().map(date).transpose()?;
            pipeline::annonces_contenu(depuis, date(&jusqu)?).await
        }
        Command::AnnoncesEnrichir {
            limite,
            budget_min,
            sans_acheteur,
            sans_zone,
            sans_contenu,
        } => {
            use lj_store::repository::FileEnrichissement;
            let file = match (sans_acheteur, sans_zone, sans_contenu) {
                (true, false, false) => FileEnrichissement::SansAcheteur,
                (false, true, false) => FileEnrichissement::SansZone,
                (false, false, true) => FileEnrichissement::SansContenu,
                (false, false, false) => FileEnrichissement::NonVisitees,
                _ => anyhow::bail!("annonces-enrichir : une file à la fois"),
            };
            pipeline::annonces_enrichir(limite, budget_min, file).await
        }
        Command::AnnoncesDecp { depuis, applique } => {
            pipeline::annonces_decp(depuis.as_deref(), applique).await
        }
        Command::AnnoncesDiscover => pipeline::annonces_discover().await,
        Command::AnnoncesSonderLiens { par_moteur } => {
            pipeline::annonces_sonder_liens(par_moteur).await
        }
        Command::AnnoncesRegulariser { formes } => pipeline::annonces_regulariser(formes).await,
        Command::AnnoncesResoudre { applique, limite } => {
            pipeline::annonces_resoudre(limite, applique).await
        }
        Command::AnnoncesJumeler { applique } => pipeline::annonces_jumeler(applique).await,
        Command::Bofip { path } => pipeline::ingest_bofip(&path).await,
        Command::SyncBofip => pipeline::sync_bofip().await,
        Command::SyncCirculaires => pipeline::sync_circulaires().await,
        Command::SyncCirculairesBodies => pipeline::sync_circulaires_bodies().await,
        Command::SyncBulletins {
            site,
            full,
            dry_run,
        } => pipeline::sync_bulletins(site, full, dry_run).await,
        Command::SyncServicePublic => pipeline::sync_service_public().await,
        Command::BackfillTreatyBodies => pipeline::backfill_treaty_bodies().await,
        Command::SyncAriane {
            years,
            census_only,
            limit,
        } => pipeline::sync_ariane(years, census_only, limit).await,
        Command::SyncCeList => pipeline::sync_ce_list().await,
        Command::SyncWikiNames => pipeline::sync_wiki_names().await,
        Command::SyncDoctrineNames {
            scan,
            limit,
            dry_run,
        } => pipeline::sync_doctrine_names(scan, limit, dry_run).await,
        Command::SyncHal { scan } => pipeline::sync_hal(scan).await,
        Command::SyncDoctrineOai { scan, dry_run } => {
            pipeline::sync_doctrine_oai(scan, dry_run).await
        }
        Command::SeedArticleCommentaires => pipeline::seed_article_commentaires().await,
        Command::ExtractTextRefs => pipeline::extract_text_refs().await,
        Command::Jorf { paths } => pipeline::ingest_jorf(&paths).await,
        Command::SyncJorf => pipeline::sync_jorf().await,
        Command::JorfBodies => pipeline::backfill_jorf_bodies().await,
        Command::JorfPublications => pipeline::backfill_jorf_publications().await,
        Command::TreatyArticles { dry_run } => pipeline::treaty_articles(dry_run).await,
        Command::BackfillArticleText { fond, dry_run } => {
            pipeline::backfill_article_text(fond, dry_run).await
        }
        Command::BackfillDecisionText { fond, dry_run } => {
            pipeline::backfill_decision_text(fond, dry_run).await
        }
        Command::RecaseTexts { dry_run } => pipeline::recase_texts(dry_run).await,
        Command::AnnexStarts => pipeline::annex_starts().await,
        Command::HeadnoteTags => pipeline::headnote_tags().await,
        Command::AssignSlugs => pipeline::assign_slugs().await,
        Command::DropCountrySuffix => pipeline::drop_country_suffix().await,
        Command::RemintTextSlugs { applique } => pipeline::remint_text_slugs(applique).await,
        Command::AuditTocPaths => pipeline::audit_toc_paths().await,
        Command::RefileUnionTexts => pipeline::refile_union_texts().await,
        Command::BackfillTextRoles => pipeline::backfill_text_roles().await,
        Command::StampTexts => pipeline::stamp_texts().await,
        Command::MoveChronicle { vehicle, exclude } => {
            pipeline::move_chronicle(&vehicle, &exclude).await
        }
        Command::RenameTexts { dataset } => pipeline::rename_texts(&dataset).await,
        Command::RekeyArticleKeys => pipeline::rekey_article_keys().await,
        Command::RekeyIdentityKeys => pipeline::rekey_identity_keys().await,
        Command::DestarArticleKeys { applique } => pipeline::destar_article_keys(applique).await,
        Command::LoadLegalCorpus { only } => pipeline::load_legal_corpus(only.as_deref()).await,
        Command::IngestEuCatalog { limit } => pipeline::ingest_eu_catalog(limit).await,
        Command::SyncEuCatalog { full, limit } => pipeline::sync_eu_catalog(full, limit).await,
        Command::EuBodies { limit, dry_run } => pipeline::fill_eu_bodies(limit, dry_run).await,
        Command::SyncEuJorf { limit } => pipeline::sync_eu_jorf(limit).await,
        Command::MergeEuSwappedUids { apply } => pipeline::merge_eu_swapped_uids(apply).await,
        Command::SyncEuLaw {
            limit,
            only,
            force,
            backlog,
            concurrency,
        } => pipeline::sync_eu_law(limit, only.as_deref(), force, backlog, concurrency).await,
        Command::IngestEuRproc { dry_run } => pipeline::ingest_eu_rproc(dry_run).await,
        Command::StampFreshness => pipeline::stamp_freshness().await,
        Command::Invariants => invariants::run().await,
        Command::ReadingRanks => pipeline::reading_ranks().await,
        Command::RelabelSources => pipeline::relabel_sources().await,
        Command::CanonicalizeSourceLabels => pipeline::canonicalize_source_labels().await,
        Command::IngestDila { fond } => pipeline::ingest_dila(fond).await,
        Command::SyncDila { fonds } => {
            for fond in fonds {
                pipeline::sync_dila(fond).await?;
            }
            Ok(())
        }
        Command::BackfillEcli => pipeline::backfill_ecli().await,
        Command::BuildSuggest => pipeline::build_suggest().await,
        Command::BackfillCanonicalRef { force } => pipeline::backfill_canonical_ref(force).await,
        Command::MergeCrossSourceDuplicates => pipeline::merge_cross_source_duplicates().await,
        Command::AbsorbCopies { days } => pipeline::absorb_copies(days).await,
        Command::AnalyzeFalseMerges => pipeline::analyze_false_merges().await,
        Command::ProbeIdentity {
            days,
            source,
            out,
            apply,
        } => pipeline::probe_identity(days, &source, &out, apply).await,
        Command::SeedJurisdictions => pipeline::seed_jurisdictions().await,
        Command::IngestCedh { from_year } => pipeline::ingest_cedh(from_year).await,
        Command::CacheCedh { from_year } => pipeline::cache_cedh(from_year).await,
        Command::IngestCjue { short } => pipeline::ingest_cjue(short).await,
        Command::SyncCedh => pipeline::sync_cedh().await,
        Command::SyncCjue => pipeline::sync_cjue().await,
        Command::SyncCourtSummaries { from_year, to_year } => {
            pipeline::sync_court_summaries(from_year, to_year).await
        }
        Command::BackfillCjueJoinedCases { dry_run } => {
            pipeline::backfill_cjue_joined_cases(dry_run).await
        }
        Command::BackfillCjueTitles { dry_run } => pipeline::backfill_cjue_titles(dry_run).await,
        Command::IngestCnda { mode, only } => pipeline::ingest_cnda(mode, only).await,
        Command::SyncCnda => pipeline::sync_cnda().await,
        Command::IngestDdd { mode } => pipeline::ingest_ddd(mode).await,
        Command::SyncDdd { full } => pipeline::sync_ddd(full).await,
        Command::SyncAdlc { mode } => pipeline::sync_adlc(mode).await,
        Command::SyncComeu { full } => pipeline::sync_comeu(full).await,
        Command::Raa {
            since,
            pace,
            minutes,
            list_only,
        } => {
            pipeline::sync_raa(
                since,
                std::time::Duration::from_secs(pace),
                minutes,
                list_only,
            )
            .await
        }
        Command::Gazette {
            source,
            stock,
            offline,
            dry_run,
            verify,
            reclass,
        } => {
            let mode = match (dry_run, verify, reclass) {
                (true, _, _) => pipeline::GazetteMode::DryRun,
                (_, true, _) => pipeline::GazetteMode::Verify,
                (_, _, true) => pipeline::GazetteMode::Reclass,
                _ => pipeline::GazetteMode::Import,
            };
            pipeline::gazette(source, stock, offline, mode).await
        }
        Command::SyncJuricaf {
            prefixes,
            full,
            list,
        } => pipeline::sync_juricaf(&prefixes, full, list).await,
        Command::JuricafProbe { dir, mode } => pipeline::juricaf_probe(dir, mode).await,
        Command::ScopeBackfillCompteurs {
            collectivite,
            text_uid,
            apply,
        } => pipeline::scope_backfill_compteurs(collectivite, text_uid, apply).await,
        Command::ScopeBackfillAlsace { apply } => pipeline::scope_backfill_alsace(apply).await,
        Command::BackfillCndaSolution { apply } => pipeline::backfill_cnda_solution(apply).await,
        Command::Registries {
            source,
            with_history,
        } => pipeline::load_registries(source, with_history).await,
        Command::PartiesRelink { full } => pipeline::relink_parties(full).await,
        Command::LoadQuantumGold { dir } => pipeline::load_quantum_gold(&dir).await,
        Command::LoadQuantumReadings { lectures } => {
            pipeline::load_quantum_readings(&lectures[0], &lectures[1]).await
        }
        Command::QuantumExtract { limit } => pipeline::extract_quantum(limit).await,
        Command::Assertions(AssertionsCommand::Compile {
            full,
            days,
            text_uid,
            dry_run,
        }) => {
            pipeline::assertions_compile(pipeline::CompileOptions {
                full,
                days,
                text_uid,
                dry_run,
            })
            .await
        }
        Command::Assertions(AssertionsCommand::Control) => pipeline::assertions_control().await,
        Command::Assertions(AssertionsCommand::Coverage) => pipeline::assertions_coverage().await,
        Command::Db(db_command) => cmd_db(db_command).await,
    }
}

async fn pool_with_migrations(settings: &Settings) -> Result<deadpool_postgres::Pool> {
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    Ok(pool)
}

/// Pool deadpool sans migrations (pour les commandes de maintenance DB qui ne
/// veulent pas toucher au schéma). `pool_max` : 4 suffit partout sauf
/// `db vacuum-full` (une connexion par table en parallèle + la connexion
/// partagée de `cmd_db`).
async fn pool_only(settings: &Settings, pool_max: usize) -> Result<deadpool_postgres::Pool> {
    lj_pg::db::build_pool(&settings.db_url, pool_max).map_err(|e| anyhow!("build_pool: {e}"))
}

// ─────────────────────────────────────────────────────────────────────────────
// sync
// ─────────────────────────────────────────────────────────────────────────────

async fn cmd_sync() -> Result<()> {
    let settings = Settings::from_env()?;
    let cache_dir = settings.cache_dir();

    // Non-fatal : une panne transitoire opendata ne doit pas abandonner le
    // reste de la chaîne nocturne. On log et on continue.
    tracing::info!("=== sync opendata_conseil_etat ===");
    if let Err(e) = lj_sources::downloader::sync_opendata(&cache_dir, false) {
        tracing::error!(error = %e, "opendata sync failed, carrying on with Judilibre");
    }

    tracing::info!("=== sync judilibre ===");
    let client = judilibre_client(&settings)?;
    // `cmd_sync` n'expose pas `--from` → `date_start=None` (chaîne vide =
    // défaut par juridiction MONTHLY_START).
    lj_sources::downloader::sync_judilibre(&client, &cache_dir, "")
        .await
        .map_err(|e| anyhow!("sync judilibre: {e}"))?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// judilibre-ranges
// ─────────────────────────────────────────────────────────────────────────────

/// Une plage bornée par bisection : `(date_start, date_end, total)`.
type JudilibreRange = (String, String, i64);

/// Bisection one-shot des plages de bootstrap Judilibre (toutes les
/// juridictions, `max_chunk=50_000`, `date_from="1900-01-01"`).
async fn cmd_judilibre_ranges() -> Result<()> {
    use chrono::{Duration, NaiveDate, Utc};

    let settings = Settings::from_env()?;

    let max_chunk: i64 = 50_000;
    let date_from = "1900-01-01";
    let today = Utc::now().date_naive().format("%Y-%m-%d").to_string();

    let client = judilibre_client(&settings)?;
    let jurs = client
        .jurisdictions()
        .await
        .map_err(|e| anyhow!("taxonomy: {e}"))?;

    async fn scan_total(
        client: &lj_sources::judilibre::JudilibreClient,
        jur: &str,
        d_start: &str,
        d_end: &str,
    ) -> Result<i64> {
        let params: &[(&str, String)] = &[
            ("jurisdiction", jur.to_string()),
            ("date_type", "creation".to_string()),
            ("date_start", d_start.to_string()),
            ("date_end", d_end.to_string()),
            ("batch_size", "1".to_string()),
        ];
        let page = client
            .scan(params)
            .await
            .map_err(|e| anyhow!("scan: {e}"))?;
        Ok(page.get("total").and_then(|v| v.as_i64()).unwrap_or(0))
    }

    fn mid_date(d_start: &str, d_end: &str) -> Result<String> {
        let a = NaiveDate::parse_from_str(d_start, "%Y-%m-%d")?;
        let b = NaiveDate::parse_from_str(d_end, "%Y-%m-%d")?;
        let days = (b - a).num_days() / 2;
        Ok((a + Duration::days(days)).format("%Y-%m-%d").to_string())
    }

    // Bisection dichotomique itérative (pile explicite). Ordre de sortie
    // gauche-d'abord.
    async fn bisect(
        client: &lj_sources::judilibre::JudilibreClient,
        jur: &str,
        start: &str,
        end: &str,
        max_chunk: i64,
    ) -> Result<Vec<JudilibreRange>> {
        use chrono::Duration;
        let mut ranges: Vec<JudilibreRange> = Vec::new();
        let mut stack: Vec<(String, String)> = vec![(start.to_string(), end.to_string())];
        while let Some((d_start, d_end)) = stack.pop() {
            let total = scan_total(client, jur, &d_start, &d_end).await?;
            tracing::info!(%jur, %d_start, %d_end, total, "judilibre_range_scan");
            if total == 0 {
                continue;
            }
            if total <= max_chunk || d_start == d_end {
                ranges.push((d_start, d_end, total));
                continue;
            }
            let mid = mid_date(&d_start, &d_end)?;
            let next_day = (NaiveDate::parse_from_str(&mid, "%Y-%m-%d")? + Duration::days(1))
                .format("%Y-%m-%d")
                .to_string();
            // Empile la moitié droite d'abord pour traiter la gauche en premier.
            stack.push((next_day, d_end));
            stack.push((d_start, mid));
        }
        Ok(ranges)
    }

    let mut all_ranges: Vec<(String, Vec<JudilibreRange>)> = Vec::new();
    for jur in &jurs {
        tracing::info!(%jur, "--- bisect ---");
        let ranges = bisect(&client, jur, date_from, &today, max_chunk).await?;
        all_ranges.push((jur.to_string(), ranges));
    }

    println!("\n# BOOTSTRAP_RANGES");
    println!("BOOTSTRAP_RANGES: dict[str, list[tuple[str, str]]] = {{");
    for (jur, ranges) in &all_ranges {
        println!("    \"{jur}\": [");
        for (d_start, d_end, total) in ranges {
            println!("        (\"{d_start}\", \"{d_end}\"),  # {total}");
        }
        println!("    ],");
    }
    println!("}}");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// analyze
// ─────────────────────────────────────────────────────────────────────────────

/// Passe d'analyse du corpus local (défauts : `extra_dir=[]`, `sample=20`,
/// `seed=0`, sortie stdout).
fn cmd_analyze() -> Result<()> {
    let settings = Settings::from_env()?;
    let report = analyze::run(&settings.paths().opendata(), &[], 20, 0)?;
    let payload = serde_json::to_string_pretty(&report)?;
    println!("{payload}");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// migrate
// ─────────────────────────────────────────────────────────────────────────────

async fn cmd_migrate() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool = pool_only(&settings, 4).await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let applied =
        lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Apply)
            .await
            .map_err(|e| anyhow!("migrations: {e}"))?;
    if applied.is_empty() {
        println!("Database already up to date.");
    } else {
        println!("Migrations applied: {applied:?}");
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// ingest
// ─────────────────────────────────────────────────────────────────────────────

/// Ingests `opendata_conseil_etat`, then `judilibre`, each from its cache folder.
async fn cmd_ingest(mode: pipeline::IngestMode) -> Result<()> {
    let settings = Settings::from_env()?;
    pipeline::ingest_opendata(&settings.paths().opendata(), mode).await?;
    let judilibre = pipeline::ingest_judilibre(&settings.paths().judilibre(), mode).await;
    pipeline::resolve_pending_citations().await?;
    judilibre
}

// ─────────────────────────────────────────────────────────────────────────────
// reextract-fields
// ─────────────────────────────────────────────────────────────────────────────

/// Ré-extrait les champs structurés depuis les payloads stockés.
/// `--field` (vide → tous les champs ré-extractibles) ; `--juridiction-type`
/// (vide → keyset par version) cible un sous-ensemble quelle que soit la
/// version ; `--citing-ref-uid` cible les décisions citant un texte donné ;
/// `--full` = passe intégrale/relink hebdo (tout le fonds < 1000).
#[allow(clippy::too_many_arguments)]
async fn cmd_reextract_fields(
    dry_run: bool,
    full: bool,
    jurisdiction_type: Vec<String>,
    jurisdiction_code: Vec<String>,
    field: Vec<String>,
    citing_ref_uid: Option<String>,
    text_query: Option<String>,
    several_provenances: bool,
    workers: Option<usize>,
) -> Result<()> {
    let courts = if !jurisdiction_code.is_empty() {
        Some(Courts::Codes(jurisdiction_code))
    } else if !jurisdiction_type.is_empty() {
        Some(Courts::Types(jurisdiction_type))
    } else {
        None
    };
    let worklist = match (&citing_ref_uid, &courts) {
        _ if full => Worklist::Full,
        _ if several_provenances => Worklist::SeveralProvenances,
        (Some(uid), _) => Worklist::CitingRefUid(uid),
        (None, Some(courts)) => Worklist::Courts {
            courts,
            text_query: text_query.as_deref(),
        },
        (None, None) => Worklist::Stale,
    };
    let fields = (!field.is_empty()).then_some(field);
    pipeline::reextract_fields(fields.as_deref(), dry_run, worklist, workers).await
}

async fn cmd_refetch(ids: Vec<String>) -> Result<()> {
    let settings = Settings::from_env()?;
    let client = judilibre_client(&settings)?;
    pipeline::refetch_judilibre(&client, &ids).await
}

// ─────────────────────────────────────────────────────────────────────────────
// purge-reverses
// ─────────────────────────────────────────────────────────────────────────────

/// Hard-delete des décisions retirées (CSV `documents_reverses`).
async fn cmd_purge_reverses() -> Result<()> {
    let settings = Settings::from_env()?;
    let csv_dir = settings.paths().opendata().join("documents_reverses");
    if !csv_dir.exists() {
        return Err(anyhow!("Dossier introuvable : {}", csv_dir.display()));
    }
    let pool = pool_with_migrations(&settings).await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = lj_store::repository::DecisionRepository::new(&conn);
    let summaries = reverses::purge_all(&csv_dir, &repo).await?;
    for s in &summaries {
        println!(
            "{}: {} rows, {} decisions marked deleted, {} missing or already purged.",
            s.file, s.processed, s.newly_deleted, s.already_deleted_or_missing
        );
    }
    Ok(())
}

async fn cmd_withdraw(public_id: &str, reason: Option<&str>, lift: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool = pool_with_migrations(&settings).await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = lj_store::repository::DecisionRepository::new(&conn);
    if lift {
        if !repo.lift_withdrawal(public_id).await? {
            anyhow::bail!("{public_id}: not withdrawn");
        }
        println!("{public_id}: lifted, back at the next read of its source");
        return Ok(());
    }
    let reason = reason.expect("clap requires --reason without --lift");
    for uid in repo.withdraw(public_id, reason).await? {
        println!("{public_id}: {uid} tombstoned");
    }
    println!("{public_id}: withdrawn");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// generate-summaries
// ─────────────────────────────────────────────────────────────────────────────

async fn summary_pipeline_generate(
    limit: Option<usize>,
    refresh_stale: bool,
    order: summary::Order,
    models: Vec<summary::Model>,
    accounts: Vec<String>,
) -> Result<()> {
    summary::backfill_summaries(
        lj_summary::SUMMARY_PROMPT_VERSION,
        1000,
        limit.map(|l| l as i64),
        false,
        refresh_stale,
        order,
        if refresh_stale {
            summary::Doors::All
        } else {
            summary::Doors::ByRank
        },
        if models.is_empty() {
            summary::CHAIN_MODELS
        } else {
            &models
        },
        &accounts,
    )
    .await
}

async fn summary_pipeline_recollage(
    from_engine: i16,
    jurisdiction: Option<String>,
    limit: Option<usize>,
    models: Vec<summary::Model>,
) -> Result<()> {
    if from_engine != lj_summary::COURT_SUMMARY_ENGINE {
        lj_summary::SummaryEngine::from_code(from_engine)
            .ok_or_else(|| anyhow!("moteur de résumé inconnu : {from_engine}"))?;
    }
    summary::backfill_summaries(
        lj_summary::SUMMARY_PROMPT_VERSION,
        1000,
        limit.map(|l| l as i64),
        false,
        false,
        summary::Order::Engine {
            code: from_engine,
            jurisdiction,
        },
        summary::Doors::ByRank,
        if models.is_empty() {
            summary::CHAIN_MODELS
        } else {
            &models
        },
        &[],
    )
    .await
}

// ─────────────────────────────────────────────────────────────────────────────
// sitemap
// ─────────────────────────────────────────────────────────────────────────────

/// Fenêtre d'archive des fiches de consultation publiées au sitemap.
const ANNONCES_SITEMAP_ARCHIVE_JOURS: i32 = 180;

/// Source sitemap matérialisée : décisions `(public_id, lastmod)` et articles de
/// référentiel `(slug, num, lastmod)` pré-chargés depuis le repo (les méthodes
/// repo sont async, le trait `SitemapSource` est sync).
struct LoadedSitemapSource {
    decisions: Vec<(String, sitemap::SourceLastmod)>,
    referential: Vec<(String, String, String, sitemap::SourceLastmod)>,
    entities: Vec<(String, String, sitemap::SourceLastmod)>,
    texts: Vec<(String, String, sitemap::SourceLastmod)>,
    jurisdiction_hubs: sitemap::JurisdictionHubPages,
    norm_hubs: sitemap::NormHubPages,
    annonces: (Vec<(String, sitemap::SourceLastmod)>, Vec<String>),
}

impl sitemap::SitemapSource for LoadedSitemapSource {
    fn iter_decisions_for_sitemap(&self) -> Result<Vec<(String, sitemap::SourceLastmod)>> {
        Ok(self.decisions.clone())
    }
    fn iter_referential_for_sitemap(
        &self,
    ) -> Result<Vec<(String, String, String, sitemap::SourceLastmod)>> {
        Ok(self.referential.clone())
    }
    fn iter_entities_for_sitemap(&self) -> Result<Vec<(String, String, sitemap::SourceLastmod)>> {
        Ok(self.entities.clone())
    }
    fn iter_texts_for_sitemap(&self) -> Result<Vec<(String, String, sitemap::SourceLastmod)>> {
        Ok(self.texts.clone())
    }
    fn iter_jurisdiction_hubs_for_sitemap(&self) -> Result<sitemap::JurisdictionHubPages> {
        Ok(self.jurisdiction_hubs.clone())
    }
    fn iter_norm_hubs_for_sitemap(&self) -> Result<sitemap::NormHubPages> {
        Ok(self.norm_hubs.clone())
    }
    fn iter_annonces_for_sitemap(&self) -> Result<sitemap::AnnoncesSitemap> {
        Ok(self.annonces.clone())
    }
}

fn jurisdiction_type(code: &str) -> Result<lj_dtos::JurisdictionType> {
    lj_dtos::JurisdictionType::from_code(code)
        .ok_or_else(|| anyhow!("unknown jurisdiction_type {code}"))
}

/// Génère les sitemaps et les publie en base.
///
/// Charge les décisions via le repo, construit l'ensemble en mémoire, puis
/// (sauf `--dry-run`) remplace la table `sitemaps` dans une transaction
/// unique — `lj-server` sert ces lignes sous `/sitemap.xml` +
/// `/sitemap-{n}.xml.gz`.
async fn cmd_sitemap(dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;

    tracing::info!("=== build sitemaps ===");
    let hub_page_size = i64::from(lj_vocab::HUB_PAGE_SIZE);
    let pool = pool_with_migrations(&settings).await?;
    let (decisions, referential, entities, texts, jurisdiction_hubs, norm_hubs, annonces) = {
        let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = lj_store::repository::DecisionRepository::new(&conn);
        let decisions = repo
            .iter_decisions_for_sitemap(sitemap::MAX_URLS_PER_SITEMAP as i64)
            .await
            .map_err(|e| anyhow!("iter_decisions_for_sitemap: {e}"))?
            .into_iter()
            .map(|(id, jt, code, lastmod)| {
                Ok((
                    lj_dtos::decision_path(jurisdiction_type(&jt)?, code.as_deref(), &id),
                    lastmod,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let referential = repo
            .iter_referential_for_sitemap()
            .await
            .map_err(|e| anyhow!("iter_referential_for_sitemap: {e}"))?;
        let entities = repo
            .iter_entities_for_sitemap()
            .await
            .map_err(|e| anyhow!("iter_entities_for_sitemap: {e}"))?;
        let texts = repo
            .iter_texts_for_sitemap()
            .await
            .map_err(|e| anyhow!("iter_texts_for_sitemap: {e}"))?;
        let jurisdiction_hubs = repo
            .iter_jurisdiction_hubs_for_sitemap(hub_page_size)
            .await
            .map_err(|e| anyhow!("iter_jurisdiction_hubs_for_sitemap: {e}"))?
            .into_iter()
            .map(|(jt, code, years)| {
                Ok((
                    jurisdiction_type(&jt)?.url_segment(Some(code.as_str())),
                    code,
                    years,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let norm_hubs = repo
            .iter_norm_hubs_for_sitemap(hub_page_size)
            .await
            .map_err(|e| anyhow!("iter_norm_hubs_for_sitemap: {e}"))?
            .into_iter()
            .map(|(juris, fond, tokens)| (lj_vocab::url_segment(&juris), fond, tokens))
            .collect();
        // Annonces : la fenêtre ouverte + l'archive récente pour les fiches, et
        // les départements qui ont au moins une consultation ouverte pour les
        // hubs — au-delà, la page répond 404.
        let fond = lj_vocab::ANNONCE_FOND_MARCHE;
        let fiches = repo
            .annonces_sitemap(fond, ANNONCES_SITEMAP_ARCHIVE_JOURS)
            .await
            .map_err(|e| anyhow!("annonces_sitemap: {e}"))?
            .into_iter()
            .map(|(uid, date)| (uid, Some(date)))
            .collect();
        let departements = repo
            .annonces_facette(
                &lj_store::repository::AnnonceFiltre::nouveau(fond),
                lj_store::repository::AnnonceAxe::Zone,
            )
            .await
            .map_err(|e| anyhow!("annonces_facette(departement): {e}"))?
            .into_iter()
            .map(|(code, _, _)| code)
            .collect();
        (
            decisions,
            referential,
            entities,
            texts,
            jurisdiction_hubs,
            norm_hubs,
            (fiches, departements),
        )
    };
    let source = LoadedSitemapSource {
        decisions,
        referential,
        entities,
        texts,
        jurisdiction_hubs,
        norm_hubs,
        annonces,
    };
    let files = sitemap::build_sitemaps(&source, chrono::Utc::now().date_naive())?;

    let sub_count = files.len() - 1; // index exclu
    tracing::info!(
        sub_count,
        index = sitemap::SITEMAP_INDEX_NAME,
        "sitemaps generated"
    );

    if dry_run {
        tracing::info!("--dry-run: nothing written to the database.");
        return Ok(());
    }

    // Régénération complète atomique : DELETE + INSERT en une transaction.
    let rows: Vec<lj_store::repository::SitemapRow> = files
        .into_iter()
        .map(|f| lj_store::repository::SitemapRow {
            filename: f.filename,
            content_type: f.content_type.to_string(),
            body: f.body,
            lastmod: f.lastmod,
        })
        .collect();
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = lj_store::repository::DecisionRepository::new(&conn);
    conn.batch_execute("BEGIN").await?;
    match repo.replace_sitemaps(&rows).await {
        Ok(()) => conn.batch_execute("COMMIT").await?,
        Err(e) => {
            let _ = conn.batch_execute("ROLLBACK").await;
            return Err(anyhow!("replace_sitemaps: {e}"));
        }
    }
    tracing::info!(count = rows.len(), "sitemaps published to the database");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// indexnow
// ─────────────────────────────────────────────────────────────────────────────

/// Pushes to IndexNow what changed at the source within the window: decisions whose
/// provenance was (re)ingested, and consultations published, under the `max_urls` cap.
async fn cmd_indexnow(since_hours: f64, max_urls: usize) -> Result<()> {
    use chrono::{Duration, Utc};

    let settings = Settings::from_env()?;
    let Some(key) = settings.indexnow_key.as_deref() else {
        return Err(anyhow!(
            "LIBREJUSTICE_INDEXNOW_KEY non posé — clé IndexNow requise."
        ));
    };
    let since = Utc::now() - Duration::milliseconds((since_hours * 3_600_000.0) as i64);

    let pool = pool_with_migrations(&settings).await?;
    let (urls, truncated) = {
        let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
        // `ingested_at` carries no index, so the selection is a full seq scan that
        // can outlive the default 30 s `statement_timeout`. Lifted on THIS connection
        // only: `indexnow` is a one-shot CLI process.
        conn.batch_execute("SET statement_timeout = 0").await?;
        let repo = lj_store::repository::DecisionRepository::new(&conn);
        let decisions = repo
            .iter_public_ids_ingested_since(since, max_urls as i64)
            .await
            .map_err(|e| anyhow!("iter_public_ids_ingested_since: {e}"))?;
        let annonces = repo
            .annonces_vues_depuis(lj_vocab::ANNONCE_FOND_MARCHE, since, max_urls as i64)
            .await
            .map_err(|e| anyhow!("annonces_vues_depuis: {e}"))?;
        // Les consultations passent devant : leur fenêtre utile est l'échéance, à
        // trois ou quatre semaines, alors qu'une décision reste indexable des
        // années. Sous le cap, c'est donc la queue des décisions qui cède.
        let mut all: Vec<String> = annonces.iter().map(|u| indexnow::notice_url(u)).collect();
        for (id, jt, code) in &decisions {
            let path = lj_dtos::decision_path(jurisdiction_type(jt)?, code.as_deref(), id);
            all.push(indexnow::decision_url(&path));
        }
        let truncated = all.len() > max_urls;
        all.truncate(max_urls);
        (all, truncated)
    };

    if truncated {
        tracing::warn!(
            max_urls,
            since_hours,
            "plus d'URLs (re)publiées que la borne — soumission tronquée. \
             Relancer avec --max-urls plus haut, ou par fenêtres --since-hours plus courtes."
        );
    }
    let submitted = indexnow::submit(&urls, key).await?;
    tracing::info!(submitted, "IndexNow: URL(s) submitted.");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// db
// ─────────────────────────────────────────────────────────────────────────────

/// `statement_timeout` of heavy maintenance (ANALYZE, VACUUM FULL, REINDEX, prewarm), above the
/// pool's: a session `SET` overrides the startup `options`, finite so a runaway op is killed.
const MAINTENANCE_STATEMENT_TIMEOUT: &str = "2h";

/// `statement_timeout` of `db reindex-search`. Its six `REINDEX CONCURRENTLY` wait on
/// each other's snapshots and end together, at the pace of the largest.
const REINDEX_STATEMENT_TIMEOUT: &str = "4h";

/// Maintenance DB. Toutes ouvrent une connexion en `autocommit` et
/// exécutent du SQL de maintenance — pas de migrations.
async fn cmd_db(command: DbCommand) -> Result<()> {
    let settings = Settings::from_env()?;
    // The shared connection below, one per parallel table of `db vacuum-full`
    // and `db reindex-search`, and the blocker sweep the latter holds throughout.
    let pool = pool_only(&settings, SEARCH_TABLES.len() + 2).await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Relève le statement_timeout pour les ops lourdes. `gc-mcp` (DELETE borné)
    // garde le garde-fou 30 s.
    if !matches!(command, DbCommand::GcMcp) {
        conn.batch_execute(&format!(
            "SET statement_timeout = '{MAINTENANCE_STATEMENT_TIMEOUT}'"
        ))
        .await?;
    }
    match command {
        DbCommand::Analyze => db_analyze(&conn).await,
        DbCommand::ReindexSearch => db_reindex_search(&pool).await,
        DbCommand::VacuumFull => db_vacuum_full(&pool).await,
        DbCommand::GcMcp => db_gc_mcp(&conn).await,
        DbCommand::Prewarm => db_prewarm(&conn).await,
        DbCommand::PurgeOrphanTombstones { apply } => {
            db_purge_orphan_tombstones(&conn, apply).await
        }
    }
}

/// Purge les tombstones orphelins (retraits totaux vidés sans provenance active,
/// cf. `DecisionRepository::purge_orphan_tombstones`). Ceinture-bretelle :
/// dry-run par défaut (compte + échantillon, aucune suppression) ; `--apply`
/// requis pour supprimer ; re-vérifie l'absence d'orphelin résiduel après coup.
async fn db_purge_orphan_tombstones(conn: &lj_pg::db::Connection, apply: bool) -> Result<()> {
    let repo = lj_store::repository::DecisionRepository::new(conn);
    let n = repo.count_orphan_tombstones().await?;
    println!("Orphan tombstones (withdrawn, emptied, no active provenance): {n}");
    let sample = repo.sample_orphan_tombstones(5).await?;
    if !sample.is_empty() {
        println!("source_uid sample: {}", sample.join(", "));
    }
    if n == 0 {
        println!("Nothing to purge.");
        return Ok(());
    }
    if !apply {
        println!("Dry-run: nothing deleted. Re-run with --apply to purge.");
        return Ok(());
    }
    let deleted = repo.purge_orphan_tombstones().await?;
    println!("{deleted} decisions deleted (cascade provenances/chunks/full_text/citations).");
    let remaining = repo.count_orphan_tombstones().await?;
    if remaining != 0 {
        return Err(anyhow!(
            "purge incomplète : {remaining} orphelins résiduels"
        ));
    }
    println!("Verified: 0 orphan left.");
    Ok(())
}

async fn db_analyze(conn: &lj_pg::db::Connection) -> Result<()> {
    // Sans transaction ouverte, chaque `execute` tokio-postgres s'auto-commit.
    // ANALYZE ne tourne pas dans un bloc.
    conn.execute("ANALYZE decisions", &[]).await?;
    conn.execute("ANALYZE decision_chunks", &[]).await?;
    conn.execute("ANALYZE decision_full_text", &[]).await?;
    // Stats fraîches pour l'overlay, les backlinks (GIN `lj_cit_terms`) et le
    // service `/norm/`.
    conn.execute("ANALYZE legal_citation", &[]).await?;
    conn.execute("ANALYZE legal_text", &[]).await?;
    conn.execute("ANALYZE legal_article", &[]).await?;
    // The annonce tables grow by a sliver of their size a day, which never reaches
    // the autoanalyze threshold.
    conn.execute("ANALYZE annonce", &[]).await?;
    conn.execute("ANALYZE annonce_source", &[]).await?;
    conn.execute("ANALYZE annonce_entity", &[]).await?;
    println!("ANALYZE done.");
    Ok(())
}

/// Drops the **invalid** indexes of a table and of its TOAST table. An interrupted
/// `REINDEX CONCURRENTLY` (deadlock, kill, timeout) leaves an `<index>_ccnew` with
/// `indisvalid = false`: never read, but `indisready = true`, so **maintained on
/// every write** at its full size. Called before AND after the reindex.
async fn drop_invalid_indexes(conn: &lj_pg::db::Connection, table: &str) -> Result<()> {
    let rows = conn
        .query(
            "SELECT format('%I.%I', n.nspname, c.relname) FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             JOIN pg_index i ON i.indexrelid = c.oid \
             WHERE NOT i.indisvalid AND i.indrelid IN (to_regclass($1), \
                   (SELECT reltoastrelid FROM pg_class WHERE oid = to_regclass($1)))",
            &[&table],
        )
        .await?;
    for row in &rows {
        let index: String = row.get(0);
        println!(
            "DROP INDEX CONCURRENTLY {index} (invalid, leftover from an interrupted reindex) …"
        );
        conn.batch_execute(&format!("DROP INDEX CONCURRENTLY {index}"))
            .await?;
    }
    Ok(())
}

/// Pooled connections carry a short `statement_timeout`: an older open transaction
/// is a session out of pool, typically a `psql` whose client died unnoticed.
const MAINTENANCE_XACT_GRACE: &str = "5 minutes";

/// A blocker arriving mid-reindex costs as much as one already there.
const MAINTENANCE_SWEEP: std::time::Duration = std::time::Duration::from_secs(60);

/// Cancels transactions older than [`MAINTENANCE_XACT_GRACE`], ours excepted.
/// `REINDEX` and `DROP INDEX CONCURRENTLY` wait on every snapshot older than
/// their own, on any table.
async fn cancel_blocking_transactions(conn: &lj_pg::db::Connection) -> Result<()> {
    let rows = conn
        .query(
            "SELECT pid, application_name, \
                    extract(epoch FROM now() - xact_start)::bigint \
               FROM pg_stat_activity \
              WHERE backend_type = 'client backend' \
                AND pid <> pg_backend_pid() \
                AND application_name IS DISTINCT FROM current_setting('application_name') \
                AND xact_start < now() - $1::text::interval",
            &[&MAINTENANCE_XACT_GRACE],
        )
        .await?;
    for row in &rows {
        let pid: i32 = row.get(0);
        let app: String = row.get(1);
        let age: i64 = row.get(2);
        conn.execute("SELECT pg_cancel_backend($1)", &[&pid])
            .await?;
        println!("maintenance: {age} s transaction cancelled (pid {pid}, application {app}).");
    }
    Ok(())
}

/// Maintenance hebdomadaire des tables de recherche :
/// `REINDEX TABLE CONCURRENTLY` puis `VACUUM`, table par table, en
/// parallèle. Zéro indisponibilité.
///
/// `REINDEX TABLE` et non `REINDEX INDEX` sur les seuls BM25 : les btrees
/// bloatent tout autant. Le rebuild purge aussi les tombstones tantivy des
/// index BM25, que la politique de merge par couches ne reprend jamais.
///
/// Le `VACUUM` simple qui suit rend l'espace mort du heap réutilisable sur
/// place et entretient la visibility map ; il ne rend rien à l'OS.
///
/// `CONCURRENTLY` ne peut pas tourner dans un bloc transactionnel →
/// `batch_execute` (protocole simple, hors transaction implicite des
/// requêtes paramétrées). Un échec ne coûte que sa semaine : les index en
/// place restent valides et servent, l'orphelin est nettoyé, et la chaîne
/// remonte `chain_failed`. Pas de retry.
async fn db_reindex_search(pool: &deadpool_postgres::Pool) -> Result<()> {
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    cancel_blocking_transactions(&conn).await?;
    let sweeper = tokio::spawn(async move {
        loop {
            tokio::time::sleep(MAINTENANCE_SWEEP).await;
            if let Err(e) = cancel_blocking_transactions(&conn).await {
                eprintln!("maintenance: anti-lock net failed: {e}");
            }
        }
    });

    let mut handles = Vec::new();
    for table in SEARCH_TABLES {
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
            conn.batch_execute(&format!(
                "SET statement_timeout = '{REINDEX_STATEMENT_TIMEOUT}'"
            ))
            .await?;
            drop_invalid_indexes(&conn, table).await?;

            println!("REINDEX TABLE CONCURRENTLY {table} … [no exclusive lock]");
            let started = std::time::Instant::now();
            let outcome = conn
                .batch_execute(&format!("REINDEX TABLE CONCURRENTLY {table}"))
                .await;
            if let Err(e) = outcome {
                drop_invalid_indexes(&conn, table).await?;
                return Err(anyhow!("REINDEX {table} : {e}"));
            }
            println!("REINDEX {table} done in {} s.", started.elapsed().as_secs());

            let started = std::time::Instant::now();
            println!("VACUUM {table} …");
            conn.batch_execute(&format!("VACUUM {table}")).await?;
            println!("VACUUM {table} done in {} s.", started.elapsed().as_secs());
            Ok::<(), anyhow::Error>(())
        }));
    }
    let mut failures = Vec::new();
    for (table, handle) in SEARCH_TABLES.iter().zip(handles) {
        if let Err(e) = handle.await? {
            failures.push(format!("{table}: {e}"));
        }
    }
    sweeper.abort();
    if !failures.is_empty() {
        return Err(anyhow!(
            "reindex-search en échec — {}",
            failures.join(" ; ")
        ));
    }
    Ok(())
}

/// Tables porteuses d'index de recherche, couvertes par la maintenance
/// hebdomadaire `db reindex-search` et par le repli manuel `db vacuum-full`.
const SEARCH_TABLES: [&str; 6] = [
    "decision_text",
    "decision_chunks",
    "legal_article",
    "legal_text",
    "legal_article_usage",
    "decisions",
];

/// `VACUUM FULL` des tables de recherche : compacte le heap et
/// reconstruit **tous** leurs index — seul moyen de purger les tombstones
/// tantivy des gros segments, que les merges par couches ne reprennent
/// jamais. Les cinq tables passent en **parallèle** (une connexion chacune) :
/// le wall-clock tient à la plus grosse. Lock AccessExclusive par table
/// pendant sa réécriture ; un échec n'interrompt pas les autres (join
/// complet puis erreur agrégée).
///
/// Chaque table enchaîne sur un `VACUUM` simple qui reconstruit sa
/// visibility map, que le `VACUUM FULL` laisse vide.
async fn db_vacuum_full(pool: &deadpool_postgres::Pool) -> Result<()> {
    let maintenance_work_mem = "512MB";
    let mut handles = Vec::new();
    for table in SEARCH_TABLES {
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
            // Timeout maintenance + work_mem par connexion (le SET de session
            // de `cmd_db` ne vaut que pour la connexion partagée).
            conn.batch_execute(&format!(
                "SET statement_timeout = '{MAINTENANCE_STATEMENT_TIMEOUT}'; \
                 SET maintenance_work_mem = '{maintenance_work_mem}'"
            ))
            .await?;
            println!("VACUUM FULL {table} (mwm={maintenance_work_mem}) … [lock AccessExclusive]");
            let started = std::time::Instant::now();
            // VACUUM FULL ne peut pas tourner dans un bloc transactionnel.
            conn.batch_execute(&format!("VACUUM FULL {table}")).await?;
            println!(
                "VACUUM FULL {table} done in {} s.",
                started.elapsed().as_secs()
            );
            // VACUUM FULL réécrit le heap et laisse la visibility map VIDE. Tant
            // qu'elle n'est pas reconstruite, tout Index Only Scan va chercher chaque
            // ligne dans le heap pour tester sa visibilité — l'« index only » ne l'est
            // plus. L'autovacuum ne rattrape pas : il se déclenche sur un ratio de
            // lignes mortes que ces tables, fraîchement compactées, mettent des
            // semaines à atteindre. Un VACUUM simple (lock ShareUpdateExclusive, ni
            // lecture ni écriture bloquée) la repose.
            let started = std::time::Instant::now();
            println!("VACUUM {table} (visibility map) …");
            conn.batch_execute(&format!("VACUUM {table}")).await?;
            println!("VACUUM {table} done in {} s.", started.elapsed().as_secs());
            Ok::<(), anyhow::Error>(())
        }));
    }
    let mut failures = Vec::new();
    for (table, handle) in SEARCH_TABLES.iter().zip(handles) {
        if let Err(e) = handle.await? {
            failures.push(format!("{table}: {e}"));
        }
    }
    if !failures.is_empty() {
        return Err(anyhow!("VACUUM FULL en échec — {}", failures.join(" ; ")));
    }
    Ok(())
}

/// Purge des tokens/codes MCP expirés — les trois tables.
///
/// `mcp_refresh_tokens` ne fait que croître, sur des lignes qu'aucun chemin
/// ne peut plus valider (`token_refresh` filtre sur `expires_at > now()`).
async fn db_gc_mcp(conn: &lj_pg::db::Connection) -> Result<()> {
    let tokens = conn
        .execute("DELETE FROM mcp_tokens WHERE expires_at < now()", &[])
        .await?;
    let refresh = conn
        .execute(
            "DELETE FROM mcp_refresh_tokens WHERE expires_at < now()",
            &[],
        )
        .await?;
    let codes = conn
        .execute("DELETE FROM mcp_auth_codes WHERE expires_at < now()", &[])
        .await?;
    println!("MCP GC: {tokens} access + {refresh} refresh + {codes} expired code(s) deleted.");
    Ok(())
}

/// Prewarm des index de recherche après un redémarrage : sans lui, la
/// première requête paie l'ouverture des segments à froid.
///
/// La liste est **découverte dans le catalogue**, pas écrite ici : un index
/// neuf est couvert le jour où il existe.
///
/// Deux caches distincts, donc deux modes et aucune concurrence entre eux :
/// l'ANN VectorChord est épinglé en `shared_buffers`, le BM25 est tiré en
/// mode `read` (cache page OS) — les index dépassent ensemble
/// `shared_buffers`.
async fn db_prewarm(conn: &lj_pg::db::Connection) -> Result<()> {
    // Le plus gros d'abord : si le cache manque, c'est lui qu'on veut dedans.
    const INDEXES: &str = "SELECT am.amname, c.relname, t.relname, pg_relation_size(c.oid) \
         FROM pg_index i \
         JOIN pg_class c ON c.oid = i.indexrelid \
         JOIN pg_class t ON t.oid = i.indrelid \
         JOIN pg_am am ON am.oid = c.relam \
         WHERE am.amname IN ('vchordrq', 'bm25') \
         ORDER BY pg_relation_size(c.oid) DESC";

    let mut chauffes = 0usize;
    for row in conn.query(INDEXES, &[]).await? {
        let am: &str = row.get(0);
        let index: &str = row.get(1);
        let table: &str = row.get(2);
        let octets: i64 = row.get(3);

        // Les index du banc vivent dans la même base mais aucune requête servie ne
        // les lit.
        if table.starts_with("bench_") {
            println!("{index}: skipped (bench index, off the served path)");
            continue;
        }

        let go = octets as f64 / 1e9;
        if am == "vchordrq" {
            println!("vchordrq_prewarm('{index}') … [{go:.1} GB, pinned in shared_buffers]"); // sql-splice-ok: progress line, the query below binds $1
            let r = conn
                .query_one("SELECT vchordrq_prewarm($1::text::regclass, 0)", &[&index])
                .await?;
            // Rend un RAPPORT texte multi-ligne (hauteur de l'arbre, nœuds, pages),
            // pas un compte de blocs.
            let rapport: String = r.get(0);
            for ligne in rapport.lines().filter(|l| !l.trim_matches('-').is_empty()) {
                println!("  {}", ligne.trim_end());
            }
        } else {
            println!("pg_prewarm('{index}', 'read') … [{go:.1} GB, OS page cache]"); // sql-splice-ok: progress line, the query below binds $1
            let r = conn
                .query_one("SELECT pg_prewarm($1::text::regclass, 'read')", &[&index])
                .await?;
            // `pg_prewarm` rend le nombre de blocs (int8) chargés.
            let blocs: i64 = r.get(0);
            println!("  {blocs} blocs");
        }
        chauffes += 1;
    }
    println!("{chauffes} indexes prewarmed");
    Ok(())
}
